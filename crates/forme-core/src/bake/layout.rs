//! Bake "par lignes": the mesh is deduced from the construction lines.
//!
//! 1. Every face of every shape that shows on the final surface becomes a
//!    chart: a flat face of a box is a rectangle in its plane, a rounded
//!    edge is a strip (along the edge × across its arc). The other shapes
//!    cut the chart along their own faces: these cuts are the construction
//!    lines (junctions, rails), found exactly in 2D. Two charts share each
//!    line, so both sides of a line get the same vertices.
//! 2. Fillets first: the number of rows across each strip is fixed (bevel
//!    segments), the strips are drawn orange in the role view.
//! 3. Loops: from each re-entrant corner (sharp, or the middle of a concave
//!    fillet) lines run straight across the face and on around the part,
//!    face after face, until they come back: the edge loops a modeller cuts.
//! 4. Each face is now cut into four-sided patches. The number of segments
//!    of every line is solved so that opposite sides of each patch agree,
//!    then every patch is filled with a grid of quads (transfinite
//!    interpolation in its chart), every vertex exactly on its face.
//!
//! Handled for now: boxes with sharp or rounded edges (a ball where three
//! equal fillets meet) and sharp cylinders, combined by sharp Add / Carve,
//! as long as every cylinder wall is hidden somewhere (its seam goes
//! there). Anything else returns Err(reason), shown in the Bake panel.

use super::field::{box_edge_signs, prim_raw, Field, Instance, PrimDef, Step};
use super::lines::edge_r;
use super::{finish, BFace, BakeSettings, Baked, Progress, P_FILL, P_FILLET};
use crate::program::compile;
use crate::scene::Scene;
use glam::{DVec2, DVec3};
use std::collections::{HashMap, VecDeque};
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::time::Instant;

/// Faces of a box: 0..5 flat (axis * 2 + negative side), 6 + e rounded edge e.
const FILLET0: u32 = 6;
/// Faces of a cylinder (axis along local y).
const CYL_SIDE: u32 = 0;
const CYL_TOP: u32 = 1;
const CYL_BOTTOM: u32 = 2;
/// Second half of a cylinder wall seen all around (a round hole).
const CYL_SIDE_B: u32 = 3;
/// Rounded rims of a cylinder: CYL_RIM0 + 0 the top one, + 1 the bottom one
/// (the same numbers as the box's rounded edges, FILLET0 + rim); their
/// second halves, with the wall's, CYL_RIM_B0 + rim.
const CYL_RIM0: u32 = FILLET0;
const CYL_RIM1: u32 = FILLET0 + 1;
const CYL_RIM_B0: u32 = FILLET0 + 2;
/// Faces of a box: 18 + k a rounded corner (ball) where three edges rounded
/// alike meet; k from the signs, bit 0 x < 0, bit 1 y < 0, bit 2 z < 0.
const CORNER0: u32 = 18;
/// Junction fillets (blend of a step): BLEND0 + index in `Layout::blends`,
/// with the prim of the shape added by that step.
const BLEND0: u32 = 1000;

/// Profile of a smooth union (quadratic smooth min) between two square
/// planes, heights u over the first and v off the second: √u + √v = √k,
/// u = k t², v = k (1 - t)², t from 0 (rail on the first) to 1. Arc length
/// from t = 0, over k.
fn blend_s(t: f64) -> f64 {
    let f = |x: f64| {
        let r = (x * x + 0.25).sqrt();
        0.5 * x * r + 0.125 * (x + r).ln()
    };
    2.0 * std::f64::consts::SQRT_2 * (f(t - 0.5) - f(-0.5))
}

/// Inverse of `blend_s`.
fn blend_t(s: f64) -> f64 {
    let total = blend_s(1.0);
    let s = s.clamp(0.0, total);
    let mut t = s / total;
    for _ in 0..30 {
        let ds = 2.0 * (t * t + (1.0 - t) * (1.0 - t)).sqrt();
        let nt = (t - (blend_s(t) - s) / ds).clamp(0.0, 1.0);
        if (nt - t).abs() < 1e-15 {
            break;
        }
        t = nt;
    }
    t
}

/// Height of a smooth union around a sharp edge square to a plane, at a
/// distance r from the edge (revolution of the profile): (√k - √r)².
fn blend_u(k: f64, r: f64) -> f64 {
    let d = k.sqrt() - r.clamp(0.0, k).sqrt();
    d * d
}

fn corner_signs(k: u32) -> DVec3 {
    let s = |bit: u32| if k & bit == 0 { 1.0 } else { -1.0 };
    DVec3::new(s(1), s(2), s(4))
}

fn corner_of(sg: DVec3) -> u32 {
    (sg.x < 0.0) as u32 | ((sg.y < 0.0) as u32) << 1 | ((sg.z < 0.0) as u32) << 2
}

/// The three edges meeting at the corner of signs `sg`, by axis.
fn corner_edges(sg: DVec3) -> [usize; 3] {
    [edge_index(0, sg.y, sg.z), edge_index(1, sg.z, sg.x), edge_index(2, sg.x, sg.y)]
}

/// Radius of a box corner rounded as a ball (its three edges rounded alike).
/// Radius of a cylinder's rounded rim (0 top, 1 bottom), local units, as
/// the distance function clamps it; 0 when sharp.
fn cyl_rim(pr: &PrimDef, rim: usize) -> f64 {
    let r = edge_r(pr, rim);
    if pr.kind != 1 || r <= 0.0 {
        0.0
    } else {
        r.min(pr.size.x).min(pr.size.y)
    }
}

fn corner_r(pr: &PrimDef, k: u32) -> Option<f64> {
    if pr.kind != 0 {
        return None;
    }
    let r = corner_edges(corner_signs(k)).map(|e| edge_r(pr, e));
    (r[0] > 0.0 && (r[1] - r[0]).abs() < 1e-6 && (r[2] - r[0]).abs() < 1e-6).then_some(r[0])
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
struct FaceId {
    prim: u32,
    face: u32,
}

/// One step of the history: a box placed in the world.
struct Shape<'a> {
    pr: &'a PrimDef,
    inst: Instance,
    prim: u32,
    step: usize,
    carve: bool,
    /// Junction fillet with the model before it (smooth Add), world units.
    k: f64,
}

/// One piece of a junction fillet: a strip along one wall of the shape
/// added, or the corner around one of its edges.
#[derive(Clone, Copy)]
struct BlendPart {
    /// The shape added with the fillet (its step).
    shape: usize,
    /// The face it stands on (earlier shape).
    a: FaceId,
    kind: BlendKind,
}

#[derive(Clone, Copy)]
enum BlendKind {
    /// Along the wall `face` of the shape added.
    Strip { face: u32 },
    /// Around the edge between walls `faces`, foot `c` on the plane, along `axis`.
    Corner { faces: [u32; 2], c: DVec3, axis: DVec3 },
}

impl Shape<'_> {
    fn local(&self, p: DVec3) -> DVec3 {
        self.inst.to_local.transform_point3(p)
    }
    fn d(&self, p: DVec3) -> f64 {
        prim_raw(self.pr, self.local(p)) * self.pr.sc
    }
    /// Face of the shape under `p` (a point on its surface).
    fn face_at(&self, p: DVec3) -> u32 {
        self.face_at_skip(p, u32::MAX)
    }
    /// Same, leaving face `skip` out (a face flush with the chart asking).
    fn face_at_skip(&self, p: DVec3, skip: u32) -> u32 {
        let l = self.local(p);
        let mut best = (f64::INFINITY, 0u32);
        if self.pr.kind == 1 {
            // (the rounded rims first: where the point is past both rails)
            for rim in 0..2u32 {
                let r = cyl_rim(self.pr, rim as usize);
                let sg = if rim == 0 { 1.0 } else { -1.0 };
                if r > 0.0 && CYL_RIM0 + rim != skip && l.y * sg >= self.pr.size.y - r && DVec2::new(l.x, l.z).length() >= self.pr.size.x - r {
                    return CYL_RIM0 + rim;
                }
            }
            for f in [CYL_SIDE, CYL_TOP, CYL_BOTTOM] {
                if f == skip {
                    continue;
                }
                let v = self.face_fn_local(f, l).abs();
                if v < best.0 {
                    best = (v, f);
                }
            }
            return best.1;
        }
        let s = self.pr.size;
        for k in 0..8u32 {
            if let Some(r) = corner_r(self.pr, k) {
                let sg = corner_signs(k);
                if (0..3).all(|i| l[i] * sg[i] >= s[i] - r) {
                    return CORNER0 + k;
                }
            }
        }
        for f in 0..6u32 {
            if f == skip {
                continue;
            }
            let v = self.face_fn_local(f, l).abs();
            if v < best.0 {
                best = (v, f);
            }
        }
        for e in 0..12usize {
            let r = edge_r(self.pr, e);
            if r <= 0.0 {
                continue;
            }
            let (ax, sb, sc) = box_edge_signs(e);
            let (b, c) = ((ax + 1) % 3, (ax + 2) % 3);
            if l[b] * sb >= s[b] - r && l[c] * sc >= s[c] - r {
                let v = self.face_fn_local(FILLET0 + e as u32, l).abs();
                if v < best.0 {
                    best = (v, FILLET0 + e as u32);
                }
            }
        }
        best.1
    }
    /// Signed distance (local units) to the surface carrying face `f`.
    fn face_fn_local(&self, f: u32, l: DVec3) -> f64 {
        let s = self.pr.size;
        if self.pr.kind == 1 {
            return match f {
                CYL_SIDE | CYL_SIDE_B => DVec2::new(l.x, l.z).length() - s.x,
                CYL_TOP => l.y - s.y,
                CYL_RIM0.. => {
                    // the tube of a rounded rim (a torus)
                    let rim = ((f - CYL_RIM0) % 2) as usize;
                    let r = cyl_rim(self.pr, rim);
                    let sg = if rim == 0 { 1.0 } else { -1.0 };
                    DVec2::new(DVec2::new(l.x, l.z).length() - (s.x - r), l.y * sg - (s.y - r)).length() - r
                }
                _ => -l.y - s.y,
            };
        }
        if f >= CORNER0 {
            let sg = corner_signs(f - CORNER0);
            let r = corner_r(self.pr, f - CORNER0).unwrap_or(0.0);
            return (l - (s - DVec3::splat(r)) * sg).length() - r;
        }
        if f < FILLET0 {
            let ax = (f / 2) as usize;
            let sg = if f % 2 == 0 { 1.0 } else { -1.0 };
            l[ax] * sg - s[ax]
        } else {
            let e = (f - FILLET0) as usize;
            let r = edge_r(self.pr, e);
            let (ax, sb, sc) = box_edge_signs(e);
            let (b, c) = ((ax + 1) % 3, (ax + 2) % 3);
            DVec2::new(l[b] - sb * (s[b] - r), l[c] - sc * (s[c] - r)).length() - r
        }
    }
    fn face_fn(&self, f: u32, p: DVec3) -> f64 {
        self.face_fn_local(f, self.local(p)) * self.pr.sc
    }
    /// Outward normal of a flat face (box side, cylinder cap).
    fn flat_normal(&self, f: u32) -> Option<DVec3> {
        let (ax, sg) = if self.pr.kind == 1 {
            match f {
                CYL_TOP => (1, 1.0),
                CYL_BOTTOM => (1, -1.0),
                _ => return None,
            }
        } else if f < 6 {
            ((f / 2) as usize, if f % 2 == 0 { 1.0 } else { -1.0 })
        } else {
            return None;
        };
        Some(self.inst.to_world.transform_vector3(unit(ax) * sg).normalize())
    }
    /// A flat face of the shape on the plane through `p` square to `n`
    /// (flush with it): the face, and whether it faces along `n`.
    fn flat_on(&self, p: DVec3, n: DVec3, tol: f64) -> Option<(u32, bool)> {
        let faces: &[u32] = if self.pr.kind == 1 { &[CYL_TOP, CYL_BOTTOM] } else { &[0, 1, 2, 3, 4, 5] };
        faces.iter().find_map(|&f| {
            let m = self.flat_normal(f)?;
            (m.dot(n).abs() > 1.0 - 1e-9 && self.face_fn(f, p).abs() < tol).then_some((f, m.dot(n) > 0.0))
        })
    }
}

fn edge_index(axis: usize, s1: f64, s2: f64) -> usize {
    axis * 4
        + match (s1 >= 0.0, s2 >= 0.0) {
            (true, true) => 0,
            (false, true) => 1,
            (false, false) => 2,
            (true, false) => 3,
        }
}

fn unit(ax: usize) -> DVec3 {
    let mut v = DVec3::ZERO;
    v[ax] = 1.0;
    v
}

fn perp(v: DVec2) -> DVec2 {
    DVec2::new(-v.y, v.x)
}

/// Where a ray from `o` along `d` enters the convex quad `poly`, through a
/// side that is a line (`lines`) and on into its inside (not grazing along
/// a side or a corner, `margin`); None from inside the quad or on it.
fn enter_poly(poly: &[DVec2; 4], lines: &[bool; 4], o: DVec2, d: DVec2, margin: f64) -> Option<f64> {
    let area: f64 = (0..4).map(|i| poly[i].perp_dot(poly[(i + 1) % 4])).sum();
    let n: Vec<DVec2> = (0..4).map(|i| perp(poly[(i + 1) % 4] - poly[i]).normalize_or_zero() * area.signum()).collect();
    if (0..4).all(|i| (o - poly[i]).dot(n[i]) > -margin) {
        return None;
    }
    let clip = |m: f64| -> Option<(f64, f64, Option<usize>)> {
        let (mut lo, mut hi, mut k) = (f64::NEG_INFINITY, f64::INFINITY, None);
        for i in 0..4 {
            let num = (o - poly[i]).dot(n[i]) - m;
            let den = d.dot(n[i]);
            if den.abs() < 1e-12 {
                if num < 0.0 {
                    return None;
                }
            } else if den > 0.0 {
                if -num / den > lo {
                    lo = -num / den;
                    k = Some(i);
                }
            } else {
                hi = hi.min(-num / den);
            }
        }
        Some((lo, hi, k))
    };
    let (lo, hi, _) = clip(margin)?;
    if lo >= hi || hi <= 0.0 {
        return None;
    }
    let (t, _, k) = clip(0.0)?;
    (k.is_some_and(|k| lines[k]) && t > 0.0).then_some(t)
}

/// Whether a ray from `o` along `d` starts on a side of the convex quad
/// `poly` that is a line and goes into it (a loop from a node on the side of
/// a reserved area: it ends there).
fn starts_into(poly: &[DVec2; 4], lines: &[bool; 4], o: DVec2, d: DVec2, margin: f64) -> bool {
    let area: f64 = (0..4).map(|i| poly[i].perp_dot(poly[(i + 1) % 4])).sum();
    let n: Vec<DVec2> = (0..4).map(|i| perp(poly[(i + 1) % 4] - poly[i]).normalize_or_zero() * area.signum()).collect();
    (0..4).any(|i| {
        lines[i]
            && (o - poly[i]).dot(n[i]).abs() < margin
            && d.dot(n[i]) > 0.1
            && (0..4).filter(|&j| j != i).all(|j| (o - poly[j]).dot(n[j]) > margin)
    })
}

/// Where a ray from `o` along `d` first meets the boundary of chart `c`:
/// (edge, point).
fn boundary_hit(lay: &Layout, c: usize, o: DVec2, d: DVec2) -> Option<(usize, DVec3)> {
    let mut best: Option<(f64, usize, usize, f64)> = None;
    let eps = lay.node_tol * 10.0;
    for lp in &lay.bnd[c] {
        for &(e, _) in lp {
            let p2 = lay.pts2(e, c);
            for i in 0..p2.len() - 1 {
                let (p, q) = (p2[i], p2[i + 1]);
                let r = q - p;
                let den = d.perp_dot(r);
                if den.abs() < 1e-14 {
                    continue;
                }
                let s = (p - o).perp_dot(r) / den;
                let w = (p - o).perp_dot(d) / den;
                if s > eps && (-1e-9..=1.0 + 1e-9).contains(&w) && best.is_none_or(|b| s < b.0) {
                    best = Some((s, e, i, w));
                }
            }
        }
    }
    let (_, e, i, w) = best?;
    let ed = &lay.edges[e];
    Some((e, ed.pts[i].lerp(ed.pts[i + 1], w.clamp(0.0, 1.0))))
}

/// An end of boundary edge `e` within `near` of `x`.
fn boundary_end(lay: &Layout, e: usize, x: DVec3, near: f64) -> Option<usize> {
    let ed = &lay.edges[e];
    let (da, db) = (x.distance(lay.nodes[ed.a]), x.distance(lay.nodes[ed.b]));
    if da < near && da <= db {
        Some(ed.a)
    } else if db < near {
        Some(ed.b)
    } else {
        None
    }
}

/// Node of the boundary at `x` on edge `e`: an end of it within `near`, or
/// a new one there.
fn boundary_node(lay: &mut Layout, e: usize, x: DVec3, near: f64) -> usize {
    boundary_end(lay, e, x, near).unwrap_or_else(|| lay.split(e, x))
}

/// Signed turn from direction `a` to `b` (left positive).
fn turn(a: DVec2, b: DVec2) -> f64 {
    a.perp_dot(b).atan2(a.dot(b))
}

/// Flat chart (world mm in the face plane) or strip chart (along the edge,
/// arc length across).
#[derive(Clone, Copy)]
enum Map {
    Plane { o: DVec3, u: DVec3, v: DVec3 },
    Cyl { o: DVec3, a: DVec3, b: DVec3, c: DVec3, r: f64 },
    /// Ball corner: gnomonic chart around direction `d` (great circles are
    /// straight lines, the corner's three arcs a triangle).
    Sph { c: DVec3, d: DVec3, e1: DVec3, e2: DVec3, r: f64 },
    /// Junction fillet along a straight line where two square planes meet:
    /// along `e` × arc length across, from the rail on plane `na` (y = 0)
    /// to the rail on plane `nb`; `o` on the line where the planes meet.
    Blend { o: DVec3, e: DVec3, na: DVec3, nb: DVec3, k: f64 },
    /// Junction fillet around a sharp edge square to a plane: the plane's
    /// coordinates from the edge's foot `c`, height (along `a`) from the
    /// distance to the edge; the edge meets the fillet at the centre.
    Rev { c: DVec3, a: DVec3, e1: DVec3, e2: DVec3, k: f64 },
    /// Junction fillet around a cylinder standing on a plane: the profile
    /// of `Blend` turned around the axis `a` (foot `c` on the plane, radius
    /// `r`); around (angle × `rr` from direction `b` toward `e`) × arc length
    /// across, from the rail on the plane (y = 0) to the rail on the wall.
    Ring { c: DVec3, a: DVec3, b: DVec3, e: DVec3, r: f64, rr: f64, k: f64 },
    /// Rounded rim of a cylinder (a quarter of a torus): around (angle ×
    /// `rr` from direction `b` toward `e`) × arc length across, from the
    /// rail on the wall (y = 0) to the rail on the cap; `c` the centre of the
    /// tube's circle on the axis, `a` the axis out of the cap, `rm` the
    /// radius of the tube's circle, `r` the tube's radius.
    Tor { c: DVec3, a: DVec3, b: DVec3, e: DVec3, rm: f64, r: f64, rr: f64 },
}

impl Map {
    fn to3(&self, q: DVec2) -> DVec3 {
        match *self {
            Map::Plane { o, u, v } => o + u * q.x + v * q.y,
            Map::Cyl { o, a, b, c, r } => {
                let th = q.y / r;
                o + a * q.x + (b * th.cos() + c * th.sin()) * r
            }
            Map::Sph { c, d, e1, e2, r } => c + (d + e1 * (q.x / r) + e2 * (q.y / r)).normalize() * r,
            Map::Blend { o, e, na, nb, k } => {
                let t = blend_t(q.y / k);
                o + e * q.x + na * (k * t * t) + nb * (k * (1.0 - t) * (1.0 - t))
            }
            Map::Rev { c, a, e1, e2, k } => c + e1 * q.x + e2 * q.y + a * blend_u(k, q.length()),
            Map::Ring { c, a, b, e, r, rr, k } => {
                let th = q.x / rr;
                let t = blend_t(q.y / k);
                c + (b * th.cos() + e * th.sin()) * (r + k * (1.0 - t) * (1.0 - t)) + a * (k * t * t)
            }
            Map::Tor { c, a, b, e, rm, r, rr } => {
                let (th, ph) = (q.x / rr, q.y / r);
                c + (b * th.cos() + e * th.sin()) * (rm + r * ph.cos()) + a * (r * ph.sin())
            }
        }
    }
    fn to2(&self, p: DVec3) -> DVec2 {
        match *self {
            Map::Plane { o, u, v } => {
                let d = p - o;
                DVec2::new(d.dot(u), d.dot(v))
            }
            Map::Cyl { o, a, b, c, r } => {
                let d = p - o;
                DVec2::new(d.dot(a), d.dot(c).atan2(d.dot(b)) * r)
            }
            Map::Sph { c, d, e1, e2, r } => {
                let n = p - c;
                let w = n.dot(d).max(r * 1e-6);
                DVec2::new(n.dot(e1), n.dot(e2)) * (r / w)
            }
            Map::Blend { o, e, na, nb, k } => {
                let d = p - o;
                let (su, sv) = (d.dot(na).max(0.0).sqrt(), d.dot(nb).max(0.0).sqrt());
                let t = if su + sv > 1e-300 { su / (su + sv) } else { 0.5 };
                DVec2::new(d.dot(e), k * blend_s(t))
            }
            Map::Rev { c, e1, e2, .. } => {
                let d = p - c;
                DVec2::new(d.dot(e1), d.dot(e2))
            }
            Map::Ring { c, a, b, e, r, rr, k } => {
                let d = p - c;
                let h = d.dot(a);
                let w = d - a * h;
                let (su, sv) = (h.max(0.0).sqrt(), (w.length() - r).max(0.0).sqrt());
                let t = if su + sv > 1e-300 { su / (su + sv) } else { 0.5 };
                DVec2::new(w.dot(e).atan2(w.dot(b)) * rr, k * blend_s(t))
            }
            Map::Tor { c, a, b, e, rm, r, rr } => {
                let d = p - c;
                let h = d.dot(a);
                let w = d - a * h;
                DVec2::new(w.dot(e).atan2(w.dot(b)) * rr, h.atan2(w.length() - rm) * r)
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Cons {
    /// Side of the chart rectangle: 0 bottom, 1 right, 2 top, 3 left.
    Rect(usize),
    /// Rounded corner of a flat face (edge square to the face).
    Arc(usize),
    /// Round cap of a cylinder (disc around the chart origin).
    Disc,
    /// Ball corner: stays on the corner's side of the plane square to axis i.
    Oct(usize),
    /// End of a fillet strip where the next edge of the corner is rounded
    /// alike and the third is sharp: the two strips meet on the plane
    /// `so * l[o] - sa * l[ax] = d` (local), this strip on the positive
    /// side; `face` the other strip.
    Miter { ax: usize, sa: f64, o: usize, so: f64, d: f64, face: u32 },
    /// Must lie outside (Add) or inside (Carve) the model before the step.
    Prev,
    /// Must lie outside a later shape.
    Later(usize),
}

struct Chart {
    id: FaceId,
    shape: usize,
    map: Map,
    lo: DVec2,
    hi: DVec2,
    side: [FaceId; 4],
    /// (centre, radius, quadrant signs, rounded edge face)
    arcs: Vec<(DVec2, f64, DVec2, FaceId)>,
    /// Strip: radius and rows across.
    strip: Option<(f64, usize)>,
    /// Cylinder wall: length of a row around (chord tolerance).
    ring: Option<f64>,
    /// Cylinder cap: radius and the wall around it.
    disc: Option<(f64, FaceId)>,
    /// Ball corner: its three axes (world, pointing out) and the rounded
    /// edge across each of its arcs.
    ball: Option<([DVec3; 3], [FaceId; 3])>,
    cons: Vec<Cons>,
}

impl Chart {
    fn row(&self) -> f64 {
        match (self.strip, self.ring) {
            (Some((r, segs)), _) => r * FRAC_PI_2 / segs as f64,
            (None, Some(row)) => row,
            _ => 0.0,
        }
    }
}

/// A line between two nodes, shared by two charts (or inside one: a loop).
#[derive(Clone)]
struct GEdge {
    a: usize,
    b: usize,
    pts: Vec<DVec3>,
    charts: [usize; 2],
    ray: bool,
    alive: bool,
}

struct Layout<'a> {
    field: &'a Field,
    shapes: Vec<Shape<'a>>,
    prev: Vec<Option<Field>>,
    charts: Vec<Chart>,
    nodes: Vec<DVec3>,
    edges: Vec<GEdge>,
    /// Per chart: closed loops of (edge, forward), region on the left.
    bnd: Vec<Vec<Vec<(usize, bool)>>>,
    /// Per chart: loop edges inside it.
    rays: Vec<Vec<usize>>,
    /// Per chart: loop nodes inside the face (diagonal hooks).
    inner: Vec<Vec<usize>>,
    origins: Vec<usize>,
    node_tol: f64,
    /// Cylinder walls cut in two halves: prim and the chart map of half A
    /// (half A is where |θ| ≤ π/2 in that map).
    walls: Vec<(u32, Map)>,
    /// Pieces of junction fillets, chart id face BLEND0 + index.
    blends: Vec<BlendPart>,
}

impl Layout<'_> {
    fn shape_of(&self, prim: u32) -> Option<&Shape<'_>> {
        self.shapes.iter().find(|s| s.prim == prim)
    }

    fn cval(&self, ch: &Chart, k: Cons, q: DVec2) -> f64 {
        match k {
            Cons::Rect(0) => q.y - ch.lo.y,
            Cons::Rect(1) => ch.hi.x - q.x,
            Cons::Rect(2) => ch.hi.y - q.y,
            Cons::Rect(_) => q.x - ch.lo.x,
            Cons::Arc(i) => {
                let (c, r, sg, _) = ch.arcs[i];
                let w = ((q - c) * sg).max(DVec2::ZERO);
                r - w.length()
            }
            Cons::Disc => ch.disc.map(|(r, _)| r - q.length()).unwrap_or(1e9),
            Cons::Oct(i) => match (ch.ball, ch.map) {
                (Some((ax, _)), Map::Sph { c, .. }) => (ch.map.to3(q) - c).dot(ax[i]),
                _ => 1e9,
            },
            Cons::Miter { ax, sa, o, so, d, .. } => {
                let sh = &self.shapes[ch.shape];
                let l = sh.local(ch.map.to3(q));
                (so * l[o] - sa * l[ax] - d) * sh.pr.sc
            }
            Cons::Prev => {
                // a face in contact with the part before (flush with one of
                // its faces, or lying on it) is not the face: the part's own
                // face keeps the surface they share
                let sh = &self.shapes[ch.shape];
                let d = self.prev[ch.shape].as_ref().map(|f| f.dist(ch.map.to3(q))).unwrap_or(1e9);
                if sh.carve {
                    -d - self.flush_tol()
                } else {
                    // (a junction fillet starts k away from the model)
                    d - sh.k - self.flush_tol()
                }
            }
            Cons::Later(j) => {
                // a later face flush with this one: an Add leaves it (seen
                // from the air side), a Carve takes it (from the material side)
                let sj = &self.shapes[j];
                let mut p = ch.map.to3(q);
                if let Some(n) = self.air(ch) {
                    p += n * if sj.carve { -self.flush_tol() } else { self.flush_tol() };
                }
                sj.d(p) - sj.k
            }
        }
    }

    /// Distance under which two flat faces are flush (same plane).
    fn flush_tol(&self) -> f64 {
        self.node_tol * 0.05
    }

    /// Normal of a flat chart toward the air of the part.
    fn air(&self, ch: &Chart) -> Option<DVec3> {
        if !matches!(ch.map, Map::Plane { .. }) {
            return None;
        }
        let sh = &self.shapes[ch.shape];
        let n = sh.flat_normal(ch.id.face)?;
        Some(if sh.carve { -n } else { n })
    }

    /// Smallest constraint (the region is where it is positive).
    fn fmin(&self, ch: &Chart, q: DVec2) -> (f64, Cons) {
        let mut best = (f64::INFINITY, Cons::Rect(0));
        for &k in &ch.cons {
            let v = self.cval(ch, k, q);
            if v < best.0 {
                best = (v, k);
            }
        }
        best
    }

    /// Which half of a cut cylinder wall a point on it belongs to.
    fn half(&self, f: FaceId, p: DVec3) -> FaceId {
        let b = match f.face {
            CYL_SIDE => CYL_SIDE_B,
            CYL_RIM0 | CYL_RIM1 if self.shape_of(f.prim).is_some_and(|s| s.pr.kind == 1) => CYL_RIM_B0 + (f.face - CYL_RIM0),
            _ => return f,
        };
        match self.walls.iter().find(|w| w.0 == f.prim) {
            Some(&(_, m @ Map::Cyl { r, .. })) if m.to2(p).y.abs() > FRAC_PI_2 * r => FaceId { prim: f.prim, face: b },
            _ => f,
        }
    }

    fn label(&self, ch: &Chart, k: Cons, q: DVec2) -> FaceId {
        let f = self.label_raw(ch, k, q);
        match k {
            Cons::Rect(_) | Cons::Arc(_) | Cons::Oct(_) | Cons::Miter { .. } => f,
            _ => self.half(f, ch.map.to3(q)),
        }
    }

    fn label_raw(&self, ch: &Chart, k: Cons, q: DVec2) -> FaceId {
        let tol = self.flush_tol();
        match k {
            Cons::Rect(s) => {
                // a later Add flush with this face, standing on this side:
                // its face continues this one across the side
                if let Some(n) = self.air(ch) {
                    let p = ch.map.to3(q);
                    for &c in &ch.cons {
                        let Cons::Later(j) = c else { continue };
                        let sj = &self.shapes[j];
                        if !sj.carve && sj.k == 0.0 && sj.d(p).abs() < tol {
                            if let Some((g, true)) = sj.flat_on(p, n, tol) {
                                return FaceId { prim: sj.prim, face: g };
                            }
                        }
                    }
                }
                ch.side[s]
            }
            Cons::Arc(i) => ch.arcs[i].3,
            Cons::Disc => ch.disc.map(|d| d.1).unwrap_or(ch.id),
            Cons::Oct(i) => ch.ball.map(|b| b.1[i]).unwrap_or(ch.id),
            Cons::Miter { face, .. } => FaceId { prim: ch.id.prim, face },
            Cons::Prev => {
                let p = ch.map.to3(q);
                let me = &self.shapes[ch.shape];
                let s = self.shapes.iter().filter(|s| s.step < me.step).min_by(|a, b| a.d(p).abs().total_cmp(&b.d(p).abs())).unwrap();
                let mut f = FaceId { prim: s.prim, face: s.face_at(p) };
                // flush with a face of the part before: that face across
                // (same way) or the next one (face to face, in contact)
                if let Some(n) = self.air(ch) {
                    if let Some((g, same)) = s.flat_on(p, n, tol) {
                        f.face = if same { g } else { s.face_at_skip(p, g) };
                    }
                }
                // across the rail of a junction fillet: its strip
                if me.k > 0.0 {
                    if let Some(i) = self.blends.iter().position(|b| b.shape == ch.shape && b.a == f && matches!(b.kind, BlendKind::Strip { face } if face == ch.id.face)) {
                        return FaceId { prim: me.prim, face: BLEND0 + i as u32 };
                    }
                }
                f
            }
            Cons::Later(j) => {
                let p = ch.map.to3(q);
                if self.shapes[j].k > 0.0 {
                    if let Some(b) = self.blend_at(j, ch.id, p) {
                        return b;
                    }
                }
                let sj = &self.shapes[j];
                let mut f = sj.face_at(p);
                // a later face flush with this one is not across: its next face is
                if let Some((g, _)) = self.air(ch).and_then(|n| sj.flat_on(p, n, tol)) {
                    f = sj.face_at_skip(p, g);
                }
                FaceId { prim: sj.prim, face: f }
            }
        }
    }

    /// The face whose surface carries the line toward face `l` across: `l`
    /// itself, or for a face flush with this one (same plane), the face
    /// where that one ends.
    fn geom(&self, ch: &Chart, k: Cons, l: FaceId, q: DVec2) -> FaceId {
        if !matches!(k, Cons::Prev | Cons::Later(_)) || l.face >= BLEND0 {
            return l;
        }
        let p = ch.map.to3(q);
        if let (Some(n), Some(s)) = (self.air(ch), self.shape_of(l.prim)) {
            if let Some((g, _)) = s.flat_on(p, n, self.flush_tol()) {
                if g == l.face {
                    return FaceId { prim: l.prim, face: s.face_at_skip(p, g) };
                }
            }
        }
        l
    }

    /// The function whose zero set carries a piece of boundary (`g` from `geom`).
    fn psi(&self, ch: &Chart, k: Cons, g: FaceId, q: DVec2) -> f64 {
        match k {
            Cons::Rect(_) | Cons::Arc(_) | Cons::Disc | Cons::Oct(_) | Cons::Miter { .. } => self.cval(ch, k, q),
            _ => self.across(ch.id, g, ch.map.to3(q)),
        }
    }

    /// Function zero on the line between face `me` and face `other`, at
    /// `p` on `me`: the surface of `other`, or the rail of a junction fillet.
    fn across(&self, me: FaceId, other: FaceId, p: DVec3) -> f64 {
        if other.face >= BLEND0 {
            let Some(b) = self.blends.get((other.face - BLEND0) as usize) else { return 0.0 };
            let sh = &self.shapes[b.shape];
            if me == b.a {
                return match b.kind {
                    BlendKind::Strip { face } => sh.face_fn(face, p) - sh.k,
                    BlendKind::Corner { c, axis, .. } => {
                        let d = p - c;
                        (d - axis * d.dot(axis)).length() - sh.k
                    }
                };
            }
            if me.prim == sh.prim && me.face < BLEND0 {
                return self.shape_of(b.a.prim).map(|a| a.face_fn(b.a.face, p)).unwrap_or(0.0) - sh.k;
            }
            return 0.0;
        }
        self.shape_of(other.prim).map(|s| s.face_fn(other.face, p)).unwrap_or(0.0)
    }

    /// The piece of the junction fillet of shape `j` (added on face `a`)
    /// next to `p`: the strip of the wall it faces, or the corner of the
    /// edge it faces.
    fn blend_at(&self, j: usize, a: FaceId, p: DVec3) -> Option<FaceId> {
        let sh = &self.shapes[j];
        if sh.pr.kind == 1 {
            // around a cylinder: the half of the ring by the half of the wall
            let wall = self.half(FaceId { prim: sh.prim, face: CYL_SIDE }, p).face;
            let i = self.blends.iter().position(|b| b.shape == j && b.a == a && matches!(b.kind, BlendKind::Strip { face } if face == wall))?;
            return Some(FaceId { prim: sh.prim, face: BLEND0 + i as u32 });
        }
        let l = sh.local(p);
        let s = sh.pr.size;
        let out: Vec<u32> = (0..3).filter(|&i| l[i].abs() > s[i]).map(|i| (i * 2) as u32 + (l[i] < 0.0) as u32).collect();
        let i = self.blends.iter().position(|b| {
            b.shape == j
                && b.a == a
                && match b.kind {
                    BlendKind::Strip { face } => out == [face],
                    BlendKind::Corner { faces, .. } => out.len() == 2 && faces.contains(&out[0]) && faces.contains(&out[1]),
                }
        })?;
        Some(FaceId { prim: sh.prim, face: BLEND0 + i as u32 })
    }

    fn node(&mut self, p: DVec3) -> usize {
        for (i, n) in self.nodes.iter().enumerate() {
            if n.distance(p) < self.node_tol {
                return i;
            }
        }
        self.nodes.push(p);
        self.nodes.len() - 1
    }

    fn pts2(&self, e: usize, c: usize) -> Vec<DVec2> {
        let m = self.charts[c].map;
        self.edges[e].pts.iter().map(|p| m.to2(*p)).collect()
    }

    /// Polyline of a half-edge in chart `c`, from its start.
    fn hpts(&self, e: usize, fwd: bool, c: usize) -> Vec<DVec2> {
        let mut p = self.pts2(e, c);
        if !fwd {
            p.reverse();
        }
        p
    }

    fn start_dir(&self, e: usize, fwd: bool, c: usize) -> DVec2 {
        let p = self.hpts(e, fwd, c);
        let k = p.len().min(3) - 1;
        (p[k.max(1).min(p.len() - 1)] - p[0]).normalize_or_zero()
    }

    fn end_dir(&self, e: usize, fwd: bool, c: usize) -> DVec2 {
        let p = self.hpts(e, fwd, c);
        let n = p.len();
        let k = n.saturating_sub(3).min(n - 2);
        (p[n - 1] - p[k]).normalize_or_zero()
    }

    /// Exact direction of the line `e` (a junction, not a loop) at its start
    /// or end in chart `c`: square to the gradient of the surface across.
    fn tangent(&self, e: usize, fwd: bool, c: usize, at_end: bool) -> DVec2 {
        let ed = &self.edges[e];
        let rough = if at_end { self.end_dir(e, fwd, c) } else { self.start_dir(e, fwd, c) };
        let Some(o) = ed.charts.iter().copied().find(|&x| x != c) else { return rough };
        let id = self.charts[o].id;
        let me = self.charts[c].id;
        let map = self.charts[c].map;
        let node = if fwd != at_end { ed.a } else { ed.b };
        let q = map.to2(self.nodes[node]);
        // a boundary of the chart's own shape (rail of a fillet, edge of a
        // ball corner, miter) is a level of the chart's own constraint: the
        // surface across meets this one tangentially there, its distance
        // has no usable gradient on this face
        let ch = &self.charts[c];
        let p = self.hpts(e, fwd, c);
        let sample = if at_end { (p[p.len() - 2] + p[p.len() - 1]) * 0.5 } else { (p[0] + p[1]) * 0.5 };
        let own = self.fmin(ch, sample).1;
        let f = |x: DVec2| match own {
            Cons::Rect(_) | Cons::Arc(_) | Cons::Disc | Cons::Oct(_) | Cons::Miter { .. } => self.cval(ch, own, x),
            _ => self.across(me, id, map.to3(x)),
        };
        let t = perp(grad2(&f, q, self.node_tol * 5.0)).normalize_or_zero();
        if t == DVec2::ZERO {
            rough
        } else if t.dot(rough) < 0.0 {
            -t
        } else {
            t
        }
    }

    /// The straight piece of chart `c`'s boundary that meets node `v`,
    /// other than `e`: its direction in the chart, away from `v`.
    fn straight_at(&self, c: usize, e: usize, v: usize) -> Option<DVec2> {
        for lp in &self.bnd[c] {
            for &(f, _) in lp {
                let ed = &self.edges[f];
                if f == e || !ed.alive || (ed.a != v && ed.b != v) {
                    continue;
                }
                let p = self.pts2(f, c);
                let (pa, pb) = (p[0], p[p.len() - 1]);
                let t = (pb - pa).normalize_or_zero();
                if t == DVec2::ZERO || p.iter().any(|&x| (x - pa).perp_dot(t).abs() > self.node_tol * 10.0) {
                    return None;
                }
                return Some(if ed.a == v { t } else { -t });
            }
        }
        None
    }

    /// The boundary edge of chart `c` that runs through `x` (not at one of
    /// its ends).
    fn edge_through(&self, c: usize, x: DVec3) -> Option<usize> {
        let near = self.node_tol * 10.0;
        self.bnd[c].iter().flatten().map(|&(e, _)| e).find(|&e| {
            let ed = &self.edges[e];
            if !ed.alive || self.nodes[ed.a].distance(x) < near || self.nodes[ed.b].distance(x) < near {
                return false;
            }
            ed.pts.windows(2).any(|w| {
                let ab = w[1] - w[0];
                let t = ((x - w[0]).dot(ab) / ab.length_squared().max(1e-30)).clamp(0.0, 1.0);
                (w[0] + ab * t).distance(x) < near
            })
        })
    }

    /// Cuts made on one rail of a strip around a cylinder (its rounded rim,
    /// a junction fillet's ring), made again at the same angles on its
    /// other rail, where it shows.
    fn carry_across(&mut self, c: usize, cuts: &[DVec3]) {
        let sm = self.charts[c].map;
        let (lo, hi) = (self.charts[c].lo.y, self.charts[c].hi.y);
        for &x in cuts {
            let q = sm.to2(x);
            for y in [lo, hi] {
                if (q.y - y).abs() < (hi - lo) * 0.01 {
                    continue;
                }
                let w = sm.to3(DVec2::new(q.x, y));
                if let Some(e2) = self.edge_through(c, w) {
                    self.split(e2, w);
                }
            }
        }
    }

    /// Splits edge `e` at the 3D point `x` (on it); returns the node.
    fn split(&mut self, e: usize, x: DVec3) -> usize {
        let n = self.node(x);
        let ed = self.edges[e].clone();
        if n == ed.a || n == ed.b {
            return n;
        }
        // closest segment
        let mut best = (f64::INFINITY, 0usize);
        for i in 0..ed.pts.len() - 1 {
            let (a, b) = (ed.pts[i], ed.pts[i + 1]);
            let ab = b - a;
            let t = ((x - a).dot(ab) / ab.length_squared().max(1e-30)).clamp(0.0, 1.0);
            let d = (a + ab * t).distance(x);
            if d < best.0 {
                best = (d, i);
            }
        }
        let i = best.1;
        let mut p1: Vec<DVec3> = ed.pts[..=i].to_vec();
        p1.push(self.nodes[n]);
        let mut p2 = vec![self.nodes[n]];
        p2.extend_from_slice(&ed.pts[i + 1..]);
        // drop points that coincide with the new node
        if p1.len() > 2 && p1[p1.len() - 2].distance(self.nodes[n]) < self.node_tol {
            p1.remove(p1.len() - 2);
        }
        if p2.len() > 2 && p2[1].distance(self.nodes[n]) < self.node_tol {
            p2.remove(1);
        }
        let e1 = self.edges.len();
        self.edges.push(GEdge { a: ed.a, b: n, pts: p1, ..ed.clone() });
        let e2 = self.edges.len();
        self.edges.push(GEdge { a: n, b: ed.b, pts: p2, ..ed.clone() });
        self.edges[e].alive = false;
        let mut cs = vec![ed.charts[0]];
        if ed.charts[1] != ed.charts[0] {
            cs.push(ed.charts[1]);
        }
        for c in cs {
            for lp in &mut self.bnd[c] {
                if let Some(k) = lp.iter().position(|x| x.0 == e) {
                    let fwd = lp[k].1;
                    let rep = if fwd { [(e1, true), (e2, true)] } else { [(e2, false), (e1, false)] };
                    lp.splice(k..=k, rep);
                }
            }
            if let Some(k) = self.rays[c].iter().position(|&x| x == e) {
                self.rays[c].splice(k..=k, [e1, e2]);
            }
        }
        n
    }
}

/// Gradient of a 2D function by central differences.
fn grad2(f: &dyn Fn(DVec2) -> f64, q: DVec2, e: f64) -> DVec2 {
    DVec2::new(f(q + DVec2::new(e, 0.0)) - f(q - DVec2::new(e, 0.0)), f(q + DVec2::new(0.0, e)) - f(q - DVec2::new(0.0, e))) / (2.0 * e)
}

/// Newton projection on the zero set of `f`.
fn proj1(f: &dyn Fn(DVec2) -> f64, mut q: DVec2, e: f64) -> DVec2 {
    for _ in 0..40 {
        let v = f(q);
        if v.abs() < 1e-12 {
            break;
        }
        let g = grad2(f, q, e);
        let gg = g.length_squared();
        if gg < 1e-20 {
            break;
        }
        q -= g * (v / gg);
    }
    q
}

/// Where two curves cross, when they cross clearly (not tangent).
fn solve_cross(f: &dyn Fn(DVec2) -> f64, g: &dyn Fn(DVec2) -> f64, q0: DVec2, e: f64) -> Option<DVec2> {
    let mut q = q0;
    for _ in 0..50 {
        let (a, b) = (f(q), g(q));
        if a.abs() < 1e-11 && b.abs() < 1e-11 {
            return Some(q);
        }
        let ga = grad2(f, q, e);
        let gb = grad2(g, q, e);
        let det = ga.perp_dot(gb);
        if det.abs() < 0.1 * ga.length() * gb.length() {
            return None;
        }
        q += DVec2::new((-a * gb.y + b * ga.y) / det, (-b * ga.x + a * gb.x) / det);
    }
    (f(q).abs() < 1e-8 && g(q).abs() < 1e-8).then_some(q)
}

/// Closed boundary loops of {f > 0} on a grid (marching squares), region
/// on the left.
fn march(f: &dyn Fn(DVec2) -> f64, lo: DVec2, hi: DVec2, step: f64) -> Vec<Vec<DVec2>> {
    let size = hi - lo;
    let nx = ((size.x / step).ceil() as usize).clamp(2, 1500) + 3;
    let ny = ((size.y / step).ceil() as usize).clamp(2, 1500) + 3;
    let dx = size.x / (nx - 3) as f64;
    let dy = size.y / (ny - 3) as f64;
    let g0 = lo - DVec2::new(1.5 * dx, 1.5 * dy);
    let at = |i: usize, j: usize| g0 + DVec2::new(i as f64 * dx, j as f64 * dy);
    let mut val = vec![0.0f64; (nx + 1) * (ny + 1)];
    for j in 0..=ny {
        for i in 0..=nx {
            val[j * (nx + 1) + i] = f(at(i, j));
        }
    }
    let v = |i: usize, j: usize| val[j * (nx + 1) + i];
    let inside = |i: usize, j: usize| v(i, j) > 0.0;
    // edge ids: horizontal (i,j)-(i+1,j) = 2*(j*(nx+1)+i), vertical (i,j)-(i,j+1) = +1
    let hid = |i: usize, j: usize| 2 * (j * (nx + 1) + i);
    let vid = |i: usize, j: usize| 2 * (j * (nx + 1) + i) + 1;
    let mut next: HashMap<usize, usize> = HashMap::new();
    for j in 0..ny {
        for i in 0..nx {
            let c = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
            let ins: Vec<bool> = c.iter().map(|&(a, b)| inside(a, b)).collect();
            let eid = [hid(i, j), vid(i + 1, j), hid(i, j + 1), vid(i, j)];
            let exits: Vec<usize> = (0..4).filter(|&k| ins[k] && !ins[(k + 1) % 4]).collect();
            let entries: Vec<usize> = (0..4).filter(|&k| !ins[k] && ins[(k + 1) % 4]).collect();
            if exits.len() == 1 {
                next.insert(eid[exits[0]], eid[entries[0]]);
            } else if exits.len() == 2 {
                let centre = (v(i, j) + v(i + 1, j) + v(i + 1, j + 1) + v(i, j + 1)) * 0.25 > 0.0;
                for &x in &exits {
                    // centre inside: cut the outside corner after the exit
                    let en = if centre { (x + 1) % 4 } else { (x + 3) % 4 };
                    next.insert(eid[x], eid[en]);
                }
            }
        }
    }
    let point = |id: usize| -> DVec2 {
        let k = id / 2;
        let (i, j) = (k % (nx + 1), k / (nx + 1));
        let (a, b) = if id % 2 == 0 { ((i, j), (i + 1, j)) } else { ((i, j), (i, j + 1)) };
        let (pa, pb) = (at(a.0, a.1), at(b.0, b.1));
        let (mut lo_t, mut hi_t) = (0.0, 1.0);
        let a_in = inside(a.0, a.1);
        for _ in 0..40 {
            let m = (lo_t + hi_t) * 0.5;
            if (f(pa.lerp(pb, m)) > 0.0) == a_in {
                lo_t = m;
            } else {
                hi_t = m;
            }
        }
        pa.lerp(pb, (lo_t + hi_t) * 0.5)
    };
    let mut out = vec![];
    let mut used: HashMap<usize, bool> = HashMap::new();
    let mut keys: Vec<usize> = next.keys().copied().collect();
    keys.sort();
    for s in keys {
        if used.contains_key(&s) {
            continue;
        }
        let mut lp = vec![];
        let mut cur = s;
        loop {
            used.insert(cur, true);
            lp.push(point(cur));
            match next.get(&cur) {
                Some(&n) if n == s => break,
                Some(&n) if !used.contains_key(&n) => cur = n,
                _ => {
                    lp.clear();
                    break;
                }
            }
        }
        if lp.len() >= 3 {
            out.push(lp);
        }
    }
    out
}

/// Settings of the line bake.
#[derive(Clone, Copy, Debug)]
pub struct LayoutSettings {
    /// Target edge length (mm).
    pub size: f64,
    /// Chord tolerance on fillets (mm).
    pub tol: f64,
    pub threads: usize,
    /// Concave fillets hook into the face's grid by a diagonal (Romain's
    /// choice) instead of running their rows straight across the face.
    pub diagonal: bool,
}

/// Rows across a fillet of radius `r` and opening `angle`, even, at least 2.
fn fillet_segs(r: f64, angle: f64, tol: f64) -> usize {
    let step = 2.0 * (1.0 - (tol / r).min(0.9)).acos();
    let n = ((angle / step.max(1e-3)).ceil() as usize).max(2);
    n + n % 2
}

/// Whether the history is one this bake handles (Err: why not).
pub fn handles(scene: &Scene) -> Result<(), String> {
    let field = Field::new(&compile(scene));
    let steps: Vec<Step> = field.steps().ok_or("historique en arbre")?;
    if steps.is_empty() {
        return Err("scène vide".into());
    }
    check(&field, &steps)
}

fn check(field: &Field, steps: &[Step]) -> Result<(), String> {
    for (i, s) in steps.iter().enumerate() {
        let pr = &field.prims[s.prim as usize];
        if pr.kind > 1 || pr.taper.abs() > 1e-6 || pr.shell > 0.0 {
            return Err("seulement des boîtes et des cylindres pour l'instant".into());
        }
        // a cylinder's rounded rims: a flat cap and a straight wall must remain
        if pr.kind == 1 {
            let (r0, r1) = (cyl_rim(pr, 0), cyl_rim(pr, 1));
            if r0.max(r1) > pr.size.x * (1.0 - 1e-6) || r0 + r1 > 2.0 * pr.size.y * (1.0 - 1e-6) {
                return Err("cylindre arrondi jusqu'au bout (sans dessus plat ou sans paroi droite) : pas encore géré".into());
            }
        }
        if field.instances(s.prim).len() != 1 {
            return Err("copies pas encore gérées".into());
        }
        if i > 0 && (s.mode > 1 || s.chamfer) {
            return Err("seulement Add / Carve pour l'instant".into());
        }
        if i > 0 && s.k > 1e-9 {
            if s.mode != 0 {
                return Err("fillet de jonction d'une forme creusée pas encore géré".into());
            }
            // (a cylinder's rounded rims: the profile check below says whether
            // they come near the fillet)
            if pr.kind == 0 && (pr.round > 0.0 || pr.chamfer > 0.0 || pr.edges.iter().any(|r| *r != 0.0)) {
                return Err("fillet de jonction : seulement une boîte à arêtes vives ou un cylindre posés sur une face, pour l'instant".into());
            }
        }
        if (0..12).any(|e| edge_r(pr, e) < 0.0) {
            return Err("chanfreins pas encore gérés".into());
        }
        if pr.kind == 0 && (0..8u32).any(|k| corner_edges(corner_signs(k)).iter().all(|&e| edge_r(pr, e) > 0.0) && corner_r(pr, k).is_none()) {
            return Err("coin où trois fillets de rayons différents se rejoignent, pas encore géré".into());
        }

    }
    Ok(())
}

/// Where a hook's square should have a side: a loop that came across its
/// rows on a pass (from another face, around an edge). Its chart, the
/// arc's middle, which side (0: offset along a1, 1: along a2) and how far
/// from the corner of the arc's tangents.
#[derive(Clone, Copy, Debug)]
struct Hint {
    chart: usize,
    m: DVec3,
    w: usize,
    c: f64,
}

/// Bake by construction lines.
pub fn build(scene: &Scene, st: &LayoutSettings, progress: &Progress) -> Result<Baked, String> {
    // a loop coming round from another face across a hook's rows (a notch
    // seen on the top and the bottom of a part, its squares placed apart):
    // once more, with that hook's square on the loop
    let mut hints: Vec<Hint> = vec![];
    loop {
        let mut found = vec![];
        let r = build_pass(scene, st, progress, &hints, &mut found);
        let new: Vec<Hint> = found
            .into_iter()
            .filter(|h| !hints.iter().any(|k| k.chart == h.chart && k.w == h.w && k.m.distance(h.m) < 1e-6))
            .collect();
        if std::env::var("DEBUG_LAYOUT").is_ok() {
            eprintln!("passe : indices {:?}", new);
        }
        if new.is_empty() || hints.len() > 32 {
            return r;
        }
        hints.extend(new);
    }
}

fn build_pass(scene: &Scene, st: &LayoutSettings, progress: &Progress, hints: &[Hint], found: &mut Vec<Hint>) -> Result<Baked, String> {
    let t0 = Instant::now();
    let field = Field::new(&compile(scene));
    let steps: Vec<Step> = field.steps().ok_or("historique en arbre")?;
    if steps.is_empty() {
        return Err("scène vide".into());
    }
    let (lo, hi) = scene.bounds();
    let extent = if lo.x <= hi.x { ((hi - lo).max_element() as f64).max(1.0) } else { 20.0 };
    check(&field, &steps)?;
    // shapes, one per step
    let mut shapes = vec![];
    for (i, s) in steps.iter().enumerate() {
        let inst = field.instances(s.prim);
        let k = if i > 0 && s.mode == 0 && !s.chamfer { s.k } else { 0.0 };
        shapes.push(Shape { pr: &field.prims[s.prim as usize], inst: inst[0], prim: s.prim, step: i, carve: i > 0 && s.mode == 1, k });
    }
    // a cylinder whose wall touches a flat face of a box along a line (a
    // hole or a boss tangent to the part's edge): a wall of no thickness
    // there, not yet
    for c in shapes.iter().filter(|s| s.pr.kind == 1) {
        let a = c.inst.to_world.transform_vector3(unit(1)).normalize();
        let o = c.inst.to_world.transform_point3(DVec3::ZERO);
        let (r, h) = (c.pr.size.x * c.pr.sc, c.pr.size.y * c.pr.sc);
        for b in shapes.iter().filter(|s| s.pr.kind != 1) {
            for f in 0..6u32 {
                let Some(n) = b.flat_normal(f) else { continue };
                let d = b.face_fn(f, o);
                if a.dot(n).abs() > 1e-6 || (d.abs() - r).abs() > extent * 2e-6 {
                    continue;
                }
                let touch = (0..=8).any(|i| {
                    let p = o - n * d + a * (h * (i as f64 / 4.0 - 1.0));
                    b.d(p).abs() < extent * 2e-6 && b.face_at(p) == f
                });
                if touch {
                    return Err("forme tangente à une face de la pièce (contact sur une ligne) : pas encore géré".into());
                }
            }
            // or standing on a face, its circle touching the rail of one of
            // the face's rounded edges (the flat part pinched to a point)
            let l = b.local(o);
            let al = b.inst.to_local.transform_vector3(a).normalize();
            let s = b.pr.size;
            let rl = r / b.pr.sc;
            for ax in 0..3usize {
                if al[ax].abs() < 1.0 - 1e-6 {
                    continue;
                }
                for sg in [1.0, -1.0] {
                    // (the cylinder reaches that face's plane)
                    if ((sg * s[ax] - l[ax]) * al[ax].signum()).abs() > h / b.pr.sc + 1e-9 {
                        continue;
                    }
                    for (b1, b2) in [((ax + 1) % 3, (ax + 2) % 3), ((ax + 2) % 3, (ax + 1) % 3)] {
                        for sb in [1.0, -1.0] {
                            let mut sv = DVec3::ZERO;
                            sv[ax] = sg;
                            sv[b1] = sb;
                            let re = edge_r(b.pr, edge_index(b2, sv[(b2 + 1) % 3], sv[(b2 + 2) % 3]));
                            if re <= 0.0 {
                                continue;
                            }
                            let rail = sb * (s[b1] - re);
                            if ((l[b1] - rail).abs() - rl).abs() < extent * 2e-6 / b.pr.sc && l[b2].abs() < s[b2] {
                                return Err("forme tangente à l'arrondi d'une arête (contact en un point) : pas encore géré".into());
                            }
                            // (its circle across the rail: into the rounded edge)
                            if (l[b1] - rail).abs() < rl && (l[b1] - sb * s[b1]).abs() > 1e-9 && l[b2].abs() < s[b2] - re {
                                return Err("forme qui entame l'arrondi d'une arête : pas encore géré".into());
                            }
                        }
                    }
                }
            }
        }
    }
    let prev: Vec<Option<Field>> = (0..shapes.len()).map(|i| (i > 0).then(|| field.prefix(&steps, i))).collect();
    let tol_f = (st.tol * 2.0).max(0.02);

    // charts: flat faces and rounded edges of every shape
    let mut charts: Vec<Chart> = vec![];
    for (si, sh) in shapes.iter().enumerate() {
        let pr = sh.pr;
        let s = pr.size;
        let sc = pr.sc;
        let tw = sh.inst.to_world;
        let rr = |e: usize| edge_r(pr, e).max(0.0);
        let mut cons_base = vec![Cons::Rect(0), Cons::Rect(1), Cons::Rect(2), Cons::Rect(3)];
        if si > 0 {
            cons_base.push(Cons::Prev);
        }
        for j in (si + 1)..shapes.len() {
            cons_base.push(Cons::Later(j));
        }
        if pr.kind == 1 {
            // cylinder: the wall (along the axis × around, seam placed later
            // where the wall does not show) and two round caps
            let (rad, h) = (s.x * sc, s.y * sc);
            let o = tw.transform_point3(DVec3::ZERO);
            let am = tw.transform_vector3(unit(1)).normalize();
            let bm = tw.transform_vector3(unit(0)).normalize();
            let cm = tw.transform_vector3(unit(2)).normalize();
            let wall = FaceId { prim: sh.prim, face: CYL_SIDE };
            let (top, bottom) = (FaceId { prim: sh.prim, face: CYL_TOP }, FaceId { prim: sh.prim, face: CYL_BOTTOM });
            // rounded rims (world radius) and the face next to the wall and
            // to the cap at each end: the rim's strip, or each other
            let rims = [cyl_rim(pr, 0) * sc, cyl_rim(pr, 1) * sc];
            let rim_id = |i: usize| FaceId { prim: sh.prim, face: CYL_RIM0 + i as u32 };
            let end = |i: usize, cap: FaceId| if rims[i] > 0.0 { rim_id(i) } else { cap };
            let step = 2.0 * (1.0 - (tol_f / rad).min(0.9)).acos();
            charts.push(Chart {
                id: wall,
                shape: si,
                map: Map::Cyl { o, a: am, b: bm, c: cm, r: rad },
                lo: DVec2::new(-h + rims[1], -PI * rad),
                hi: DVec2::new(h - rims[0], PI * rad),
                side: [wall, end(0, top), wall, end(1, bottom)],
                arcs: vec![],
                strip: None,
                ring: Some((rad * step).min(st.size)),
                disc: None,
                ball: None,
                cons: cons_base.clone(),
            });
            for (i, (sg, id)) in [(1.0, top), (-1.0, bottom)].into_iter().enumerate() {
                let w = rad * 1.02;
                let mut cons = cons_base.clone();
                cons.push(Cons::Disc);
                charts.push(Chart {
                    id,
                    shape: si,
                    map: Map::Plane { o: o + am * (sg * h), u: cm, v: bm },
                    lo: DVec2::splat(-w),
                    hi: DVec2::splat(w),
                    side: [wall; 4],
                    arcs: vec![],
                    strip: None,
                    ring: None,
                    disc: Some((rad - rims[i], end(i, wall))),
                    ball: None,
                    cons,
                });
                // the rim's strip, all around (cut open or in two halves
                // with the wall, below)
                let r = rims[i];
                if r > 0.0 {
                    let rm = rad - r;
                    let rr = rm + r * 0.64;
                    let segs = fillet_segs(r, FRAC_PI_2, tol_f).max(((r * FRAC_PI_2 / st.size).round() as usize + 1) / 2 * 2);
                    // (around, both rails take the wall's segments: `ring` is
                    // the wall's row, in this chart's units)
                    let row = (rad * step).min(st.size) * rr / rad;
                    charts.push(Chart {
                        id: rim_id(i),
                        shape: si,
                        map: Map::Tor { c: o + am * (sg * (h - r)), a: am * sg, b: bm, e: cm, rm, r, rr },
                        lo: DVec2::new(-PI * rr, 0.0),
                        hi: DVec2::new(PI * rr, r * FRAC_PI_2),
                        side: [wall, rim_id(i), id, rim_id(i)],
                        arcs: vec![],
                        strip: Some((r, segs)),
                        ring: Some(row),
                        disc: None,
                        ball: None,
                        cons: cons_base.clone(),
                    });
                }
            }
            continue;
        }
        for a in 0..3usize {
            for sg in [1.0f64, -1.0] {
                let (b, c) = ((a + 1) % 3, (a + 2) % 3);
                let face = (a * 2 + (sg < 0.0) as usize) as u32;
                let o = tw.transform_point3(unit(a) * (sg * s[a]));
                let u = tw.transform_vector3(unit(b)).normalize();
                let v = tw.transform_vector3(unit(c)).normalize();
                // edge parallel to `along`, on side `so` of axis `other`, on this face
                let side_edge = |along: usize, other: usize, so: f64| -> usize {
                    let mut sv = DVec3::ZERO;
                    sv[a] = sg;
                    sv[other] = so;
                    edge_index(along, sv[(along + 1) % 3], sv[(along + 2) % 3])
                };
                let es = [side_edge(b, c, -1.0), side_edge(c, b, 1.0), side_edge(b, c, 1.0), side_edge(c, b, -1.0)];
                let axes = [(c, -1.0), (b, 1.0), (c, 1.0), (b, -1.0)];
                let mut side = [FaceId { prim: sh.prim, face: 0 }; 4];
                for k in 0..4 {
                    side[k] = if rr(es[k]) > 0.0 {
                        FaceId { prim: sh.prim, face: FILLET0 + es[k] as u32 }
                    } else {
                        let (ax, so) = axes[k];
                        FaceId { prim: sh.prim, face: (ax * 2 + (so < 0.0) as usize) as u32 }
                    };
                }
                let lo = DVec2::new(-s[b] + rr(es[3]), -s[c] + rr(es[0])) * sc;
                let hi = DVec2::new(s[b] - rr(es[1]), s[c] - rr(es[2])) * sc;
                let mut arcs = vec![];
                for (sb, scn) in [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
                    let e = edge_index(a, sb, scn);
                    let r = rr(e);
                    if r > 0.0 {
                        arcs.push((DVec2::new(sb * (s[b] - r), scn * (s[c] - r)) * sc, r * sc, DVec2::new(sb, scn), FaceId { prim: sh.prim, face: FILLET0 + e as u32 }));
                    }
                }
                let mut cons = cons_base.clone();
                for i in 0..arcs.len() {
                    cons.push(Cons::Arc(i));
                }
                charts.push(Chart { id: FaceId { prim: sh.prim, face }, shape: si, map: Map::Plane { o, u, v }, lo, hi, side, arcs, strip: None, ring: None, disc: None, ball: None, cons });
            }
        }
        for e in 0..12usize {
            let r = rr(e);
            if r <= 0.0 {
                continue;
            }
            let (ax, sb, scn) = box_edge_signs(e);
            let (b, c) = ((ax + 1) % 3, (ax + 2) % 3);
            let mut cl = DVec3::ZERO;
            cl[b] = sb * (s[b] - r);
            cl[c] = scn * (s[c] - r);
            let o = tw.transform_point3(cl);
            let am = tw.transform_vector3(unit(ax)).normalize();
            let bm = tw.transform_vector3(unit(b) * sb).normalize();
            let cm = tw.transform_vector3(unit(c) * scn).normalize();
            let rad = r * sc;
            let side = [
                FaceId { prim: sh.prim, face: (b * 2 + (sb < 0.0) as usize) as u32 },
                FaceId { prim: sh.prim, face: (ax * 2) as u32 },
                FaceId { prim: sh.prim, face: (c * 2 + (scn < 0.0) as usize) as u32 },
                FaceId { prim: sh.prim, face: (ax * 2 + 1) as u32 },
            ];
            let segs = fillet_segs(rad, FRAC_PI_2, tol_f).max(((rad * FRAC_PI_2 / st.size).round() as usize + 1) / 2 * 2);
            // an end on a ball corner stops where the ball starts; an end
            // where one more edge of the corner is rounded (the third
            // sharp) meets that edge's strip on a miter
            let mut side = side;
            let (mut t0, mut t1) = (-s[ax], s[ax]);
            let mut miters = vec![];
            for (end, sa) in [(1usize, 1.0f64), (3, -1.0)] {
                let mut sg = DVec3::ZERO;
                sg[ax] = sa;
                sg[b] = sb;
                sg[c] = scn;
                if let Some(rc) = corner_r(pr, corner_of(sg)) {
                    side[end] = FaceId { prim: sh.prim, face: CORNER0 + corner_of(sg) };
                    if sa > 0.0 {
                        t1 = s[ax] - rc;
                    } else {
                        t0 = -s[ax] + rc;
                    }
                    continue;
                }
                let ce = corner_edges(sg);
                let others: Vec<usize> = [b, c].into_iter().filter(|&o| rr(ce[o]) > 0.0).collect();
                // (equal radii only: other corners, where they show, fail later)
                if let [o] = others[..].iter().copied().filter(|&o| (rr(ce[o]) - r).abs() < 1e-9).collect::<Vec<_>>()[..] {
                    side[end] = FaceId { prim: sh.prim, face: FILLET0 + ce[o] as u32 };
                    miters.push(Cons::Miter { ax, sa, o, so: sg[o], d: s[o] - s[ax], face: FILLET0 + ce[o] as u32 });
                    // the miter cuts the strip before its end
                    if sa > 0.0 {
                        t1 = s[ax] + r;
                    } else {
                        t0 = -s[ax] - r;
                    }
                }
            }
            let mut cons = cons_base.clone();
            cons.extend(miters);
            charts.push(Chart {
                id: FaceId { prim: sh.prim, face: FILLET0 + e as u32 },
                shape: si,
                map: Map::Cyl { o, a: am, b: bm, c: cm, r: rad },
                lo: DVec2::new(t0 * sc, 0.0),
                hi: DVec2::new(t1 * sc, rad * FRAC_PI_2),
                side,
                arcs: vec![],
                strip: Some((rad, segs)),
                ring: None,
                disc: None,
                ball: None,
                cons,
            });
        }
        // ball corners: an octant of sphere, three arcs on the strips
        for k in 0..8u32 {
            let Some(r) = corner_r(pr, k) else { continue };
            let sg = corner_signs(k);
            let c = tw.transform_point3((s - DVec3::splat(r)) * sg);
            let ax = [0usize, 1, 2].map(|i| tw.transform_vector3(unit(i) * sg[i]).normalize());
            let d = (ax[0] + ax[1] + ax[2]).normalize();
            let e1 = (ax[0] - ax[1]).normalize();
            let e2 = d.cross(e1);
            let rad = r * sc;
            let id = FaceId { prim: sh.prim, face: CORNER0 + k };
            let faces = corner_edges(sg).map(|e| FaceId { prim: sh.prim, face: FILLET0 + e as u32 });
            let mut cons = cons_base.clone();
            cons.extend([Cons::Oct(0), Cons::Oct(1), Cons::Oct(2)]);
            charts.push(Chart {
                id,
                shape: si,
                map: Map::Sph { c, d, e1, e2, r: rad },
                lo: DVec2::splat(-1.6 * rad),
                hi: DVec2::splat(1.6 * rad),
                side: [id; 4],
                arcs: vec![],
                strip: None,
                ring: None,
                disc: None,
                ball: Some((ax, faces)),
                cons,
            });
        }
    }
    // junction fillets: a sharp box added with a blend, standing on a flat
    // face of the model, its walls square to it: a strip along each wall
    // (straight, its section the blend's profile) and a corner around each
    // edge standing on the face (the profile turned around the edge)
    let mut blends: Vec<BlendPart> = vec![];
    for si in 0..shapes.len() {
        let sh = &shapes[si];
        let k = sh.k;
        if k <= 0.0 {
            continue;
        }
        let (s, sc, tw) = (sh.pr.size, sh.pr.sc, sh.inst.to_world);
        let cb = tw.transform_point3(DVec3::ZERO);
        let wax = |i: usize| tw.transform_vector3(unit(i)).normalize();
        // the face it stands on: (chart, box axis along its normal, normal, height of the centre)
        let mut on: Vec<(usize, usize, DVec3, f64)> = vec![];
        for (ci, ch) in charts.iter().enumerate() {
            let Map::Plane { o, .. } = ch.map else { continue };
            let other = &shapes[ch.shape];
            if other.step >= sh.step || other.pr.kind != 0 || ch.id.face >= FILLET0 {
                continue;
            }
            let fa = ch.id.face as usize;
            let na = other.inst.to_world.transform_vector3(unit(fa / 2) * if fa % 2 == 0 { 1.0 } else { -1.0 }).normalize();
            let Some(ab) = (0..3).find(|&i| wax(i).dot(na).abs() > 1.0 - 1e-9) else { continue };
            // (a cylinder stands on its end)
            if sh.pr.kind == 1 && ab != 1 {
                continue;
            }
            let h = (cb - o).dot(na);
            let half = s[ab] * sc;
            let q = ch.map.to2(cb);
            if h - half < -extent * 1e-6 && h + half > k * 1.5 && q.x > ch.lo.x && q.x < ch.hi.x && q.y > ch.lo.y && q.y < ch.hi.y {
                on.push((ci, ab, na, h));
            }
        }
        if on.len() != 1 {
            return Err(if sh.pr.kind == 1 {
                "fillet de jonction : le cylindre doit être posé debout sur une face plane, pour l'instant"
            } else {
                "fillet de jonction : la boîte doit être posée d'aplomb sur une face plane, pour l'instant"
            }
            .into());
        }
        let (ca, ab, na, h) = on[0];
        let fa = charts[ca].id;
        let foot = cb - na * h;
        let len_l = k * blend_s(1.0);
        let rows = fillet_segs(k * 0.71, FRAC_PI_2, tol_f).max(((len_l / st.size).round() as usize + 1) / 2 * 2);
        if std::env::var("DEBUG_LAYOUT").is_ok() {
            eprintln!("raccord : k {k} profil {len_l:.3} rangées {rows} taille {:.3} tol {tol_f}", st.size);
        }
        let mut cons = vec![Cons::Rect(0), Cons::Rect(1), Cons::Rect(2), Cons::Rect(3)];
        for jj in (si + 1)..shapes.len() {
            cons.push(Cons::Later(jj));
        }
        let mut new_charts: Vec<Chart> = vec![];
        if sh.pr.kind == 1 {
            // a cylinder: a ring around it, in two halves like its wall
            // (each half |θ| ≤ π/2 in the frame of the wall's half)
            let rad = s.x * sc;
            let (bm, cm) = (wax(0), wax(2));
            let rr = rad + k * 0.25;
            let base = blends.len() as u32;
            let ring_id = |w: u32| FaceId { prim: sh.prim, face: BLEND0 + base + w };
            for (w, (b, e, wall)) in [(cm, -bm, CYL_SIDE), (-cm, bm, CYL_SIDE_B)].into_iter().enumerate() {
                let other = ring_id(1 - w as u32);
                blends.push(BlendPart { shape: si, a: fa, kind: BlendKind::Strip { face: wall } });
                new_charts.push(Chart {
                    id: ring_id(w as u32),
                    shape: si,
                    map: Map::Ring { c: foot, a: na, b, e, r: rad, rr, k },
                    lo: DVec2::new(-FRAC_PI_2 * rr, 0.0),
                    hi: DVec2::new(FRAC_PI_2 * rr, len_l),
                    side: [fa, other, FaceId { prim: sh.prim, face: wall }, other],
                    arcs: vec![],
                    strip: Some((len_l / FRAC_PI_2, rows)),
                    ring: None,
                    disc: None,
                    ball: None,
                    cons: cons.clone(),
                });
            }
        } else {
            let (i, j) = ((ab + 1) % 3, (ab + 2) % 3);
            let walls = [(i, 1.0f64), (i, -1.0), (j, 1.0), (j, -1.0)];
            let face_of = |ax: usize, sg: f64| (ax * 2) as u32 + (sg < 0.0) as u32;
            let base = blends.len() as u32;
            let strip_id = |ax: usize, sg: f64| FaceId { prim: sh.prim, face: BLEND0 + base + walls.iter().position(|w| w.0 == ax && w.1 == sg).unwrap() as u32 };
            let corners = [(1.0f64, 1.0f64), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)];
            let corner_id = |si_: f64, sj: f64| FaceId { prim: sh.prim, face: BLEND0 + base + 4 + corners.iter().position(|c| c.0 == si_ && c.1 == sj).unwrap() as u32 };
            for &(ax, sg) in &walls {
                let ox = if ax == i { j } else { i };
                let nb = wax(ax) * sg;
                let e = wax(ox);
                let o = foot + nb * (s[ax] * sc) - e * (s[ox] * sc);
                let len = 2.0 * s[ox] * sc;
                // the edge at x = len is on the + side of axis ox
                let end = |so: f64| if ax == i { corner_id(sg, so) } else { corner_id(so, sg) };
                blends.push(BlendPart { shape: si, a: fa, kind: BlendKind::Strip { face: face_of(ax, sg) } });
                new_charts.push(Chart {
                    id: strip_id(ax, sg),
                    shape: si,
                    map: Map::Blend { o, e, na, nb, k },
                    lo: DVec2::ZERO,
                    hi: DVec2::new(len, len_l),
                    side: [fa, end(1.0), FaceId { prim: sh.prim, face: face_of(ax, sg) }, end(-1.0)],
                    arcs: vec![],
                    strip: Some((len_l / FRAC_PI_2, rows)),
                    ring: None,
                    disc: None,
                    ball: None,
                    cons: cons.clone(),
                });
            }
            for &(sgi, sgj) in &corners {
                let (e1, e2) = (wax(i) * sgi, wax(j) * sgj);
                let c = foot + e1 * (s[i] * sc) + e2 * (s[j] * sc);
                blends.push(BlendPart { shape: si, a: fa, kind: BlendKind::Corner { faces: [face_of(i, sgi), face_of(j, sgj)], c, axis: na } });
                let mut cc = cons.clone();
                cc.push(Cons::Disc);
                new_charts.push(Chart {
                    id: corner_id(sgi, sgj),
                    shape: si,
                    map: Map::Rev { c, a: na, e1, e2, k },
                    lo: DVec2::ZERO,
                    hi: DVec2::splat(k * 1.02),
                    // y = 0: the end of the strip of wall i; x = 0: of wall j
                    side: [strip_id(i, sgi), fa, fa, strip_id(j, sgj)],
                    arcs: vec![],
                    strip: None,
                    ring: None,
                    disc: Some((k, fa)),
                    ball: None,
                    cons: cc,
                });
            }
        }
        // the blend is the profile only where the model before it is that
        // face alone (no other edge or shape within its reach)
        let pf = prev[si].as_ref().ok_or("historique")?;
        let tol = extent * 1e-7 + 1e-9;
        for ch in &new_charts {
            for a in 0..=8 {
                for b in 0..=8 {
                    let (q, u, v) = match ch.map {
                        Map::Rev { .. } => {
                            let (r, th) = (k * a as f64 / 8.0, FRAC_PI_2 * b as f64 / 8.0);
                            (DVec2::new(th.cos(), th.sin()) * r, blend_u(k, r), r)
                        }
                        _ => {
                            let t = b as f64 / 8.0;
                            (DVec2::new(ch.lo.x + (ch.hi.x - ch.lo.x) * a as f64 / 8.0, k * blend_s(t)), k * t * t, k * (1.0 - t) * (1.0 - t))
                        }
                    };
                    let p = ch.map.to3(q);
                    if (pf.dist(p) - u).abs() > tol || (sh.d(p) - v).abs() > tol {
                        return Err("fillet de jonction trop près d'une arête ou d'une autre forme, pas encore géré".into());
                    }
                }
            }
        }
        charts.extend(new_charts);
    }
    progress.set(1, charts.len() as u32);

    let mut lay = Layout { field: &field, shapes, prev, charts, nodes: vec![], edges: vec![], bnd: vec![], rays: vec![], inner: vec![], origins: vec![], node_tol: extent * 2e-6, walls: vec![], blends };
    let fd = extent * 1e-7;

    // a cylinder wall is cut open where it does not show: its seam
    // (θ = ±π in its chart) goes in the middle of the largest hidden gap.
    // Seen all around (a round hole), it is cut in two halves along its
    // local x axis, each half a chart of its own. The strips of its rounded
    // rims go with it (the same seam, or the same two halves).
    for ci in 0..lay.charts.len() {
        let ch = &lay.charts[ci];
        let (Some(_), Map::Cyl { o, a, b, c, r }) = (ch.ring, ch.map) else { continue };
        let rims: Vec<usize> = (0..lay.charts.len()).filter(|&x| lay.charts[x].id.prim == ch.id.prim && matches!(lay.charts[x].map, Map::Tor { .. })).collect();
        let (nth, nt) = (720usize, 48usize);
        // (a point of chart x at angle th, j of nt across)
        let at = |x: usize, th: f64, j: usize| -> DVec2 {
            let cx = &lay.charts[x];
            match cx.map {
                Map::Tor { rr, .. } => DVec2::new(th * rr, cx.hi.y * j as f64 / nt as f64),
                _ => DVec2::new(cx.lo.x + (cx.hi.x - cx.lo.x) * j as f64 / nt as f64, th * r),
            }
        };
        let shows = |x: usize, th: f64| -> bool {
            let cx = &lay.charts[x];
            (0..=nt).any(|j| cx.cons.iter().filter(|k| matches!(k, Cons::Prev | Cons::Later(_))).all(|&k| lay.cval(cx, k, at(x, th, j)) > 0.0))
        };
        let mut vis = vec![false; nth];
        for (k, v) in vis.iter_mut().enumerate() {
            let th = -PI + (k as f64 + 0.5) * TAU / nth as f64;
            *v = std::iter::once(ci).chain(rims.iter().copied()).any(|x| shows(x, th));
        }
        if !vis.iter().any(|&v| v) {
            continue;
        }
        let mut best = (0usize, 0usize);
        for s in 0..nth {
            if vis[s] || !vis[(s + nth - 1) % nth] {
                continue;
            }
            let mut l = 0;
            while l < nth && !vis[(s + l) % nth] {
                l += 1;
            }
            if l > best.0 {
                best = (l, s);
            }
        }
        let turned = |m: Map, phi: f64| match m {
            Map::Cyl { o, a, b, c, r } => Map::Cyl { o, a, b: b * phi.cos() + c * phi.sin(), c: c * phi.cos() - b * phi.sin(), r },
            Map::Tor { c, a, b, e, rm, r, rr } => Map::Tor { c, a, b: b * phi.cos() + e * phi.sin(), e: e * phi.cos() - b * phi.sin(), rm, r, rr },
            m => m,
        };
        if best.0 < 4 {
            let id = ch.id;
            let (ma, mb) = (turned(ch.map, FRAC_PI_2), turned(ch.map, -FRAC_PI_2));
            let ida = id;
            let idb = FaceId { prim: id.prim, face: CYL_SIDE_B };
            let to_b = |f: FaceId| if f.prim == id.prim && (f.face == CYL_RIM0 || f.face == CYL_RIM1) { FaceId { prim: f.prim, face: CYL_RIM_B0 + (f.face - CYL_RIM0) } } else { f };
            let mut hb = Chart { id: idb, map: mb, lo: DVec2::new(ch.lo.x, -FRAC_PI_2 * r), hi: DVec2::new(ch.hi.x, FRAC_PI_2 * r), side: ch.side.map(to_b), arcs: vec![], cons: ch.cons.clone(), ..*ch };
            hb.side[0] = ida;
            hb.side[2] = ida;
            let ha = &mut lay.charts[ci];
            ha.map = ma;
            ha.lo.y = -FRAC_PI_2 * r;
            ha.hi.y = FRAC_PI_2 * r;
            ha.side[0] = idb;
            ha.side[2] = idb;
            lay.charts.push(hb);
            lay.walls.push((id.prim, ma));
            for x in rims {
                let ra = lay.charts[x].id;
                let rb = to_b(ra);
                let Map::Tor { rr, .. } = lay.charts[x].map else { continue };
                let m = lay.charts[x].map;
                let mut tb = Chart { id: rb, map: turned(m, -FRAC_PI_2), arcs: vec![], cons: lay.charts[x].cons.clone(), ..lay.charts[x] };
                tb.lo.x = -FRAC_PI_2 * rr;
                tb.hi.x = FRAC_PI_2 * rr;
                tb.side = [idb, ra, tb.side[2], ra];
                let ta = &mut lay.charts[x];
                ta.map = turned(m, FRAC_PI_2);
                ta.lo.x = -FRAC_PI_2 * rr;
                ta.hi.x = FRAC_PI_2 * rr;
                ta.side[1] = rb;
                ta.side[3] = rb;
                lay.charts.push(tb);
            }
            continue;
        }
        let phi = -PI + (best.1 as f64 + best.0 as f64 * 0.5) * TAU / nth as f64 + PI;
        lay.charts[ci].map = Map::Cyl { o, a, b: b * phi.cos() + c * phi.sin(), c: c * phi.cos() - b * phi.sin(), r };
        for x in rims {
            lay.charts[x].map = turned(lay.charts[x].map, phi);
        }
    }
    // a ring of junction fillet goes with the two halves of its wall
    for sh in &lay.shapes {
        if sh.k > 0.0 && sh.pr.kind == 1 && !lay.walls.iter().any(|w| w.0 == sh.prim) {
            return Err("fillet de jonction : cylindre caché en partie, pas encore géré".into());
        }
    }
    let nc = lay.charts.len();
    lay.bnd = vec![vec![]; nc];
    lay.rays = vec![vec![]; nc];
    lay.inner = vec![vec![]; nc];

    // 1. pieces of boundary of every chart, between nodes
    struct Piece {
        chart: usize,
        a: usize,
        b: usize,
        label: FaceId,
        pts: Vec<DVec3>,
    }
    let mut pieces: Vec<Piece> = vec![];
    let mut chart_loops: Vec<Vec<Vec<usize>>> = vec![vec![]; nc];
    let mut raw_loops: Vec<(usize, Vec<(FaceId, Vec<DVec3>)>)> = vec![];
    for ci in 0..nc {
        let ch = &lay.charts[ci];
        let size = ch.hi - ch.lo;
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        let mut grid = (extent / 260.0).min(size.min_element() / 6.0);
        if let Some((r, _)) = ch.strip {
            grid = grid.min(r * 0.25);
        }
        for (_, r, _, _) in &ch.arcs {
            grid = grid.min(r * 0.25);
        }
        let f = |q: DVec2| lay.fmin(ch, q).0;
        let loops = march(&f, ch.lo, ch.hi, grid);
        for lp in loops {
            // refine every point onto the curve it belongs to
            let mut pts: Vec<(DVec2, Cons, FaceId, FaceId)> = vec![];
            for q in lp {
                let (_, k) = lay.fmin(ch, q);
                let l = lay.label(ch, k, q);
                let g = lay.geom(ch, k, l, q);
                let q2 = proj1(&|x| lay.psi(ch, k, g, x), q, fd);
                if q2.distance(q) > grid * 2.0 {
                    continue;
                }
                pts.push((q2, k, l, g));
            }
            let n = pts.len();
            if n < 3 {
                continue;
            }
            // nodes where the curve or the face across changes
            let mut marks: Vec<(usize, DVec2)> = vec![];
            for i in 0..n {
                let (qa, ka, la, ga) = pts[i];
                let (qb, kb, lb, gb) = pts[(i + 1) % n];
                if ka == kb && la == lb && ga == gb {
                    continue;
                }
                let fa = |x: DVec2| lay.psi(ch, ka, ga, x);
                let fb = |x: DVec2| lay.psi(ch, kb, gb, x);
                // (one curve, the face across changes: no crossing to solve)
                let same = ka == kb && !matches!(ka, Cons::Prev | Cons::Later(_));
                let mut node = if same { None } else { solve_cross(&fa, &fb, (qa + qb) * 0.5, fd).filter(|x| x.distance((qa + qb) * 0.5) < grid * 3.0) };
                if node.is_none() {
                    // tangent change: bisection on which curve is under the point
                    let (mut s0, mut s1) = (0.0, 1.0);
                    let mut last = qa;
                    for _ in 0..50 {
                        let m = (s0 + s1) * 0.5;
                        let x = qa.lerp(qb, m);
                        let (_, k) = lay.fmin(ch, x);
                        let l = lay.label(ch, k, x);
                        let g = lay.geom(ch, k, l, x);
                        let xp = proj1(&|y| lay.psi(ch, k, g, y), x, fd);
                        if k == ka && l == la && g == ga {
                            s0 = m;
                        } else {
                            s1 = m;
                        }
                        last = xp;
                    }
                    node = Some(last);
                }
                marks.push((i, node.unwrap()));
            }
            // two marks on one spot (a march point right on a corner): no
            // piece between them, the piece before runs to the second
            let mut k = 0;
            while marks.len() > 1 && k < marks.len() {
                let nk = (k + 1) % marks.len();
                if ch.map.to3(marks[k].1).distance(ch.map.to3(marks[nk].1)) < lay.node_tol {
                    marks.remove(k);
                } else {
                    k += 1;
                }
            }
            if marks.is_empty() {
                return Err("contour fermé sans coin (trou rond) pas encore géré".into());
            }
            let mut raw = vec![];
            for (k, &(i, q)) in marks.iter().enumerate() {
                let (j, nq) = marks[(k + 1) % marks.len()];
                // points strictly between the two nodes
                let mut poly = vec![q];
                let mut t = (i + 1) % n;
                let label = pts[t].2;
                loop {
                    let p = pts[t].0;
                    if p.distance(q) > grid && p.distance(nq) > grid {
                        poly.push(p);
                    }
                    if t == j {
                        break;
                    }
                    t = (t + 1) % n;
                }
                poly.push(nq);
                raw.push((label, poly.iter().map(|x| ch.map.to3(*x)).collect::<Vec<DVec3>>()));
            }
            raw_loops.push((ci, raw));
        }
        progress.tick();
    }
    for (ci, raw) in raw_loops {
        let mut ids = vec![];
        // a cylinder's rounded rim meets another shape along a row (a plane
        // square to the axis: the part's face where the cylinder stands on
        // its rounded foot, the face a hole's rounded floor opens on) or
        // right across, from rail to rail (a box through the rim): the
        // strip then ends there; a cut into the strip that stays on one
        // rail is not handled yet
        if let (Map::Tor { r, .. }, true) = (lay.charts[ci].map, matches!(lay.charts[ci].map, Map::Tor { .. })) {
            let m = lay.charts[ci].map;
            let (lo, hi) = (lay.charts[ci].lo.y, lay.charts[ci].hi.y);
            let across = raw.iter().any(|(l, pts)| {
                let y: Vec<f64> = pts.iter().map(|p| m.to2(*p).y).collect();
                let (y0, y1) = y.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| (a.min(v), b.max(v)));
                l.prim != lay.charts[ci].id.prim && y1 - y0 > r * 0.01 && !(y0 - lo < r * 0.01 && hi - y1 < r * 0.01)
            });
            if across {
                return Err("bord arrondi d'un cylindre coupé par une autre forme ou par le bord de la pièce : pas encore géré".into());
            }
        }
        for (label, pts) in raw {
            let a = lay.node(pts[0]);
            let b = lay.node(*pts.last().unwrap());
            if a == b {
                if std::env::var("DEBUG_LAYOUT").is_ok() {
                    eprintln!("refermé : chart {:?} label {:?} {} points {:?} .. {:?}", lay.charts[ci].id, label, pts.len(), pts[0], pts.last().unwrap());
                }
                return Err("bord refermé sur un seul coin".into());
            }
            pieces.push(Piece { chart: ci, a, b, label, pts });
            ids.push(pieces.len() - 1);
        }
        chart_loops[ci].push(ids);
    }

    // 2. each piece meets its twin in the chart across
    let chart_of: HashMap<FaceId, usize> = lay.charts.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
    let mut piece_edge: Vec<Option<(usize, bool)>> = vec![None; pieces.len()];
    for i in 0..pieces.len() {
        if piece_edge[i].is_some() {
            continue;
        }
        let p = &pieces[i];
        let me = lay.charts[p.chart].id;
        let Some(&other) = chart_of.get(&p.label) else {
            return Err(format!("face voisine absente ({:?})", p.label));
        };
        let mid = p.pts[p.pts.len() / 2];
        let twin = (0..pieces.len())
            .filter(|&j| j != i && piece_edge[j].is_none() && pieces[j].chart == other && pieces[j].label == me && ((pieces[j].a == p.a && pieces[j].b == p.b) || (pieces[j].a == p.b && pieces[j].b == p.a)))
            .min_by(|&x, &y| {
                let dx = pieces[x].pts.iter().map(|q| q.distance(mid)).fold(f64::INFINITY, f64::min);
                let dy = pieces[y].pts.iter().map(|q| q.distance(mid)).fold(f64::INFINITY, f64::min);
                dx.total_cmp(&dy)
            });
        let Some(j) = twin else {
            if std::env::var("DEBUG_LAYOUT").is_ok() {
                eprintln!("sans partenaire : {:?} -> {:?}, coins {} {:?} / {} {:?}, {} points, milieu {:?}", me, p.label, p.a, lay.nodes[p.a], p.b, lay.nodes[p.b], p.pts.len(), mid);
                for (j, q) in pieces.iter().enumerate() {
                    if q.chart == other && q.label == me {
                        eprintln!("  en face {j} : coins {} {:?} / {} {:?} libre {}", q.a, lay.nodes[q.a], q.b, lay.nodes[q.b], piece_edge[j].is_none());
                    }
                }
            }
            return Err("ligne sans partenaire de l'autre côté".into());
        };
        let e = lay.edges.len();
        let mut pts = p.pts.clone();
        pts[0] = lay.nodes[p.a];
        let last = pts.len() - 1;
        pts[last] = lay.nodes[p.b];
        lay.edges.push(GEdge { a: p.a, b: p.b, pts, charts: [p.chart, other], ray: false, alive: true });
        piece_edge[i] = Some((e, true));
        piece_edge[j] = Some((e, pieces[j].a == p.a));
    }
    for ci in 0..nc {
        for lp in &chart_loops[ci] {
            let l: Vec<(usize, bool)> = lp.iter().map(|&i| piece_edge[i].unwrap()).collect();
            lay.bnd[ci].push(l);
        }
    }

    // arcs longer than a quarter turn (a half or a whole hole) are cut
    // into quarters, each hooked on its own
    for ci in 0..nc {
        if !matches!(lay.charts[ci].map, Map::Plane { .. }) {
            continue;
        }
        let loops = lay.bnd[ci].clone();
        for lp in loops {
            for &(e, fwd) in &lp {
                let other = lay.edges[e].charts.iter().copied().find(|&c| c != ci).unwrap_or(ci);
                let sm = lay.charts[other].map;
                if !matches!(sm, Map::Cyl { .. } | Map::Ring { .. } | Map::Tor { .. }) {
                    continue;
                }
                let pp = lay.hpts(e, fwd, ci);
                let total: f64 = pp.windows(3).map(|w| turn(w[1] - w[0], w[2] - w[1])).sum();
                let k = (total.abs() / FRAC_PI_2).round() as usize;
                let (qa, qb) = (sm.to2(lay.nodes[lay.edges[e].a]), sm.to2(lay.nodes[lay.edges[e].b]));
                let me = lay.charts[ci].id;
                let Some(sh) = lay.shape_of(me.prim) else { continue };
                // a hole's arc cut by a straight edge of the face (a notch
                // through the edge): cut where the circle is square to the
                // face's axes (its widest points and its bottom), as on a
                // whole hole, so that its squares stand square to the edge
                let (ea, eb) = (lay.edges[e].a, lay.edges[e].b);
                // (the same on the flat cap of a rounded rim, the cut
                // carried across the rim strip below)
                let square = match (sm, lay.charts[ci].map) {
                    (Map::Cyl { a, .. }, Map::Plane { u, v, .. }) => lay.charts[other].strip.is_none() && a.dot(u.cross(v)).abs() > 0.999 && total.abs() > 0.6,
                    (Map::Tor { a, .. }, Map::Plane { u, v, .. }) => a.dot(u.cross(v)).abs() > 0.999 && total.abs() > 0.6,
                    _ => false,
                };
                let centre = match sm {
                    Map::Cyl { o, .. } => Some(o),
                    Map::Tor { c, .. } => Some(c),
                    _ => None,
                };
                if let (Some(o), true) = (centre, square) {
                    if lay.straight_at(ci, e, ea).is_some() || lay.straight_at(ci, e, eb).is_some() {
                        let map = lay.charts[ci].map;
                        let p2 = lay.pts2(e, ci);
                        let c2 = map.to2(o);
                        let r = p2[0].distance(c2);
                        let mut acc = vec![0.0];
                        for w in p2.windows(2) {
                            acc.push(acc[acc.len() - 1] + w[0].distance(w[1]));
                        }
                        let len = acc[acc.len() - 1];
                        let margin = st.size * 0.35;
                        let mut cuts: Vec<(f64, DVec2)> = vec![];
                        for dir in [DVec2::X, -DVec2::X, DVec2::Y, -DVec2::Y] {
                            let q = c2 + dir * r;
                            let mut best = (f64::INFINITY, 0.0);
                            for (i, w) in p2.windows(2).enumerate() {
                                let ab = w[1] - w[0];
                                let t = ((q - w[0]).dot(ab) / ab.length_squared().max(1e-30)).clamp(0.0, 1.0);
                                let d = (w[0] + ab * t).distance(q);
                                if d < best.0 {
                                    best = (d, acc[i] + (acc[i + 1] - acc[i]) * t);
                                }
                            }
                            if best.0 < r * 0.05 && best.1 > margin && len - best.1 > margin {
                                cuts.push((best.1, q));
                            }
                        }
                        cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
                        let xs: Vec<DVec3> = cuts.iter().map(|&(_, q)| sm.to3(proj1(&|x| sh.face_fn(me.face, sm.to3(x)), sm.to2(map.to3(q)), fd))).collect();
                        let mut cur = e;
                        for &x in &xs {
                            lay.split(cur, x);
                            cur = lay.edges.len() - 1;
                        }
                        if matches!(sm, Map::Tor { .. }) {
                            lay.carry_across(other, &xs);
                        }
                        continue;
                    }
                }
                if k < 2 {
                    continue;
                }
                let cuts: Vec<DVec3> = (1..k).map(|j| sm.to3(proj1(&|x| sh.face_fn(me.face, sm.to3(x)), qa.lerp(qb, j as f64 / k as f64), fd))).collect();
                let mut cur = e;
                for &x in &cuts {
                    lay.split(cur, x);
                    cur = lay.edges.len() - 1;
                }
                // a strip around a cylinder (its rounded rim, a junction
                // fillet's ring): the same cuts on the strip's other rail,
                // along the wall (cut at the same angles at its other end),
                // so that the wall's columns run straight
                if matches!(sm, Map::Tor { .. } | Map::Ring { .. }) {
                    lay.carry_across(other, &cuts);
                }
            }
        }
    }

    progress.set(2, 1);
    // 3. loop origins: concave fillet ends (middle of the arc) and sharp
    // re-entrant corners
    let mut queue: VecDeque<(usize, usize, DVec2)> = VecDeque::new();
    let mut forced: Vec<(usize, usize)> = vec![];
    // rounded corners of each face, (centre, radius, unit directions from
    // the centre to the arc's two ends, rows across the strip): its own
    // rounded edges, and where another shape's rounded edge crosses it (a
    // notch through a box's rounded edge) found below from the boundary
    let strip_rows = |id: FaceId| lay.charts.iter().find(|c| c.id == id).and_then(|c| c.strip).map(|s| s.1).unwrap_or(0);
    let mut rounded: Vec<Vec<(DVec2, f64, [DVec2; 2], usize)>> =
        lay.charts.iter().map(|ch| ch.arcs.iter().map(|&(c, r, sg, id)| (c, r, [DVec2::new(sg.x, 0.0), DVec2::new(0.0, sg.y)], strip_rows(id))).collect()).collect();
    // areas of a face that loops from elsewhere do not cut into: the
    // square around a hooked concave arc (a round hole's ring) and the
    // middle square of a round cap; a loop that would enter one stops in a
    // T on its side, where that side is a line (convex quad, sides flagged)
    let mut keep: Vec<Vec<([DVec2; 4], [bool; 4])>> = vec![vec![]; nc];
    // the end of a strip on a miter: a loop that reaches its rails there
    // (the rail of the other fillet, carried around the part, comes back
    // exactly where the miter starts) ends on the rail in a T instead of
    // cutting a triangle off the strip against the miter
    for ci in 0..nc {
        let ch = &lay.charts[ci];
        let Some((rad, _)) = ch.strip else { continue };
        let sc = lay.shapes[ch.shape].pr.sc;
        for k in &ch.cons {
            let Cons::Miter { ax, sa, .. } = *k else { continue };
            let s_ax = lay.shapes[ch.shape].pr.size[ax] * sc;
            let (x0, x1) = (sa * (s_ax - rad * 1.25), sa * (s_ax + rad));
            let h = rad * FRAC_PI_2;
            keep[ci].push(([DVec2::new(x0, 0.0), DVec2::new(x1, 0.0), DVec2::new(x1, h), DVec2::new(x0, h)], [true, false, true, false]));
        }
    }
    // the diagonals of the hooks, per chart: a loop meeting one ends there
    // (diagonal from, to; corner of the arc's tangents, walls, arc's middle)
    #[allow(clippy::type_complexity)]
    let mut diags: Vec<Vec<(DVec2, DVec2, DVec2, DVec2, DVec2, DVec3)>> = vec![vec![]; nc];
    // round caps seen whole (a blind hole's floor, the top of a cylinder):
    // an o-grid, a square in the middle whose corners face the middles of
    // the four quarter arcs, where the wall's lines come in; nothing
    // crosses the disc
    let mut ogrid = vec![false; nc];
    // cylinders whose cap is cut through (a notch): the segments around
    // their wall are a floor, not a fixed count, since the arcs left between
    // the cuts no longer split the turn evenly
    let mut notched: Vec<u32> = vec![];
    for ci in 0..nc {
        let Some((rad, _)) = lay.charts[ci].disc else { continue };
        if matches!(lay.charts[ci].map, Map::Rev { .. }) {
            continue;
        }
        let loops = lay.bnd[ci].clone();
        if loops.len() != 1 {
            continue;
        }
        let me = lay.charts[ci].id;
        let map = lay.charts[ci].map;
        let wall = |lay: &Layout, e: usize| -> Option<usize> {
            let other = lay.edges[e].charts.iter().copied().find(|&c| c != ci).unwrap_or(ci);
            (lay.charts[other].id.prim == me.prim && matches!(lay.charts[other].map, Map::Cyl { .. } | Map::Tor { .. })).then_some(other)
        };
        let quarter = loops[0].len() == 4
            && loops[0].iter().all(|&(e, fwd)| {
                let pp = lay.hpts(e, fwd, ci);
                let total: f64 = pp.windows(3).map(|w| turn(w[1] - w[0], w[2] - w[1])).sum();
                wall(&lay, e).is_some() && (total.abs() - FRAC_PI_2).abs() < 0.2
            });
        if !quarter && std::env::var("DEBUG_LAYOUT").is_ok() {
            for &(e, fwd) in &loops[0] {
                let pp = lay.hpts(e, fwd, ci);
                let total: f64 = pp.windows(3).map(|w| turn(w[1] - w[0], w[2] - w[1])).sum();
                let other = lay.edges[e].charts.iter().copied().find(|&c| c != ci).unwrap_or(ci);
                eprintln!("dessus {:?} : vers {:?} tour {:.2} de {:?} à {:?}", me, lay.charts[other].id, total, pp[0], pp[pp.len() - 1]);
            }
        }
        if !quarter {
            // a cap cut by flat faces of other shapes (a box through the top
            // of a boss, the part's edge through a blind hole's floor): the
            // same o-grid, if the cuts are square to the chart's axes and
            // leave the four diagonals. A cut parallel to a side of the
            // square ("floor") within half a quad of it carries that side;
            // farther out, lines from its ends run square to the side
            if !loops[0].iter().any(|&(e, _)| wall(&lay, e).is_some()) {
                continue;
            }
            let mut floors: Vec<(usize, f64, [usize; 2])> = vec![];
            // (the axis a cut piece runs along, by its place in the loop)
            let mut cut_ax: Vec<Option<usize>> = vec![None; loops[0].len()];
            for (i, &(e, fwd)) in loops[0].iter().enumerate() {
                if wall(&lay, e).is_some() {
                    continue;
                }
                let other = lay.edges[e].charts.iter().copied().find(|&c| c != ci).unwrap_or(ci);
                if lay.charts[other].id.prim == me.prim || !matches!(lay.charts[other].map, Map::Plane { .. }) {
                    return Err("dessus ou fond d'un cylindre coupé par une forme courbe : pas encore géré".into());
                }
                let pp = lay.hpts(e, fwd, ci);
                let (pa, pb) = (pp[0], pp[pp.len() - 1]);
                let d = pb - pa;
                let tol = lay.node_tol * 10.0;
                let ax = if pp.iter().all(|q| (q.x - pa.x).abs() < tol) {
                    0
                } else if pp.iter().all(|q| (q.y - pa.y).abs() < tol) {
                    1
                } else {
                    return Err("dessus ou fond d'un cylindre coupé en biais par une autre forme : pas encore géré".into());
                };
                cut_ax[i] = Some(ax);
                // (a piece along the side of a sector, not across it)
                let mid = (pa + pb) * 0.5;
                let along_side = if ax == 0 { mid.x.abs() > mid.y.abs() } else { mid.y.abs() > mid.x.abs() };
                if along_side && d.length() > tol {
                    let (na, nb) = if fwd { (lay.edges[e].a, lay.edges[e].b) } else { (lay.edges[e].b, lay.edges[e].a) };
                    let sector = if ax == 0 { if mid.x > 0.0 { 0 } else { 2 } } else if mid.y > 0.0 { 1 } else { 3 };
                    floors.push((sector, if ax == 0 { mid.x.abs() } else { mid.y.abs() }, [na, nb]));
                }
            }
            // the middles of the arcs between the cuts, one per quarter of
            // the chart, where the hooks at the foot of the wall take theirs
            // (the middle angle between the arc's ends; an end on a cut is
            // taken where the cut's line meets the wall's own radius, so that
            // the columns run straight down a rounded rim and the wall)
            let r_wall = {
                let sh = &lay.shapes[lay.charts[ci].shape];
                sh.pr.size.x * sh.pr.sc
            };
            let dir_at = |lay: &Layout, node: usize, ax: Option<usize>| -> DVec2 {
                let q = map.to2(lay.nodes[node]);
                match ax {
                    Some(0) => DVec2::new(q.x, q.y.signum() * (r_wall * r_wall - q.x * q.x).max(0.0).sqrt()).normalize(),
                    Some(_) => DVec2::new(q.x.signum() * (r_wall * r_wall - q.y * q.y).max(0.0).sqrt(), q.y).normalize(),
                    None => q.normalize(),
                }
            };
            let nl = loops[0].len();
            let mut quarters: [Option<(usize, usize, DVec2)>; 4] = [None; 4];
            for (i, &(e, fwd)) in loops[0].iter().enumerate() {
                let Some(other) = wall(&lay, e) else { continue };
                let (na, nb) = if fwd { (lay.edges[e].a, lay.edges[e].b) } else { (lay.edges[e].b, lay.edges[e].a) };
                let m2 = (dir_at(&lay, na, cut_ax[(i + nl - 1) % nl]) + dir_at(&lay, nb, cut_ax[(i + 1) % nl])).normalize_or_zero() * rad;
                let q = (m2.y.atan2(m2.x).rem_euclid(TAU) / FRAC_PI_2) as usize % 4;
                let x = map.to3(m2);
                // (an arc shorter than a half turn: its middle is between)
                let pp = lay.hpts(e, fwd, ci);
                let on = pp.windows(3).map(|w| turn(w[1] - w[0], w[2] - w[1])).sum::<f64>().abs() < PI * 0.9;
                if std::env::var("DEBUG_LAYOUT").is_ok() {
                    eprintln!("milieu {:?} quart {q} sur l'arc {on} ({:?} -> {:?})", m2, map.to2(lay.nodes[na]), map.to2(lay.nodes[nb]));
                }
                if !on || quarters[q].is_some() || m2.x.abs().min(m2.y.abs()) < rad * 0.05 {
                    return Err("dessus ou fond d'un cylindre coupé sur une diagonale de son o-grid : pas encore géré".into());
                }
                let m = lay.split(e, x);
                quarters[q] = Some((m, other, m2));
            }
            let Some(mids) = quarters.iter().copied().collect::<Option<Vec<(usize, usize, DVec2)>>>() else {
                return Err("dessus ou fond d'un cylindre coupé sur une diagonale de son o-grid : pas encore géré".into());
            };
            // the square: its sides, as levels along each sector's axis
            // (sector 0: +x of the chart, 1: +y, 2: -x, 3: -y)
            let s0 = rad * 0.6 * std::f64::consts::FRAC_1_SQRT_2;
            let axis = [DVec2::X, DVec2::Y, -DVec2::X, -DVec2::Y];
            let mut level = [s0; 4];
            let mut snapped = [false; 4];
            let mut ends: [Option<(f64, [usize; 2])>; 4] = [None; 4];
            for &(sector, l, nn) in &floors {
                if ends[sector].is_some() {
                    return Err("dessus ou fond d'un cylindre coupé deux fois du même côté : pas encore géré".into());
                }
                if l < s0 - st.size * 0.5 {
                    return Err("dessus ou fond d'un cylindre coupé trop près de son centre : pas encore géré".into());
                }
                if (l - s0).abs() < st.size * 0.5 {
                    level[sector] = l;
                    snapped[sector] = true;
                }
                ends[sector] = Some((l, nn));
            }
            let corner = |q: usize| -> DVec2 {
                // corner in the quarter between sectors q and q + 1, facing
                // its middle as on a whole cap; on a side the cut carries,
                // moved onto the cut
                let mut g = mids[q].2 * 0.6;
                for k in [q, (q + 1) % 4] {
                    if snapped[k] {
                        g += axis[k] * (level[k] - g.dot(axis[k]));
                    }
                }
                g
            };
            ogrid[ci] = true;
            notched.push(me.prim);
            // corners: mids[q] is in the quarter between sectors q and q + 1
            let mut gs: Vec<(usize, DVec2)> = vec![];
            for q in 0..4 {
                let g2 = corner(q);
                let g = lay.node(map.to3(g2));
                // (a corner of every patch around it, even where the line
                // from the arc's middle comes in flat, beside a cut)
                forced.push((ci, g));
                gs.push((g, g2));
            }
            let ray = |lay: &mut Layout, a: usize, b: usize| {
                let re = lay.edges.len();
                lay.edges.push(GEdge { a, b, pts: vec![lay.nodes[a], lay.nodes[b]], charts: [ci, ci], ray: true, alive: true });
                lay.rays[ci].push(re);
            };
            for j in 0..4 {
                let (m, other, _) = mids[j];
                let (g, gq) = gs[j];
                let (g2, gq2) = gs[(j + 1) % 4];
                lay.origins.push(m);
                lay.origins.push(g);
                lay.inner[ci].push(g);
                ray(&mut lay, m, g);
                // the side from g to g2 faces sector (j + 1) % 4
                let sector = (j + 1) % 4;
                match ends[sector] {
                    None => ray(&mut lay, g, g2),
                    Some((_, nn)) => {
                        // the cut's ends along the side, from g
                        let side = (gq2 - gq).normalize();
                        let mut pts: Vec<(f64, usize)> = nn.iter().map(|&n| ((map.to2(lay.nodes[n]) - gq).dot(side), n)).collect();
                        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
                        let span = gq.distance(gq2);
                        if pts[0].0 < lay.node_tol * 10.0 || pts[1].0 > span - lay.node_tol * 10.0 {
                            return Err("dessus ou fond d'un cylindre coupé plus large que le carré de son o-grid : pas encore géré".into());
                        }
                        if snapped[sector] {
                            // the cut carries the side
                            ray(&mut lay, g, pts[0].1);
                            ray(&mut lay, pts[1].1, g2);
                        } else {
                            // lines from its ends square to the side
                            let feet: Vec<usize> = pts.iter().map(|&(t, _)| lay.node(map.to3(gq + side * t))).collect();
                            for k in 0..2 {
                                lay.inner[ci].push(feet[k]);
                                ray(&mut lay, pts[k].1, feet[k]);
                            }
                            ray(&mut lay, g, feet[0]);
                            ray(&mut lay, feet[0], feet[1]);
                            ray(&mut lay, feet[1], g2);
                        }
                    }
                }
                // the wall's line from the arc's middle
                queue.push_back((other, m, DVec2::ZERO));
            }
            keep[ci].push(([gs[0].1, gs[1].1, gs[2].1, gs[3].1], [true; 4]));
            continue;
        }
        ogrid[ci] = true;
        let mut mids: Vec<(f64, usize, DVec2, usize)> = vec![];
        for &(e, _) in &loops[0] {
            let other = lay.edges[e].charts.iter().copied().find(|&c| c != ci).unwrap_or(ci);
            let (qa, qb) = (map.to2(lay.nodes[lay.edges[e].a]), map.to2(lay.nodes[lay.edges[e].b]));
            let m2 = (qa.normalize() + qb.normalize()).normalize() * rad;
            let m = lay.split(e, map.to3(m2));
            mids.push((m2.y.atan2(m2.x), m, m2, other));
        }
        mids.sort_by(|a, b| a.0.total_cmp(&b.0));
        let gs: Vec<(usize, DVec2)> = mids.iter().map(|&(_, _, m2, _)| (lay.node(map.to3(m2 * 0.6)), m2 * 0.6)).collect();
        for (j, &(_, m, _, other)) in mids.iter().enumerate() {
            let (g, _) = gs[j];
            let (g2, _) = gs[(j + 1) % 4];
            lay.origins.push(m);
            lay.origins.push(g);
            lay.inner[ci].push(g);
            for (a, b) in [(m, g), (g, g2)] {
                let re = lay.edges.len();
                lay.edges.push(GEdge { a, b, pts: vec![lay.nodes[a], lay.nodes[b]], charts: [ci, ci], ray: true, alive: true });
                lay.rays[ci].push(re);
            }
            // the wall's line from the arc's middle
            queue.push_back((other, m, DVec2::ZERO));
        }
        keep[ci].push(([gs[0].1, gs[1].1, gs[2].1, gs[3].1], [true; 4]));
    }
    // ball corners: three quads around the centre, a loop from the middle
    // of each arc on into its strip (the strip's middle row)
    for ci in 0..nc {
        let (Some(_), Map::Sph { c, r, .. }) = (lay.charts[ci].ball, lay.charts[ci].map) else { continue };
        let loops = lay.bnd[ci].clone();
        if loops.len() != 1 || loops[0].len() != 3 {
            return Err("coin arrondi coupé par une autre forme, pas encore géré".into());
        }
        let g = lay.node(lay.charts[ci].map.to3(DVec2::ZERO));
        lay.origins.push(g);
        lay.inner[ci].push(g);
        for &(e, _) in &loops[0] {
            let (pa, pb) = (lay.nodes[lay.edges[e].a], lay.nodes[lay.edges[e].b]);
            let mid = c + ((pa - c).normalize() + (pb - c).normalize()).normalize() * r;
            let other = lay.edges[e].charts.iter().copied().find(|&x| x != ci).unwrap_or(ci);
            let m = lay.split(e, mid);
            let re = lay.edges.len();
            lay.edges.push(GEdge { a: g, b: m, pts: vec![lay.nodes[g], lay.nodes[m]], charts: [ci, ci], ray: true, alive: true });
            lay.rays[ci].push(re);
            queue.push_back((other, m, DVec2::ZERO));
        }
    }
    // hooks placed so far: the wall chart, the arc's middle, the hook node
    // or landing, whether it was a landing, and whether on the part's own
    // edge (the other end of a through opening takes the same square: see
    // below)
    let mut ends: Vec<(usize, DVec3, DVec3, bool, bool)> = vec![];
    for ci in 0..nc {
        if lay.charts[ci].strip.is_some() || ogrid[ci] || matches!(lay.charts[ci].map, Map::Rev { .. }) {
            continue;
        }
        // hooks with no room inside the face, placed once every loop of
        // the face is planned (they meet the boundary where a loop does)
        let mut tight: Vec<(usize, DVec2, DVec2, bool, usize, DVec2, Option<DVec2>, Option<DVec3>)> = vec![];
        // sharp re-entrant corners (first: hooks below align on their loops)
        let loops = lay.bnd[ci].clone();
        for lp in loops {
            let n = lp.len();
            for k in 0..n {
                let (ea, fa) = lp[k];
                let (eb, fb) = lp[(k + 1) % n];
                let ti = lay.end_dir(ea, fa, ci);
                let to = lay.start_dir(eb, fb, ci);
                if turn(ti, to) < -0.5 {
                    let v = if fa { lay.edges[ea].b } else { lay.edges[ea].a };
                    lay.origins.push(v);
                    queue.push_back((ci, v, ti));
                    queue.push_back((ci, v, -to));
                }
            }
        }
        let loops = lay.bnd[ci].clone();
        for lp in loops {
            let n = lp.len();
            for k in 0..n {
                let (e, fwd) = lp[k];
                let other = lay.edges[e].charts.iter().copied().find(|&c| c != ci).unwrap_or(ci);
                let pp = lay.hpts(e, fwd, ci);
                let total: f64 = pp.windows(3).map(|w| turn(w[1] - w[0], w[2] - w[1])).sum();
                let planar = matches!(lay.charts[ci].map, Map::Plane { .. });
                let rev = matches!(lay.charts[other].map, Map::Rev { .. });
                if planar && (matches!(lay.charts[other].map, Map::Cyl { .. } | Map::Ring { .. } | Map::Tor { .. }) || rev) && total.abs() > 0.6 {
                    // an arc against a fillet strip or a cylinder wall: its
                    // middle, in the curved chart (a whole row on a strip)
                    let sm = lay.charts[other].map;
                    let (na, nb) = (lay.edges[e].a, lay.edges[e].b);
                    let (qa, qb) = (sm.to2(lay.nodes[na]), sm.to2(lay.nodes[nb]));
                    let mut mid = (qa + qb) * 0.5;
                    if lay.charts[other].strip.is_some() {
                        let row = lay.charts[other].row();
                        mid.y = (mid.y / row).round() * row;
                    }
                    let me = lay.charts[ci].id;
                    if let Map::Rev { k, .. } = sm {
                        // the rail of a junction fillet's corner: a circle
                        mid = (qa.normalize() + qb.normalize()).normalize() * k;
                    } else if let Some(sh) = lay.shape_of(me.prim) {
                        mid = proj1(&|x| sh.face_fn(me.face, sm.to3(x)), mid, fd);
                    }
                    // exact directions of the arc at both ends
                    let t_in = lay.tangent(e, fwd, ci, false);
                    let t_out = lay.tangent(e, fwd, ci, true);
                    // an end where the arc runs into a straight edge of the
                    // face at an angle (a hole through the edge): the side of
                    // the square there stands square to that edge, and an arc
                    // between that edge and the circle's widest point (both
                    // ends square to the edge) has no square of its own, it
                    // is part of the next one's side
                    let (vs, ve) = if fwd { (na, nb) } else { (nb, na) };
                    let hole = lay.charts[other].strip.is_none() && matches!(sm, Map::Cyl { .. });
                    let square_to = |t: DVec2, v: usize| -> (DVec2, bool) {
                        match lay.straight_at(ci, e, v).filter(|_| hole) {
                            Some(d) if (-d).perp_dot(t).abs() > 0.17 || (-d).dot(t) < 0.0 => {
                                let n = perp(d);
                                (if n.dot(t) < 0.0 { -n } else { n }, true)
                            }
                            _ => (t, false),
                        }
                    };
                    let (h_in, cut_in) = square_to(t_in, vs);
                    let (h_out, cut_out) = square_to(-t_out, ve);
                    let h_out = -h_out;
                    if (cut_in || cut_out) && h_in.perp_dot(h_out).abs() < 0.17 && total < 0.0 && st.diagonal && planar && !rev {
                        if std::env::var("DEBUG_LAYOUT").is_ok() {
                            eprintln!("arc entre le bord et le flanc, sans carré : {:?}", lay.charts[ci].id);
                        }
                        continue;
                    }
                    let m = lay.split(e, sm.to3(mid));
                    if total < 0.0 && st.diagonal {
                        // concave fillet hooked diagonally: from its middle a
                        // line runs to a grid node one quad away from the
                        // corner, where the two loops of the face cross
                        lay.origins.push(m);
                        let map = lay.charts[ci].map;
                        let (ps, pe) = if fwd { (na, nb) } else { (nb, na) };
                        let (p0, q0) = (map.to2(lay.nodes[ps]), map.to2(lay.nodes[pe]));
                        let (a1, a2) = (h_in.normalize(), -h_out.normalize());
                        // corner where the two walls would meet
                        let den = a1.perp_dot(h_out);
                        if den.abs() < 1e-6 {
                            return Err("fillet entre deux murs parallèles".into());
                        }
                        let s = (q0 - p0).perp_dot(h_out) / den;
                        let x = p0 + a1 * s;
                        // one quad away, nearer when the face ends before
                        // (a hole or a junction close to the face's edge)
                        let mut hook = st.size;
                        let mut g2 = x + (a1 + a2) * hook;
                        let mq = map.to2(lay.nodes[m]);
                        // (the node on the face, and the diagonal to it too:
                        // not across a hole or a shape)
                        let clear = |g: DVec2, h: f64| lay.fmin(&lay.charts[ci], g).0 > h * 0.1 && (1..8).all(|i| lay.fmin(&lay.charts[ci], mq.lerp(g, i as f64 / 8.0)).0 > 0.0);
                        for _ in 0..8 {
                            if clear(g2, hook) {
                                break;
                            }
                            hook *= 0.7;
                            g2 = x + (a1 + a2) * hook;
                        }
                        // no room for a hook inside the face (an edge, a hole
                        // or a shape right there, less than ~half a quad): the
                        // diagonal runs on to that boundary, where a loop
                        // planned nearby meets it if there is one, and the
                        // sides of the square there are the boundary itself
                        let (n0, n1) = (perp(t_in), perp(t_out));
                        let dn = n0.perp_dot(n1);
                        let centre = (dn.abs() > 1e-9).then(|| p0 + n0 * ((q0 - p0).perp_dot(n1) / dn));
                        // the other end of a through opening (a hole's arc
                        // on the face across, on the same wall, right in
                        // line): the same square here, so that both ends
                        // agree. Its landing in line, when this face's edge
                        // is there too, else a hook node there when it
                        // landed on the part's own edge (a face wider at
                        // this end: its edges rounded at the other); its
                        // hook node in line. (Only where the diagonal is
                        // clear of any other shape)
                        let m3 = lay.nodes[m];
                        let twin = ends.iter().find(|t| t.0 == other && t.1.distance(m3) > lay.node_tol * 10.0 && map.to2(t.1).distance(mq) < lay.node_tol * 10.0).copied().and_then(|t| {
                            let y2 = map.to2(t.2);
                            if !(1..8).all(|i| lay.fmin(&lay.charts[ci], mq.lerp(y2, i as f64 / 8.0)).0 > 0.0) {
                                return None;
                            }
                            if t.3 && boundary_hit(&lay, ci, mq, (y2 - mq).normalize_or_zero()).is_some_and(|h| map.to2(h.1).distance(y2) < lay.node_tol * 10.0) {
                                Some((true, t.2))
                            } else if (!t.3 || t.4) && lay.fmin(&lay.charts[ci], y2).0 > hook * 0.1 {
                                Some((false, t.2))
                            } else {
                                None
                            }
                        });
                        if twin.is_some() && std::env::var("DEBUG_LAYOUT").is_ok() {
                            eprintln!("jumeau {:?} n{m} {:?} <- {:?}", lay.charts[ci].id, m3, twin);
                        }
                        if hook < st.size * 0.6 || twin.is_some_and(|t| t.0) {
                            tight.push((m, a1, a2, rev, other, x, centre, twin.filter(|t| t.0).map(|t| t.1)));
                            continue;
                        }
                        let twin_g = twin.map(|t| map.to2(t.1));
                        // on a loop already planned nearby, parallel to one
                        // of the walls (a hole's ring, another hook): there,
                        // rather than a thin row beside it
                        let mut hk = [hook, hook];
                        for (w, ax) in [a1, a2].into_iter().enumerate() {
                            let mut best: Option<f64> = None;
                            for &(qc, qn, qd) in queue.iter() {
                                if qc != ci || qd == DVec2::ZERO || qd.normalize().dot([a2, a1][w]).abs() < 0.999 {
                                    continue;
                                }
                                let c = (map.to2(lay.nodes[qn]) - x).dot(ax);
                                if c > hook * 0.05 && c < hook * 1.6 && best.is_none_or(|b| (c - hook).abs() < (b - hook).abs()) {
                                    best = Some(c);
                                }
                            }
                            // or where that side lands on the boundary right
                            // by a node of it (an arc's end, a corner): through
                            // it, rather than a sliver row along the face's edge
                            if best.is_none() {
                                let b = [a2, a1][w];
                                let gt = x + (a1 + a2) * hook;
                                for dir in [b, -b] {
                                    let Some((e, x3)) = boundary_hit(&lay, ci, gt, dir) else { continue };
                                    // (a straight edge: by an arc's node the loop
                                    // just lands on the arc)
                                    let p2 = lay.pts2(e, ci);
                                    let (pa, pb) = (p2[0], p2[p2.len() - 1]);
                                    if p2.iter().any(|&p| (p - pa).perp_dot((pb - pa).normalize_or_zero()).abs() > lay.node_tol * 10.0) {
                                        continue;
                                    }
                                    for n in [lay.edges[e].a, lay.edges[e].b] {
                                        let c = (map.to2(lay.nodes[n]) - x).dot(ax);
                                        if lay.nodes[n].distance(x3) < st.size * 0.2 && c > hook * 0.05 && (c - hook).abs() < st.size * 0.2 && best.is_none_or(|b| (c - hook).abs() < (b - hook).abs()) {
                                            if std::env::var("DEBUG_LAYOUT").is_ok() {
                                                eprintln!("côté du carré par n{n} {:?} ({c:.2} au lieu de {hook:.2})", lay.nodes[n]);
                                            }
                                            best = Some(c);
                                        }
                                    }
                                }
                            }
                            // or where a loop from another face came across
                            // the rows on the last pass
                            let mm = lay.nodes[m];
                            if let Some(h) = hints.iter().find(|h| h.chart == ci && h.w == w && h.m.distance(mm) < lay.node_tol * 10.0 && h.c > hook * 0.05 && h.c < hook * 1.6) {
                                best = Some(h.c);
                            }
                            // or another feature less than two hooks away
                            // beyond that side (a second hole, a shape): the
                            // two squares would overlap and their diagonals
                            // cross. The side is on the middle of the gap,
                            // which the other feature's square then shares
                            if best.is_none() {
                                if let Some(cn) = centre {
                                    let r = cn.distance(p0);
                                    let b = [a2, a1][w];
                                    let eps = lay.node_tol * 10.0;
                                    let mut gap: Option<f64> = None;
                                    for t in [-2.0 * r, -1.5 * r, -r, -0.5 * r, 0.0, hook * 0.5, hook] {
                                        let o = x + b * t + ax * eps;
                                        if lay.fmin(&lay.charts[ci], o).0 <= 0.0 {
                                            continue;
                                        }
                                        let Some((e, x3)) = boundary_hit(&lay, ci, o, ax) else { continue };
                                        // (not the face's own edge: that is the
                                        // rule above, the square on the edge)
                                        if lay.edges[e].charts.iter().all(|&c| lay.charts[c].id.prim == lay.charts[ci].id.prim) {
                                            continue;
                                        }
                                        let d = (map.to2(x3) - o).dot(ax) + eps;
                                        if gap.is_none_or(|g| d < g) {
                                            gap = Some(d);
                                        }
                                    }
                                    if let Some(d) = gap.filter(|&d| d < hook * 2.0 && d > hook * 0.3) {
                                        if std::env::var("DEBUG_LAYOUT").is_ok() {
                                            eprintln!("côté du carré au milieu de l'écart {:?} ({:.2} au lieu de {hook:.2})", lay.nodes[m], d * 0.5);
                                        }
                                        best = Some(d * 0.5);
                                    }
                                }
                            }
                            if let Some(c) = best {
                                hk[w] = c;
                            }
                        }
                        // (not when that takes the node off the face)
                        let g2 = [hk, [hk[0], hook], [hook, hk[1]], [hook, hook]]
                            .into_iter()
                            .map(|h| x + a1 * h[0] + a2 * h[1])
                            .find(|&g| lay.fmin(&lay.charts[ci], g).0 > hook * 0.1)
                            .unwrap_or(x + (a1 + a2) * hook);
                        let g2 = twin_g.unwrap_or(g2);
                        // nor when a side of the square would run along the
                        // face's edge less than half a quad away (a sliver
                        // row): the square rests on the edge there, as when
                        // there is no room. (A straight edge along that side,
                        // not another feature's arc, which has its own square)
                        let near_edge = [a1, a2].into_iter().any(|dir| {
                            boundary_hit(&lay, ci, g2, dir).is_some_and(|(e, x3)| {
                                let p2 = lay.pts2(e, ci);
                                let (pa, pb) = (p2[0], p2[p2.len() - 1]);
                                let t = (pb - pa).normalize_or_zero();
                                let straight = p2.iter().all(|&p| (p - pa).perp_dot(t).abs() < lay.node_tol * 10.0);
                                straight && t.dot(dir).abs() < 0.1 && (map.to2(x3) - g2).length() < st.size * 0.5
                            })
                        });
                        if near_edge && twin_g.is_none() {
                            tight.push((m, a1, a2, rev, other, x, centre, None));
                            continue;
                        }
                        // the hook node of another feature right there: shared
                        let near_g = lay.inner[ci].iter().copied().find(|&n| map.to2(lay.nodes[n]).distance(g2) < hook * 0.5);
                        let g = match near_g {
                            Some(n) => n,
                            None => {
                                let n = lay.node(map.to3(g2));
                                lay.origins.push(n);
                                lay.inner[ci].push(n);
                                n
                            }
                        };
                        ends.push((other, m3, lay.nodes[g], false, true));
                        // the square between the arc's centre and the hook
                        // node: its two sides through the node are loops
                        let (n0, n1) = (perp(t_in), perp(t_out));
                        let dn = n0.perp_dot(n1);
                        let dd = a1.perp_dot(a2);
                        if dn.abs() > 1e-9 && dd.abs() > 1e-9 {
                            let c = p0 + n0 * ((q0 - p0).perp_dot(n1) / dn);
                            let g2 = map.to2(lay.nodes[g]);
                            let w = g2 - c;
                            let (al, be) = (w.perp_dot(a2) / dd, a1.perp_dot(w) / dd);
                            if al > 0.0 && be > 0.0 {
                                keep[ci].push(([c, c + a1 * al, g2, c + a2 * be], [false, true, true, false]));
                            }
                        }
                        let re = lay.edges.len();
                        lay.edges.push(GEdge { a: m, b: g, pts: vec![lay.nodes[m], lay.nodes[g]], charts: [ci, ci], ray: true, alive: true });
                        diags[ci].push((mq, map.to2(lay.nodes[g]), x, a1, a2, lay.nodes[m]));
                        lay.rays[ci].push(re);
                        for d in [a1, -a1, a2, -a2] {
                            queue.push_back((ci, g, d));
                        }
                        if rev {
                            // a junction fillet's corner is one patch: its
                            // arc's middle is a corner of it
                            forced.push((other, m));
                        } else {
                            queue.push_back((other, m, DVec2::ZERO));
                        }
                    } else if total < 0.0 {
                        // concave fillet: a re-entrant corner in its middle
                        lay.origins.push(m);
                        queue.push_back((ci, m, t_in));
                        queue.push_back((ci, m, -t_out));
                        if rev {
                            forced.push((other, m));
                        } else {
                            queue.push_back((other, m, DVec2::ZERO));
                        }
                    } else {
                        // convex fillet: loops from its rails, its middle is a
                        // corner of the rounded patch
                        forced.push((ci, m));
                        // another shape's rounded edge across this face,
                        // a whole quarter: a rounded corner of its own
                        let foreign = lay.charts[other].id.prim != lay.charts[ci].id.prim;
                        if foreign && lay.charts[other].strip.is_some() && lay.charts[ci].disc.is_none() && (total.abs() - FRAC_PI_2).abs() < 0.15 && t_in.dot(t_out).abs() < 0.05 {
                            let map = lay.charts[ci].map;
                            let (ps, pe) = if fwd { (na, nb) } else { (nb, na) };
                            let (p0, q0) = (map.to2(lay.nodes[ps]), map.to2(lay.nodes[pe]));
                            // centre: on both ends' normals
                            let (n0, n1) = (perp(t_in), perp(t_out));
                            let den = n0.perp_dot(n1);
                            if den.abs() > 1e-9 {
                                let s0 = (q0 - p0).perp_dot(n1) / den;
                                let c = p0 + n0 * s0;
                                let (r0, r1) = (p0.distance(c), q0.distance(c));
                                if (r0 - r1).abs() < r0.max(r1) * 0.02 {
                                    let r = (r0 + r1) * 0.5;
                                    let rows = lay.charts[other].strip.map(|s| s.1).unwrap_or(0);
                                    rounded[ci].push((c, r, [(p0 - c) / r0, (q0 - c) / r1], rows));
                                }
                            }
                        }
                        queue.push_back((ci, na, DVec2::ZERO));
                        queue.push_back((ci, nb, DVec2::ZERO));
                    }
                }
            }
        }
        for (m, a1, a2, rev, other, x, c, twin) in tight {
            let map = lay.charts[ci].map;
            let o2 = map.to2(lay.nodes[m]);
            let dd = a1.perp_dot(a2);
            // a point seen from the corner of the arc's tangents, along both
            // walls: the gaps between the arc and the square's sides there
            let gaps = |y: DVec2| {
                let w = y - x;
                (w.perp_dot(a2) / dd, a1.perp_dot(w) / dd)
            };
            let Some((e, x3)) = boundary_hit(&lay, ci, o2, (a1 + a2).normalize()) else {
                return Err("accroche sans place ni bord en face".into());
            };
            let xq = map.to2(x3);
            // the loops planned on the face, parallel to a wall, and where
            // they land on the boundary
            let mut landings: Vec<(f64, usize, DVec3)> = vec![];
            let mut crossing: Option<(f64, usize, DVec3)> = None;
            for &(qc, qn, qd) in queue.iter() {
                if qc != ci || qd == DVec2::ZERO {
                    continue;
                }
                let qd = qd.normalize();
                let side = if qd.dot(a2).abs() > 0.999 {
                    0
                } else if qd.dot(a1).abs() > 0.999 {
                    1
                } else {
                    continue;
                };
                let q = map.to2(lay.nodes[qn]);
                let off = [gaps(q).0, gaps(q).1][side];
                if let Some((e2, y3)) = boundary_hit(&lay, ci, q, qd) {
                    landings.push((off, e2, y3));
                    // a loop across the diagonal's way: it would end on the
                    // diagonal, a T that no counts can close (the rows on
                    // both sides of the diagonal differ by its piece)
                    let y2 = map.to2(y3);
                    let (r, dg) = (y2 - q, xq - o2);
                    let den = dg.perp_dot(r);
                    if den.abs() > 1e-12 {
                        let t = (q - o2).perp_dot(r) / den;
                        let w = (q - o2).perp_dot(dg) / den;
                        let (ya, yb) = gaps(y2);
                        let clear = (1..8).all(|i| lay.fmin(&lay.charts[ci], o2.lerp(y2, i as f64 / 8.0)).0 > 0.0);
                        // (not a diagonal bent far off: within 30 degrees)
                        let bent = (y2 - o2).normalize_or_zero().dot(dg.normalize_or_zero()) < 0.866;
                        if t > 0.05 && t < 1.0 && w > 0.0 && w < 1.0 && ya > 0.0 && yb > 0.0 && clear && !bent && crossing.is_none_or(|c: (f64, usize, DVec3)| t < c.0) {
                            crossing = Some((t, e2, y3));
                        }
                    }
                }
            }
            // (edge, point, snap): where a loop planned nearby will land if
            // there is one, else the boundary's node there or a new one
            let mut best = (e, x3, st.size * 0.3);
            let mut bd = st.size * 0.6;
            for &(_, e2, y3) in &landings {
                let dist = map.to2(y3).distance(xq);
                if dist < bd {
                    bd = dist;
                    best = (e2, y3, st.size * 0.02);
                }
            }
            // or a loop from another face that ran into this diagonal on
            // the last pass (round the part's edge, from the other end of
            // a through hole)
            let mm = lay.nodes[m];
            for h in hints.iter().filter(|h| h.chart == ci && h.m.distance(mm) < lay.node_tol * 10.0) {
                let (aw, ao) = if h.w == 0 { (a1, a2) } else { (a2, a1) };
                let dg = xq - o2;
                let den = dg.dot(aw);
                if den.abs() < 1e-12 {
                    continue;
                }
                let t = (h.c - (o2 - x).dot(aw)) / den;
                if t <= 0.05 || t >= 1.0 || crossing.is_some_and(|c| c.0 <= t) {
                    continue;
                }
                let q = o2 + dg * t;
                if let Some((e2, y3)) = boundary_hit(&lay, ci, q, ao) {
                    let y2 = map.to2(y3);
                    let (ya, yb) = gaps(y2);
                    let clear = (1..8).all(|i| lay.fmin(&lay.charts[ci], o2.lerp(y2, i as f64 / 8.0)).0 > 0.0);
                    let bent = (y2 - o2).normalize_or_zero().dot(dg.normalize_or_zero()) < 0.866;
                    if ya > 0.0 && yb > 0.0 && clear && !bent {
                        crossing = Some((t, e2, y3));
                    }
                }
            }
            // a loop across the way: the diagonal lands where that loop
            // does, the loop is then a side of the square
            if let Some((_, e2, y3)) = crossing {
                bd = 0.0;
                best = (e2, y3, st.size * 0.02);
            }
            // the other end of a through opening landed: in line with it
            if let Some(t3) = twin {
                let y2 = map.to2(t3);
                if let Some((e2, y3)) = boundary_hit(&lay, ci, o2, (y2 - o2).normalize_or_zero()).filter(|h| map.to2(h.1).distance(y2) < lay.node_tol * 10.0) {
                    bd = 0.0;
                    best = (e2, y3, st.size * 0.02);
                }
            }
            // a corner of the face in the square's quadrant, too close for
            // a hook on either wall (less than a quad and a half: a hook
            // there would leave less than half a quad to both edges): the
            // square rests on both edges, its corner is the face's corner
            // and the diagonal runs into it, rather than leaving a short
            // stretch of edge there and a loop from the landing across the
            // face (a sliver row, and a T on the next diagonal)
            if bd >= st.size * 0.6 {
                let ed = &lay.edges[e];
                let mut cn: Option<(f64, usize)> = None;
                for n in [ed.a, ed.b] {
                    let y2 = map.to2(lay.nodes[n]);
                    let dist = y2.distance(xq);
                    let (ya, yb) = gaps(y2);
                    let convex = corner_angle(&lay, ci, n).is_some_and(|a| a > 0.5);
                    let clear = (1..8).all(|i| lay.fmin(&lay.charts[ci], o2.lerp(y2, i as f64 / 8.0)).0 > 0.0);
                    if dist > best.2 && ya > 0.0 && yb > 0.0 && ya.max(yb) < st.size * 1.5 && convex && clear && cn.is_none_or(|c| dist < c.0) {
                        cn = Some((dist, n));
                    }
                }
                if let Some((_, n)) = cn {
                    best = (e, lay.nodes[n], st.size * 0.02);
                }
            }
            // a corner of the boundary (another shape's) right at the corner
            // of the square: its loops would graze the arc. The square's
            // corner is rather where the side of the next hook's square
            // meets the boundary, and the loops grazing the arc from the
            // square's sides end there. (Not instead of a loop's landing:
            // that loop is there anyway, the square's side is on it)
            let reserve = c;
            let y = boundary_end(&lay, best.0, best.1, best.2).map_or(best.1, |n| lay.nodes[n]);
            let (ga, gb) = gaps(map.to2(y));
            if bd >= st.size * 0.6 && ga.min(gb) < st.size * 0.3 {
                let mut alt: Option<(f64, usize, DVec3)> = None;
                for &(off, e2, y3) in &landings {
                    let y2 = map.to2(y3);
                    let (ya, yb) = gaps(y2);
                    let dist = y2.distance(xq);
                    let clear = (1..8).all(|i| lay.fmin(&lay.charts[ci], o2.lerp(y2, i as f64 / 8.0)).0 > 0.0);
                    if off >= st.size * 0.3 && ya > 0.0 && yb > 0.0 && dist < st.size && clear && alt.is_none_or(|a| dist < a.0) {
                        alt = Some((dist, e2, y3));
                    }
                }
                if let Some((_, e2, y3)) = alt {
                    best = (e2, y3, st.size * 0.02);
                }
            }
            let own = lay.edges[best.0].charts.iter().all(|&c| lay.charts[c].id.prim == lay.charts[ci].id.prim);
            let g = boundary_node(&mut lay, best.0, best.1, best.2);
            ends.push((other, lay.nodes[m], lay.nodes[g], true, own));
            lay.origins.push(g);
            if std::env::var("DEBUG_LAYOUT").is_ok() {
                eprintln!("accroche au bord {:?} n{m} -> n{g} {:?}", lay.charts[ci].id, lay.nodes[g]);
            }
            let gq = map.to2(lay.nodes[g]);
            if let Some(c) = reserve {
                let w = gq - c;
                let (al, be) = (w.perp_dot(a2) / dd, a1.perp_dot(w) / dd);
                if al > 0.0 && be > 0.0 {
                    keep[ci].push(([c, c + a1 * al, gq, c + a2 * be], [false, true, true, false]));
                }
            }
            let re = lay.edges.len();
            lay.edges.push(GEdge { a: m, b: g, pts: vec![lay.nodes[m], lay.nodes[g]], charts: [ci, ci], ray: true, alive: true });
            diags[ci].push((o2, gq, x, a1, a2, lay.nodes[m]));
            lay.rays[ci].push(re);
            for d in [a1, -a1, a2, -a2] {
                // only into the face (not along its edge)
                if lay.fmin(&lay.charts[ci], gq + d * (st.size * 0.2)).0 > st.size * 0.1 {
                    queue.push_back((ci, g, d));
                }
            }
            if rev {
                forced.push((other, m));
            } else {
                queue.push_back((other, m, DVec2::ZERO));
            }
        }
    }

    // corners of junction fillets: three quads around a node, like a ball
    // corner. The rows of the strips turn the corner, the innermost `a`
    // through the quad at the edge's foot, the others through the two quads
    // on the arc; the arc gets 2a segments
    for ci in 0..nc {
        let Map::Rev { k, .. } = lay.charts[ci].map else { continue };
        let map = lay.charts[ci].map;
        let Some(&sc) = chart_of.get(&lay.charts[ci].side[0]) else { continue };
        let Some((_, n)) = lay.charts[sc].strip else { continue };
        let len_l = k * blend_s(1.0);
        let a = ((FRAC_PI_2 * k / st.size * 0.5).round() as usize).clamp(1, n.saturating_sub(1).max(1));
        let v1 = {
            let t = blend_t(len_l * (n - a) as f64 / n as f64 / k);
            k * (1.0 - t) * (1.0 - t)
        };
        // the middle of the arc, split when the face around was hooked
        let Some(m) = forced.iter().find(|f| f.0 == ci).map(|f| f.1) else {
            return Err("coin de fillet de jonction sans milieu".into());
        };
        let mut mids = [0usize; 2];
        for (w, q) in [DVec2::new(v1, 0.0), DVec2::new(0.0, v1)].into_iter().enumerate() {
            let x = map.to3(q);
            let lp = lay.bnd[ci].clone();
            let Some(&(e, _)) = lp.iter().flatten().find(|&&(e, _)| {
                let (pa, pb) = (map.to2(lay.nodes[lay.edges[e].a]), map.to2(lay.nodes[lay.edges[e].b]));
                (pa - q).perp_dot(pb - q).abs() < k * k * 1e-6 && (pa - q).dot(pb - q) < 0.0
            }) else {
                return Err("coin de fillet de jonction : côté introuvable".into());
            };
            mids[w] = lay.split(e, x);
        }
        let g = lay.node(map.to3(DVec2::splat(v1)));
        lay.inner[ci].push(g);
        lay.origins.push(g);
        for b in [mids[0], mids[1], m] {
            let re = lay.edges.len();
            lay.edges.push(GEdge { a: g, b, pts: vec![lay.nodes[g], lay.nodes[b]], charts: [ci, ci], ray: true, alive: true });
            lay.rays[ci].push(re);
        }
    }

    // 4. trace the loops across the faces
    let snap = (st.size * 0.02).max(extent * 1e-4);
    let mut guard = 0;
    while let Some((ci, n0, d0)) = queue.pop_front() {
        guard += 1;
        if guard > 4000 {
            return Err("boucles sans fin".into());
        }
        let o = lay.charts[ci].map.to2(lay.nodes[n0]);
        // direction: given, or square to the boundary into the face
        let mut d = if d0 == DVec2::ZERO {
            match inward(&lay, ci, n0) {
                Some(d) => d,
                None => continue,
            }
        } else {
            d0.normalize()
        };
        // into a strip from its end, between its rails: along the row,
        // even where the end is cut slightly slanted (a box through the
        // rounded rim of a cylinder)
        if d0 == DVec2::ZERO && lay.charts[ci].strip.is_some() {
            let (lo, hi) = (lay.charts[ci].lo.y, lay.charts[ci].hi.y);
            if o.y - lo > (hi - lo) * 0.01 && hi - o.y > (hi - lo) * 0.01 && d.x.abs() > d.y.abs() {
                d = DVec2::new(d.x.signum(), 0.0);
            }
        }
        // a loop entering a rounded corner of the face from its arc (a row
        // of the strip) crosses the corner patch to the opposite side:
        // from the half next to a rail it runs along that rail
        let mut on_arc: Option<(DVec2, [DVec2; 2], usize)> = None;
        if d0 == DVec2::ZERO {
            for &(c, r, u, _) in &rounded[ci] {
                let w = DVec2::new((o - c).dot(u[0]), (o - c).dot(u[1]));
                // (not its ends: the rails, whose loops cross the whole face)
                if w.min_element() > r * 0.05 && ((o - c).length() - r).abs() < snap.max(r * 0.02) {
                    let k = if w.y > w.x { 1 } else { 0 };
                    d = -u[k];
                    on_arc = Some((c, u, k));
                }
            }
        }
        // first crossing with the chart's boundary
        let first_hit = |d: DVec2| -> Option<(f64, usize, usize, f64)> {
            let mut best: Option<(f64, usize, usize, f64)> = None;
            for lp in &lay.bnd[ci] {
                for &(e, _) in lp {
                    let p2 = lay.pts2(e, ci);
                    for i in 0..p2.len() - 1 {
                        let (p, q) = (p2[i], p2[i + 1]);
                        let r = q - p;
                        let den = d.perp_dot(r);
                        if den.abs() < 1e-14 {
                            continue;
                        }
                        let s = (p - o).perp_dot(r) / den;
                        let w = (p - o).perp_dot(d) / den;
                        if s > extent * 1e-6 && (-1e-9..=1.0 + 1e-9).contains(&w) && best.is_none_or(|b| s < b.0) {
                            best = Some((s, e, i, w));
                        }
                    }
                }
            }
            best
        };
        // two rounded corners facing each other along one side of the face
        // (a box's top and bottom rounded edges seen on a side face): one
        // column of quads between them, so their rails are one line, from
        // arc end to arc end (slightly slanted when the radii differ),
        // instead of two lines a T apart
        if d0 == DVec2::ZERO && on_arc.is_none() {
            let mut aim: Option<DVec2> = None;
            for &(c, r, u, rows) in &rounded[ci] {
                for k in 0..2 {
                    if (c + u[k] * r).distance(o) > snap {
                        continue;
                    }
                    // (as many rows on both: the column keeps its count)
                    for &(c2, r2, u2, rows2) in &rounded[ci] {
                        for k2 in 0..2 {
                            if rows2 != rows || u2[1 - k2].dot(u[1 - k]) < 0.999 || u2[k2].dot(u[k]) > -0.999 {
                                continue;
                            }
                            let p = c2 + u2[k2] * r2;
                            let w = p - o;
                            if w.dot(-u[k]) > snap && w.dot(u[1 - k]).abs() < r.max(r2) && aim.is_none_or(|a| w.length() < (a - o).length()) {
                                aim = Some(p);
                            }
                        }
                    }
                }
            }
            if let Some(p) = aim {
                // only when nothing stands between them
                let da = (p - o).normalize();
                if first_hit(da).is_some_and(|b| (o + da * b.0).distance(p) < snap) {
                    d = da;
                }
            }
        }
        // already a loop leaving this node this way
        let exists = lay.rays[ci].iter().any(|&r| {
            let ed = &lay.edges[r];
            let (from, to) = if ed.a == n0 { (ed.a, ed.b) } else if ed.b == n0 { (ed.b, ed.a) } else { return false };
            let m = lay.charts[ci].map;
            (m.to2(lay.nodes[to]) - m.to2(lay.nodes[from])).normalize_or_zero().dot(d) > 0.999
        });
        if exists || keep[ci].iter().any(|(poly, lines)| starts_into(poly, lines, o, d, snap)) {
            continue;
        }
        let best = first_hit(d);
        // a loop node inside the face on the way (where loops cross)
        let mut stop: Option<(f64, usize)> = None;
        for &g in &lay.inner[ci] {
            if g == n0 {
                continue;
            }
            let w = lay.charts[ci].map.to2(lay.nodes[g]) - o;
            let s = w.dot(d);
            if s > extent * 1e-6 && (w - d * s).length() < snap && best.is_none_or(|b| s < b.0) && stop.is_none_or(|x| s < x.0) {
                stop = Some((s, g));
            }
        }
        // a rounded corner of the face (between its arc and the lines from
        // the arc's ends) is a patch of its own: a loop coming from the
        // face stops on its side (a T), rather than cutting a triangle off
        // it against the arc
        let mut tee: Option<(f64, DVec2)> = None;
        if let Some((c, u, k)) = on_arc {
            let t = (o - c).dot(u[k]);
            if t > extent * 1e-6 {
                tee = Some((t, o + d * t));
            }
        }
        for &(c, r, u, _) in &rounded[ci] {
            let lo = |q: DVec2| (q - c).dot(u[0]).min((q - c).dot(u[1]));
            if lo(o) > -snap {
                continue;
            }
            // first point of the ray inside the corner square [0, r]²
            let mut enter: Option<f64> = None;
            for k in 0..2 {
                let dk = d.dot(u[k]);
                if dk.abs() < 1e-12 {
                    continue;
                }
                let t = -(o - c).dot(u[k]) / dk;
                let q = o + d * t;
                let w1 = (q - c).dot(u[1 - k]);
                if t > extent * 1e-6 && w1 >= -1e-9 && w1 <= r && enter.is_none_or(|x| t < x) {
                    enter = Some(t);
                }
            }
            if let Some(t) = enter {
                if best.is_none_or(|b| t < b.0 - snap) && tee.is_none_or(|x| t < x.0) {
                    tee = Some((t, o + d * t));
                }
            }
        }
        // a loop running into a hook's diagonal (across the rows round
        // a hole or a notch): it ends there, a T on the diagonal, rather
        // than cutting a corner off both patches beside it
        let mut across: Option<Hint> = None;
        for &(a, b, x, a1, a2, m3) in &diags[ci] {
            let r = b - a;
            let den = d.perp_dot(r);
            if den.abs() < 1e-12 {
                continue;
            }
            let t = (a - o).perp_dot(r) / den;
            let w = (a - o).perp_dot(d) / den;
            let q = o + d * t;
            if t > snap && w > 0.0 && w < 1.0 && q.distance(a) > snap && q.distance(b) > snap && best.is_none_or(|b| t < b.0 - snap) && tee.is_none_or(|x| t < x.0) {
                tee = Some((t, q));
                // (where the square's side would have to be for this loop)
                across = [a2, a1].into_iter().position(|v| v.dot(d).abs() > 0.999).map(|k| Hint { chart: ci, m: m3, w: k, c: (q - x).dot([a1, a2][k]) });
            }
        }
        for (poly, lines) in &keep[ci] {
            if let Some(t) = enter_poly(poly, lines, o, d, snap) {
                if t > extent * 1e-6 && best.is_none_or(|b| t < b.0 - snap) && tee.is_none_or(|x| t < x.0) {
                    tee = Some((t, o + d * t));
                    across = None;
                }
            }
        }
        let mut new_tee = false;
        if let Some((t, q)) = tee {
            if stop.is_none_or(|x| t < x.0 + snap) {
                if let Some(h) = across {
                    found.push(h);
                }
                // the T of a loop already there, or a new one
                let m = lay.charts[ci].map;
                let near = lay.inner[ci].iter().copied().find(|&g| m.to2(lay.nodes[g]).distance(q) < snap);
                let g = match near {
                    Some(g) => g,
                    None => {
                        let g = lay.node(m.to3(q));
                        lay.inner[ci].push(g);
                        new_tee = true;
                        // a loop already traced through the T takes it
                        let rs: Vec<usize> = lay.rays[ci].iter().copied().filter(|&r| lay.edges[r].alive).collect();
                        for r in rs {
                            let ed = &lay.edges[r];
                            if ed.a == g || ed.b == g {
                                continue;
                            }
                            let (a, b) = (m.to2(lay.nodes[ed.a]), m.to2(lay.nodes[ed.b]));
                            let ab = b - a;
                            let t = (q - a).dot(ab) / ab.length_squared().max(1e-30);
                            if t > 1e-6 && t < 1.0 - 1e-6 && (a + ab * t).distance(q) < snap {
                                let x = lay.nodes[g];
                                lay.split(r, x);
                            }
                        }
                        g
                    }
                };
                stop = Some((t, g));
            }
        }
        if let Some((_, g)) = stop {
            if lay.rays[ci].iter().any(|&r| lay.edges[r].alive && ((lay.edges[r].a == n0 && lay.edges[r].b == g) || (lay.edges[r].a == g && lay.edges[r].b == n0))) {
                continue;
            }
            let re = lay.edges.len();
            if std::env::var("DEBUG_LAYOUT").is_ok() {
                eprintln!("ray {:?} e{re} n{n0} {:?} dir {:?} -> stop n{g} {:?}", lay.charts[ci].id, lay.nodes[n0], d, lay.nodes[g]);
            }
            lay.edges.push(GEdge { a: n0, b: g, pts: vec![lay.nodes[n0], lay.nodes[g]], charts: [ci, ci], ray: true, alive: true });
            lay.rays[ci].push(re);
            // a loop meeting a T node goes on through it (the T's own loop)
            if !new_tee && !lay.origins.contains(&g) {
                queue.push_back((ci, g, d));
            }
            continue;
        }
        let Some((_, e, i, w)) = best else {
            if std::env::var("DEBUG_LAYOUT").is_ok() {
                eprintln!("sans sortie : {:?} n{n0} {:?} dir {:?}", lay.charts[ci].id, lay.nodes[n0], d);
            }
            return Err("boucle qui ne ressort pas de la face".into());
        };
        let ed = lay.edges[e].clone();
        let x = ed.pts[i].lerp(ed.pts[i + 1], w.clamp(0.0, 1.0));
        // a loop landing close to a node takes it, rather than leaving a
        // sliver row next to it (a hole's side passing by an arc's end)
        // (only a small bend of a long loop: about one degree)
        let near = (st.size * 0.3).min(x.distance(lay.nodes[n0]) * 0.02).max(snap);
        let (da, db) = (x.distance(lay.nodes[ed.a]), x.distance(lay.nodes[ed.b]));
        // (not onto the end of a loop running alongside, from right next to
        // this one's start: the two would close a triangle)
        let alongside = |n: usize, gap: f64| {
            lay.rays[ci].iter().any(|&r| {
                let ed = &lay.edges[r];
                let o = if ed.a == n { ed.b } else if ed.b == n { ed.a } else { return false };
                lay.nodes[o].distance(lay.nodes[n0]) < gap * 3.0 + snap
            })
        };
        let n1 = if da < near && da <= db && !alongside(ed.a, da) {
            ed.a
        } else if db < near && !alongside(ed.b, db) {
            ed.b
        } else {
            lay.split(e, x)
        };
        if n1 == n0 {
            continue;
        }
        let re = lay.edges.len();
        if std::env::var("DEBUG_LAYOUT").is_ok() {
            eprintln!("ray {:?} e{re} n{n0} {:?} dir {:?} -> n{n1} {:?}", lay.charts[ci].id, lay.nodes[n0], d, lay.nodes[n1]);
        }
        lay.edges.push(GEdge { a: n0, b: n1, pts: vec![lay.nodes[n0], lay.nodes[n1]], charts: [ci, ci], ray: true, alive: true });
        lay.rays[ci].push(re);
        if lay.origins.contains(&n1) {
            continue;
        }
        // on into the face across
        let next = ed.charts.iter().copied().find(|&c| c != ci);
        if let Some(nc2) = next {
            if corner_angle(&lay, nc2, n1).is_some_and(|t| t.abs() > 0.5) || matches!(lay.charts[nc2].map, Map::Rev { .. }) {
                continue;
            }
            // a loop already leaves this node across: this one closes on it;
            // the middle of a rounded corner is a patch corner, an end too
            if lay.rays[nc2].iter().any(|&r| lay.edges[r].a == n1 || lay.edges[r].b == n1) || forced.contains(&(nc2, n1)) {
                continue;
            }
            queue.push_back((nc2, n1, DVec2::ZERO));
        }
    }

    // 5. loops crossing each other inside a face
    for ci in 0..nc {
        let mut tries = 0;
        'again: loop {
            tries += 1;
            if tries > 2000 {
                return Err("croisements de boucles".into());
            }
            let rs = lay.rays[ci].clone();
            let m = lay.charts[ci].map;
            for x in 0..rs.len() {
                for y in (x + 1)..rs.len() {
                    let (ea, eb) = (&lay.edges[rs[x]], &lay.edges[rs[y]]);
                    if ea.a == eb.a || ea.a == eb.b || ea.b == eb.a || ea.b == eb.b {
                        continue;
                    }
                    let (p, p2) = (m.to2(lay.nodes[ea.a]), m.to2(lay.nodes[ea.b]));
                    let (q, q2) = (m.to2(lay.nodes[eb.a]), m.to2(lay.nodes[eb.b]));
                    let (r, s) = (p2 - p, q2 - q);
                    let den = r.perp_dot(s);
                    if den.abs() < 1e-14 {
                        continue;
                    }
                    let t = (q - p).perp_dot(s) / den;
                    let u = (q - p).perp_dot(r) / den;
                    if t > 1e-6 && t < 1.0 - 1e-6 && u > 1e-6 && u < 1.0 - 1e-6 {
                        let x3 = m.to3(p + r * t);
                        let (a, b) = (rs[x], rs[y]);
                        let n = lay.split(a, x3);
                        let x3 = lay.nodes[n];
                        lay.split(b, x3);
                        continue 'again;
                    }
                }
            }
            break;
        }
    }

    // 6. patches: faces of the line graph in each chart
    for ci in 0..nc {
        // DEBUG_FACE="prim face path": that chart's lines, for a drawing
        if let Ok(v) = std::env::var("DEBUG_FACE") {
            let w: Vec<&str> = v.split_whitespace().collect();
            if w.len() == 3 && w[0].parse() == Ok(lay.charts[ci].id.prim) && w[1].parse() == Ok(lay.charts[ci].id.face) {
                let m = lay.charts[ci].map;
                let mut out = String::new();
                for lp in &lay.bnd[ci] {
                    for &(e, _) in lp {
                        let p = lay.pts2(e, ci);
                        out += &format!("b {}\n", p.iter().map(|q| format!("{} {}", q.x, q.y)).collect::<Vec<_>>().join(" "));
                    }
                }
                for &r in &lay.rays[ci] {
                    if lay.edges[r].alive {
                        let (a, b) = (m.to2(lay.nodes[lay.edges[r].a]), m.to2(lay.nodes[lay.edges[r].b]));
                        out += &format!("r {} {} {} {}\n", a.x, a.y, b.x, b.y);
                    }
                }
                for &g in &lay.inner[ci] {
                    let q = m.to2(lay.nodes[g]);
                    out += &format!("g {} {}\n", q.x, q.y);
                }
                let _ = std::fs::write(w[2], out);
            }
        }
    }
    let mut patches: Vec<(usize, [Vec<(usize, bool)>; 4])> = vec![];
    for ci in 0..nc {
        let mut hes: Vec<(usize, bool, bool)> = vec![]; // (edge, fwd, inside)
        for lp in &lay.bnd[ci] {
            for &(e, f) in lp {
                hes.push((e, f, true));
                hes.push((e, !f, false));
            }
        }
        for &r in &lay.rays[ci] {
            hes.push((r, true, true));
            hes.push((r, false, true));
        }
        if hes.is_empty() {
            continue;
        }
        let from = |h: &(usize, bool, bool)| if h.1 { lay.edges[h.0].a } else { lay.edges[h.0].b };
        let to = |h: &(usize, bool, bool)| if h.1 { lay.edges[h.0].b } else { lay.edges[h.0].a };
        let mut out_at: HashMap<usize, Vec<(f64, usize)>> = HashMap::new();
        for (i, h) in hes.iter().enumerate() {
            let d = lay.start_dir(h.0, h.1, ci);
            out_at.entry(from(h)).or_default().push((d.y.atan2(d.x), i));
        }
        for v in out_at.values_mut() {
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        let twin = |i: usize| -> usize { hes.iter().position(|h| h.0 == hes[i].0 && h.1 != hes[i].1).unwrap() };
        let mut seen = vec![false; hes.len()];
        for s in 0..hes.len() {
            if seen[s] {
                continue;
            }
            let mut cyc = vec![];
            let mut cur = s;
            let ok;
            loop {
                if seen[cur] {
                    ok = cur == s;
                    break;
                }
                seen[cur] = true;
                cyc.push(cur);
                let t = twin(cur);
                let list = &out_at[&to(&hes[cur])];
                let p = list.iter().position(|x| x.1 == t).unwrap();
                cur = list[(p + list.len() - 1) % list.len()].1;
            }
            if !ok || cyc.iter().any(|&h| !hes[h].2) {
                continue;
            }
            // corners: where the boundary turns
            let k = cyc.len();
            let mut corners = vec![];
            let mut area = 0.0;
            for i in 0..k {
                let (a, b) = (hes[cyc[(i + k - 1) % k]], hes[cyc[i]]);
                let t = turn(lay.end_dir(a.0, a.1, ci), lay.start_dir(b.0, b.1, ci));
                if t > 0.5 || forced.contains(&(ci, from(&b))) {
                    corners.push(i);
                }
                let p = lay.hpts(b.0, b.1, ci);
                for w in p.windows(2) {
                    area += w[0].perp_dot(w[1]);
                }
            }
            if area <= 0.0 {
                return Err("face avec un trou non relié".into());
            }
            if corners.len() != 4 && std::env::var("DEBUG_LAYOUT").is_ok() {
                eprintln!("chart {:?}: {} corners", lay.charts[ci].id, corners.len());
                for i in 0..k {
                    let (a, b) = (hes[cyc[(i + k - 1) % k]], hes[cyc[i]]);
                    let t = turn(lay.end_dir(a.0, a.1, ci), lay.start_dir(b.0, b.1, ci));
                    let n0 = from(&b);
                    eprintln!("  he e{} fwd {} ray {} from n{} {:?} -> n{} turn {:.1}°", b.0, b.1, lay.edges[b.0].ray, n0, lay.nodes[n0], to(&b), t.to_degrees());
                }
                eprintln!("  origins {:?}", lay.origins);
            }
            if corners.len() != 4 {
                if let Ok(path) = std::env::var("DEBUG_CHART") {
                    // the chart's lines (2D), for a drawing: boundary, loops, the patch
                    let m = lay.charts[ci].map;
                    let mut out = String::new();
                    for lp in &lay.bnd[ci] {
                        for &(e, _) in lp {
                            let p = lay.pts2(e, ci);
                            out += &format!("b {}\n", p.iter().map(|q| format!("{} {}", q.x, q.y)).collect::<Vec<_>>().join(" "));
                        }
                    }
                    for &r in &lay.rays[ci] {
                        if lay.edges[r].alive {
                            let (a, b) = (m.to2(lay.nodes[lay.edges[r].a]), m.to2(lay.nodes[lay.edges[r].b]));
                            out += &format!("r {} {} {} {}\n", a.x, a.y, b.x, b.y);
                        }
                    }
                    for &h in &cyc {
                        let p = lay.hpts(hes[h].0, hes[h].1, ci);
                        out += &format!("p {}\n", p.iter().map(|q| format!("{} {}", q.x, q.y)).collect::<Vec<_>>().join(" "));
                    }
                    for &g in &lay.inner[ci] {
                        let q = m.to2(lay.nodes[g]);
                        out += &format!("g {} {}\n", q.x, q.y);
                    }
                    let _ = std::fs::write(path, out);
                }
                return Err(format!("patch à {} coins sur {:?}", corners.len(), lay.charts[ci].id));
            }
            let mut sides: [Vec<(usize, bool)>; 4] = Default::default();
            for (si, side) in sides.iter_mut().enumerate() {
                let (c0, c1) = (corners[si], corners[(si + 1) % 4]);
                let mut i = c0;
                while i != c1 {
                    side.push((hes[cyc[i]].0, hes[cyc[i]].1));
                    i = (i + 1) % k;
                }
            }
            patches.push((ci, sides));
        }
    }

    // 7. segments per line: opposite sides of every patch agree
    let ne = lay.edges.len();
    let mut count = vec![0usize; ne];
    let mut fixed = vec![false; ne];
    let mut minc = vec![1usize; ne];
    // (segments around a cylinder that stay evenly spaced, as the wall's
    // columns, though their count is free)
    let mut even = vec![false; ne];
    let mut target = vec![1.0f64; ne];
    for e in 0..ne {
        let ed = &lay.edges[e];
        if !ed.alive {
            continue;
        }
        let len: f64 = ed.pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        let ray_len = if ed.ray {
            // a loop across a strip follows its arc
            let m = lay.charts[ed.charts[0]].map;
            let (a, b) = (m.to2(lay.nodes[ed.a]), m.to2(lay.nodes[ed.b]));
            a.distance(b)
        } else {
            len
        };
        target[e] = ray_len / st.size;
        count[e] = (target[e].round() as usize).max(1);
        for &c in &ed.charts {
            let ch = &lay.charts[c];
            if ch.strip.is_some() || ch.ring.is_some() {
                // rows of a fillet, or segments around a cylinder (chord)
                let m = ch.map;
                let d = m.to2(lay.nodes[ed.b]) - m.to2(lay.nodes[ed.a]);
                let soft = ch.ring.is_some() && notched.contains(&ch.id.prim);
                if d.y.abs() > d.x.abs() {
                    let rows = d.y.abs() / ch.row();
                    count[e] = (if ch.strip.is_some() { rows.round() } else { (rows - 0.1).ceil() } as usize).max(1);
                    if soft {
                        minc[e] = minc[e].max(count[e]);
                        even[e] = true;
                    } else {
                        fixed[e] = true;
                    }
                } else if let (Map::Tor { .. }, Some(row)) = (m, ch.ring) {
                    // along a cylinder's rounded rim: the wall's segments,
                    // on both rails (the cap's side as the wall's)
                    count[e] = ((d.x.abs() / row - 0.1).ceil() as usize).max(1);
                    if soft {
                        minc[e] = minc[e].max(count[e]);
                        even[e] = true;
                    } else {
                        fixed[e] = true;
                    }
                }
            }
        }
    }
    let sum = |side: &Vec<(usize, bool)>, count: &Vec<usize>| -> usize { side.iter().map(|x| count[x.0]).sum() };
    let mut it = 0;
    let mut settled = true;
    loop {
        it += 1;
        if it > 20_000 {
            // T junctions tying faces together: the search below
            settled = false;
            break;
        }
        let mut bad = None;
        for (_, s) in &patches {
            for (a, b) in [(0, 2), (1, 3)] {
                let (na, nb) = (sum(&s[a], &count), sum(&s[b], &count));
                if na != nb {
                    bad = Some(if na < nb { (s[a].clone(), s[b].clone()) } else { (s[b].clone(), s[a].clone()) });
                    break;
                }
            }
            if bad.is_some() {
                break;
            }
        }
        let Some((small, big)) = bad else { break };
        let up = small.iter().filter(|x| !fixed[x.0]).max_by(|x, y| (target[x.0] / count[x.0] as f64).total_cmp(&(target[y.0] / count[y.0] as f64)));
        if let Some(&(e, _)) = up {
            count[e] += 1;
            continue;
        }
        let down = big.iter().filter(|x| !fixed[x.0] && count[x.0] > minc[x.0]).min_by(|x, y| (target[x.0] / count[x.0] as f64).total_cmp(&(target[y.0] / count[y.0] as f64)));
        match down {
            Some(&(e, _)) => count[e] -= 1,
            None => {
                settled = false;
                break;
            }
        }
    }

    if let (false, Ok(path)) = (settled, std::env::var("DEBUG_COUNTS")) {
        // the count system, to look at it elsewhere: one line per patch,
        // "chart|side0|side1|side2|side3", a side "e:count:fixed:target,..."
        let mut out = String::new();
        for (ci, sd) in &patches {
            out += &format!("{:?}", lay.charts[*ci].id);
            for side in sd {
                out += "|";
                let v: Vec<String> = side
                    .iter()
                    .map(|y| {
                        let (a, b) = (lay.nodes[lay.edges[y.0].a], lay.nodes[lay.edges[y.0].b]);
                        format!("{}:{}:{}:{:.2}:({:.1} {:.1} {:.1})-({:.1} {:.1} {:.1})", y.0, count[y.0], fixed[y.0] as u8, target[y.0], a.x, a.y, a.z, b.x, b.y, b.z)
                    })
                    .collect();
                out += &v.join(",");
            }
            out += "\n";
        }
        let _ = std::fs::write(path, out);
    }
    if !settled {
        min_conflicts(&patches, &mut count, &fixed, &minc, &target).map_err(|why| {
            if std::env::var("DEBUG_LAYOUT").is_ok() {
                eprintln!("{why}");
                for (ci, sd) in &patches {
                    let sm: Vec<usize> = sd.iter().map(|x| x.iter().map(|y| count[y.0]).sum()).collect();
                    if sm[0] != sm[2] || sm[1] != sm[3] {
                        eprintln!("  {:?} sommes {:?}", lay.charts[*ci].id, sm);
                        for (k, side) in sd.iter().enumerate() {
                            let d: Vec<String> = side.iter().map(|y| format!("e{}:{}{} {:.1}", y.0, count[y.0], if fixed[y.0] { "F" } else { "" }, target[y.0])).collect();
                            let n0 = if side[0].1 { lay.edges[side[0].0].a } else { lay.edges[side[0].0].b };
                            eprintln!("    côté {k} depuis {:?} : {}", lay.nodes[n0], d.join(" "));
                        }
                    }
                }
            }
            "nombres de segments impossibles à accorder".to_string()
        })?;
    }

    // 8. where the vertices sit on every line: the free lines take the
    // spacing of the line across their patch (fillet rows run on straight)
    let elen: Vec<f64> = (0..ne)
        .map(|e| {
            let ed = &lay.edges[e];
            if !ed.alive {
                0.0
            } else if ed.ray {
                let m = lay.charts[ed.charts[0]].map;
                m.to2(lay.nodes[ed.a]).distance(m.to2(lay.nodes[ed.b]))
            } else {
                ed.pts.windows(2).map(|w| w[0].distance(w[1])).sum()
            }
        })
        .collect();
    let mut par: Vec<Vec<f64>> = (0..ne).map(|e| (1..count[e]).map(|i| i as f64 / count[e] as f64).collect()).collect();
    // positions along a side (corner to corner), 0..1
    let side_par = |side: &Vec<(usize, bool)>, par: &Vec<Vec<f64>>| -> Vec<f64> {
        let mut out = vec![0.0];
        let mut at = 0.0;
        for &(e, f) in side {
            let mut t: Vec<f64> = par[e].clone();
            if !f {
                t = t.iter().rev().map(|x| 1.0 - x).collect();
            }
            for x in t {
                out.push(at + x * elen[e]);
            }
            at += elen[e];
            out.push(at);
        }
        let tot = at.max(1e-30);
        out.iter().map(|x| x / tot).collect()
    };
    for _ in 0..60 {
        let mut acc: Vec<Vec<(f64, f64)>> = par.iter().map(|v| vec![(0.0, 0.0); v.len()]).collect();
        for (_, sides) in &patches {
            for (a, b) in [(0usize, 2usize), (2, 0), (1, 3), (3, 1)] {
                // with diagonal hooks the fillet rows stay in their corner
                if st.diagonal && sides[a].iter().any(|x| fixed[x.0]) {
                    continue;
                }
                let pa = side_par(&sides[a], &par);
                let n = pa.len() - 1;
                let tot: f64 = sides[b].iter().map(|x| elen[x.0]).sum();
                let mut start = 0usize;
                let mut before = 0.0;
                for &(e, f) in &sides[b] {
                    let c = count[e];
                    for k in 1..c {
                        let j = start + k;
                        let want = (1.0 - pa[n - j]) * tot - before;
                        let mut t = want / elen[e].max(1e-30);
                        let mut idx = k - 1;
                        if !f {
                            t = 1.0 - t;
                            idx = c - k - 1;
                        }
                        acc[e][idx].0 += t.clamp(0.0, 1.0);
                        acc[e][idx].1 += 1.0;
                    }
                    start += c;
                    before += elen[e];
                }
            }
        }
        for e in 0..ne {
            if fixed[e] || even[e] || par[e].is_empty() {
                continue;
            }
            let c = count[e] as f64;
            for (k, (s, w)) in acc[e].iter().enumerate() {
                if *w > 0.0 {
                    par[e][k] = 0.5 * par[e][k] + 0.5 * s / w;
                }
            }
            // keep the order, with a minimal spacing
            let gap = 0.15 / c;
            for k in 0..par[e].len() {
                let lo_k = if k == 0 { gap } else { par[e][k - 1] + gap };
                par[e][k] = par[e][k].max(lo_k);
            }
            for k in (0..par[e].len()).rev() {
                let hi_k = if k + 1 == par[e].len() { 1.0 - gap } else { par[e][k + 1] - gap };
                par[e][k] = par[e][k].min(hi_k);
            }
        }
    }

    // vertices on every line, then a grid in every patch
    let mut pos: Vec<DVec3> = lay.nodes.clone();
    let mut samples: Vec<Vec<u32>> = vec![vec![]; ne];
    for e in 0..ne {
        let ed = &lay.edges[e];
        if !ed.alive {
            continue;
        }
        let mut ids = vec![ed.a as u32];
        if ed.ray {
            let m = lay.charts[ed.charts[0]].map;
            let (a, b) = (m.to2(lay.nodes[ed.a]), m.to2(lay.nodes[ed.b]));
            for &t in &par[e] {
                pos.push(m.to3(a.lerp(b, t)));
                ids.push(pos.len() as u32 - 1);
            }
        } else {
            // across a fillet strip (its end, a miter): evenly in the arc,
            // so that the rows run on straight; else evenly in length
            // (a strip it runs across, not along: the rail between a strip
            // and a cylinder wall is fixed by the wall, along the strip)
            let strip = ed
                .charts
                .iter()
                .copied()
                .find(|&c| {
                    let m = lay.charts[c].map;
                    let d = m.to2(lay.nodes[ed.b]) - m.to2(lay.nodes[ed.a]);
                    lay.charts[c].strip.is_some() && d.y.abs() > d.x.abs()
                })
                .filter(|_| fixed[e]);
            let cum: Vec<f64> = std::iter::once(0.0)
                .chain(ed.pts.windows(2).scan(0.0, |s, w| {
                    *s += match strip {
                        Some(c) => {
                            let m = lay.charts[c].map;
                            (m.to2(w[1]).y - m.to2(w[0]).y).abs()
                        }
                        None => w[0].distance(w[1]),
                    };
                    Some(*s)
                }))
                .collect();
            let total = *cum.last().unwrap();
            for &tp in &par[e] {
                let t = total * tp;
                let k = cum.partition_point(|&c| c < t).clamp(1, cum.len() - 1);
                let w = (t - cum[k - 1]) / (cum[k] - cum[k - 1]).max(1e-30);
                pos.push(ed.pts[k - 1].lerp(ed.pts[k], w));
                ids.push(pos.len() as u32 - 1);
            }
        }
        ids.push(ed.b as u32);
        samples[e] = ids;
    }
    let mut faces: Vec<Vec<u32>> = vec![];
    let mut roles: Vec<u32> = vec![];
    for (ci, sides) in &patches {
        let ch = &lay.charts[*ci];
        let side_ids = |k: usize| -> Vec<u32> {
            let mut v: Vec<u32> = vec![];
            for &(e, f) in &sides[k] {
                let mut s = samples[e].clone();
                if !f {
                    s.reverse();
                }
                if !v.is_empty() {
                    s.remove(0);
                }
                v.extend(s);
            }
            v
        };
        let (s0, s1, mut s2, mut s3) = (side_ids(0), side_ids(1), side_ids(2), side_ids(3));
        s2.reverse();
        s3.reverse();
        let (n, m) = (s0.len() - 1, s1.len() - 1);
        if s2.len() != n + 1 || s3.len() != m + 1 {
            return Err("patch mal accordé".into());
        }
        let q2: HashMap<u32, DVec2> = s0.iter().chain(&s1).chain(&s2).chain(&s3).map(|&v| (v, ch.map.to2(pos[v as usize]))).collect();
        let q = |v: u32| q2[&v];
        let params = |s: &[u32]| -> Vec<f64> {
            let mut c = vec![0.0];
            for w in s.windows(2) {
                c.push(c.last().unwrap() + q(w[0]).distance(q(w[1])));
            }
            let t = c.last().copied().unwrap_or(1.0).max(1e-30);
            c.iter().map(|x| x / t).collect()
        };
        let (ub, ut, vl, vr) = (params(&s0), params(&s2), params(&s3), params(&s1));
        let (b0, bn, t0, tn) = (q(s0[0]), q(s0[n]), q(s2[0]), q(s2[n]));
        let mut grid = vec![vec![0u32; m + 1]; n + 1];
        for i in 0..=n {
            grid[i][0] = s0[i];
            grid[i][m] = s2[i];
        }
        for j in 0..=m {
            grid[0][j] = s3[j];
            grid[n][j] = s1[j];
        }
        for i in 1..n {
            for j in 1..m {
                let (mut u, mut v) = ((ub[i] + ut[i]) * 0.5, (vl[j] + vr[j]) * 0.5);
                for _ in 0..4 {
                    u = (1.0 - v) * ub[i] + v * ut[i];
                    v = (1.0 - u) * vl[j] + u * vr[j];
                }
                let p = q(s0[i]) * (1.0 - v) + q(s2[i]) * v + q(s3[j]) * (1.0 - u) + q(s1[j]) * u
                    - (b0 * ((1.0 - u) * (1.0 - v)) + bn * (u * (1.0 - v)) + t0 * ((1.0 - u) * v) + tn * (u * v));
                pos.push(ch.map.to3(p));
                grid[i][j] = pos.len() as u32 - 1;
            }
        }
        let role = if ch.strip.is_some() || ch.id.face >= BLEND0 { P_FILLET } else { P_FILL };
        for i in 0..n {
            for j in 0..m {
                faces.push(vec![grid[i][j], grid[i + 1][j], grid[i + 1][j + 1], grid[i][j + 1]]);
                roles.push(role);
            }
        }
    }
    if faces.is_empty() {
        return Err("aucune face".into());
    }

    // 9. compact, orient outward, check
    let mut remap = vec![u32::MAX; pos.len()];
    let mut cpos = vec![];
    for f in &mut faces {
        for v in f.iter_mut() {
            if remap[*v as usize] == u32::MAX {
                remap[*v as usize] = cpos.len() as u32;
                cpos.push(pos[*v as usize]);
            }
            *v = remap[*v as usize];
        }
    }
    orient(&cpos, &mut faces, &field);
    let mut b = Baked { positions: cpos, ..Default::default() };
    for (f, r) in faces.into_iter().zip(roles) {
        b.faces.push(BFace { verts: f, normals: vec![], mat: 0, patch: r });
    }
    let bs = BakeSettings { tol: st.tol, threads: st.threads, divisions: 0.0 };
    progress.set(3, b.faces.len() as u32);
    finish(&field, &mut b, &bs, progress);
    if b.stats.open_edges > 0 || b.stats.nonmanifold_edges > 0 {
        return Err(format!("maillage mal fermé ({} bords ouverts)", b.stats.open_edges + b.stats.nonmanifold_edges));
    }
    if b.stats.max_dev > (st.size * 0.25).max(0.2) {
        return Err(format!("écart de {:.2} mm à la surface", b.stats.max_dev).replace('.', ","));
    }
    b.stats.patches = patches.len();
    b.stats.quad_size = st.size;
    b.stats.seconds = t0.elapsed().as_secs_f64();
    let _ = lay.field;
    Ok(b)
}

/// Turn of the boundary of chart `c` at node `n` (None when `n` is not on it).
fn corner_angle(lay: &Layout, c: usize, n: usize) -> Option<f64> {
    for lp in &lay.bnd[c] {
        let k = lp.len();
        for i in 0..k {
            let (ea, fa) = lp[i];
            let end = if fa { lay.edges[ea].b } else { lay.edges[ea].a };
            if end == n {
                let (eb, fb) = lp[(i + 1) % k];
                return Some(turn(lay.end_dir(ea, fa, c), lay.start_dir(eb, fb, c)));
            }
        }
    }
    None
}

/// Direction square to the boundary of chart `c` at node `n`, into the face.
fn inward(lay: &Layout, c: usize, n: usize) -> Option<DVec2> {
    for lp in &lay.bnd[c] {
        let k = lp.len();
        for i in 0..k {
            let (ea, fa) = lp[i];
            let end = if fa { lay.edges[ea].b } else { lay.edges[ea].a };
            if end == n {
                let (eb, fb) = lp[(i + 1) % k];
                // exact tangents (the polyline drifts where an arc meets a
                // line, and a loop square to it would drift across the face)
                let t = (lay.tangent(ea, fa, c, true) + lay.tangent(eb, fb, c, false)).normalize_or_zero();
                return (t != DVec2::ZERO).then(|| perp(t));
            }
        }
    }
    None
}

/// Faces oriented consistently over shared edges, then outward.
fn orient(pos: &[DVec3], faces: &mut [Vec<u32>], field: &Field) {
    let nf = faces.len();
    let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, f) in faces.iter().enumerate() {
        for k in 0..f.len() {
            let (a, b) = (f[k], f[(k + 1) % f.len()]);
            edge_faces.entry((a.min(b), a.max(b))).or_default().push(i);
        }
    }
    let has_dir = |f: &Vec<u32>, a: u32, b: u32| (0..f.len()).any(|k| f[k] == a && f[(k + 1) % f.len()] == b);
    let mut done = vec![false; nf];
    for s in 0..nf {
        if done[s] {
            continue;
        }
        done[s] = true;
        let mut comp = vec![s];
        let mut stack = vec![s];
        while let Some(i) = stack.pop() {
            let f = faces[i].clone();
            for k in 0..f.len() {
                let (a, b) = (f[k], f[(k + 1) % f.len()]);
                for &j in &edge_faces[&(a.min(b), a.max(b))] {
                    if done[j] {
                        continue;
                    }
                    if has_dir(&faces[j], a, b) {
                        faces[j].reverse();
                    }
                    done[j] = true;
                    comp.push(j);
                    stack.push(j);
                }
            }
        }
        let mut vote = 0.0;
        for &i in &comp {
            let f = &faces[i];
            let mut n = DVec3::ZERO;
            let mut c = DVec3::ZERO;
            for k in 0..f.len() {
                n += pos[f[k] as usize].cross(pos[f[(k + 1) % f.len()] as usize]);
                c += pos[f[k] as usize];
            }
            vote += n.dot(field.grad(c / f.len() as f64)).signum();
        }
        if vote < 0.0 {
            for &i in &comp {
                faces[i].reverse();
            }
        }
    }
}

/// Segments per line so that opposite sides of every patch agree, from the
/// current counts: a min-conflicts search (moves of one segment on a free
/// line of an unbalanced patch, the one that leaves the fewest unbalanced
/// sides, then the closest to the line's target), with a short tabu so it
/// does not undo its last moves.
fn min_conflicts(patches: &[(usize, [Vec<(usize, bool)>; 4])], count: &mut [usize], fixed: &[bool], minc: &[usize], target: &[f64]) -> Result<(), String> {
    let ne = count.len();
    let mut cons: Vec<(Vec<usize>, Vec<usize>)> = vec![];
    for (_, sd) in patches {
        for (a, b) in [(0, 2), (1, 3)] {
            cons.push((sd[a].iter().map(|x| x.0).collect(), sd[b].iter().map(|x| x.0).collect()));
        }
    }
    let mut of_edge: Vec<Vec<(usize, i64)>> = vec![vec![]; ne];
    for (i, (a, b)) in cons.iter().enumerate() {
        for &e in a {
            of_edge[e].push((i, 1));
        }
        for &e in b {
            of_edge[e].push((i, -1));
        }
    }
    let side = |s: &Vec<usize>, count: &[usize]| -> i64 { s.iter().map(|&e| count[e] as i64).sum() };
    let mut v: Vec<i64> = cons.iter().map(|c| side(&c.0, count) - side(&c.1, count)).collect();
    let cost = |e: usize, n: usize| ((n as f64) - target[e]).powi(2) / target[e].max(1.0);
    let mut tabu: Vec<(usize, i64)> = vec![(0, 0); ne];
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    for it in 1..400_000usize {
        let bad: Vec<usize> = (0..cons.len()).filter(|&i| v[i] != 0).collect();
        if bad.is_empty() {
            return Ok(());
        }
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let c = bad[(seed >> 33) as usize % bad.len()];
        let sgn = v[c].signum();
        let mut best: Option<(i64, f64, usize, i64)> = None;
        for (list, s) in [(&cons[c].0, 1i64), (&cons[c].1, -1i64)] {
            for &e in list {
                if fixed[e] {
                    continue;
                }
                let delta = -sgn * s;
                if (count[e] as i64 + delta) < minc[e] as i64 || (tabu[e].0 > it && tabu[e].1 == -delta) {
                    continue;
                }
                let dv: i64 = of_edge[e].iter().map(|&(ci, s2)| (v[ci] + s2 * delta).abs() - v[ci].abs()).sum();
                let dc = cost(e, (count[e] as i64 + delta) as usize) - cost(e, count[e]);
                if best.is_none_or(|b| (dv, dc) < (b.0, b.1)) {
                    best = Some((dv, dc, e, delta));
                }
            }
        }
        let Some((_, _, e, delta)) = best else {
            if cons[c].0.iter().chain(&cons[c].1).all(|&e| fixed[e]) {
                return Err(format!("côtés fixés différents ({} vs {})", side(&cons[c].0, count), side(&cons[c].1, count)));
            }
            continue;
        };
        count[e] = (count[e] as i64 + delta) as usize;
        for &(ci, s2) in &of_edge[e] {
            v[ci] += s2 * delta;
        }
        tabu[e] = (it + 7, delta);
    }
    let left = v.iter().filter(|x| **x != 0).count();
    Err(format!("{left} côtés encore différents"))
}
