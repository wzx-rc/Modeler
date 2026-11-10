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
        let l = self.local(p);
        let mut best = (f64::INFINITY, 0u32);
        if self.pr.kind == 1 {
            for f in [CYL_SIDE, CYL_TOP, CYL_BOTTOM] {
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
            Cons::Prev => {
                let sh = &self.shapes[ch.shape];
                let d = self.prev[ch.shape].as_ref().map(|f| f.dist(ch.map.to3(q))).unwrap_or(1e9);
                if sh.carve {
                    -d
                } else {
                    // (a junction fillet starts k away from the model)
                    d - sh.k
                }
            }
            Cons::Later(j) => self.shapes[j].d(ch.map.to3(q)) - self.shapes[j].k,
        }
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
        if f.face != CYL_SIDE {
            return f;
        }
        match self.walls.iter().find(|w| w.0 == f.prim) {
            Some(&(_, m @ Map::Cyl { r, .. })) if m.to2(p).y.abs() > FRAC_PI_2 * r => FaceId { prim: f.prim, face: CYL_SIDE_B },
            _ => f,
        }
    }

    fn label(&self, ch: &Chart, k: Cons, q: DVec2) -> FaceId {
        let f = self.label_raw(ch, k, q);
        match k {
            Cons::Rect(_) | Cons::Arc(_) | Cons::Oct(_) => f,
            _ => self.half(f, ch.map.to3(q)),
        }
    }

    fn label_raw(&self, ch: &Chart, k: Cons, q: DVec2) -> FaceId {
        match k {
            Cons::Rect(s) => ch.side[s],
            Cons::Arc(i) => ch.arcs[i].3,
            Cons::Disc => ch.disc.map(|d| d.1).unwrap_or(ch.id),
            Cons::Oct(i) => ch.ball.map(|b| b.1[i]).unwrap_or(ch.id),
            Cons::Prev => {
                let p = ch.map.to3(q);
                let me = &self.shapes[ch.shape];
                let s = self.shapes.iter().filter(|s| s.step < me.step).min_by(|a, b| a.d(p).abs().total_cmp(&b.d(p).abs())).unwrap();
                let f = FaceId { prim: s.prim, face: s.face_at(p) };
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
                FaceId { prim: self.shapes[j].prim, face: self.shapes[j].face_at(p) }
            }
        }
    }

    /// The function whose zero set carries a piece of boundary.
    fn psi(&self, ch: &Chart, k: Cons, l: FaceId, q: DVec2) -> f64 {
        match k {
            Cons::Rect(_) | Cons::Arc(_) | Cons::Disc | Cons::Oct(_) => self.cval(ch, k, q),
            _ => self.across(ch.id, l, ch.map.to3(q)),
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
        let f = |x: DVec2| self.across(me, id, map.to3(x));
        let t = perp(grad2(&f, q, self.node_tol * 5.0)).normalize_or_zero();
        if t == DVec2::ZERO {
            rough
        } else if t.dot(rough) < 0.0 {
            -t
        } else {
            t
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
        if pr.kind == 1 && (pr.round > 0.0 || pr.chamfer > 0.0 || pr.edges.iter().any(|r| *r != 0.0)) {
            return Err("cylindre arrondi pas encore géré".into());
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
            if pr.round > 0.0 || pr.chamfer > 0.0 || pr.edges.iter().any(|r| *r != 0.0) {
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

/// Bake by construction lines.
pub fn build(scene: &Scene, st: &LayoutSettings, progress: &Progress) -> Result<Baked, String> {
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
            let step = 2.0 * (1.0 - (tol_f / rad).min(0.9)).acos();
            charts.push(Chart {
                id: wall,
                shape: si,
                map: Map::Cyl { o, a: am, b: bm, c: cm, r: rad },
                lo: DVec2::new(-h, -PI * rad),
                hi: DVec2::new(h, PI * rad),
                side: [wall, top, wall, bottom],
                arcs: vec![],
                strip: None,
                ring: Some((rad * step).min(st.size)),
                disc: None,
                ball: None,
                cons: cons_base.clone(),
            });
            for (sg, id) in [(1.0, top), (-1.0, bottom)] {
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
                    disc: Some((rad, wall)),
                    ball: None,
                    cons,
                });
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
            // an end on a ball corner stops where the ball starts
            let mut side = side;
            let (mut t0, mut t1) = (-s[ax], s[ax]);
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
                }
            }
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
                cons: cons_base.clone(),
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
    // local x axis, each half a chart of its own.
    for ci in 0..lay.charts.len() {
        let ch = &lay.charts[ci];
        let (Some(_), Map::Cyl { o, a, b, c, r }) = (ch.ring, ch.map) else { continue };
        let (nth, nt) = (720usize, 48usize);
        let mut vis = vec![false; nth];
        for (k, v) in vis.iter_mut().enumerate() {
            let th = -PI + (k as f64 + 0.5) * TAU / nth as f64;
            *v = (0..=nt).any(|j| {
                let q = DVec2::new(ch.lo.x + (ch.hi.x - ch.lo.x) * j as f64 / nt as f64, th * r);
                ch.cons.iter().filter(|k| matches!(k, Cons::Prev | Cons::Later(_))).all(|&k| lay.cval(ch, k, q) > 0.0)
            });
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
        if best.0 < 4 {
            let id = ch.id;
            let rot = |phi: f64| Map::Cyl { o, a, b: b * phi.cos() + c * phi.sin(), c: c * phi.cos() - b * phi.sin(), r };
            let (ma, mb) = (rot(FRAC_PI_2), rot(-FRAC_PI_2));
            let ida = id;
            let idb = FaceId { prim: id.prim, face: CYL_SIDE_B };
            let mut hb = Chart { id: idb, map: mb, lo: DVec2::new(ch.lo.x, -FRAC_PI_2 * r), hi: DVec2::new(ch.hi.x, FRAC_PI_2 * r), side: ch.side, arcs: vec![], cons: ch.cons.clone(), ..*ch };
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
            continue;
        }
        let phi = -PI + (best.1 as f64 + best.0 as f64 * 0.5) * TAU / nth as f64 + PI;
        let (b2, c2) = (b * phi.cos() + c * phi.sin(), c * phi.cos() - b * phi.sin());
        lay.charts[ci].map = Map::Cyl { o, a, b: b2, c: c2, r };
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
            let mut pts: Vec<(DVec2, Cons, FaceId)> = vec![];
            for q in lp {
                let (_, k) = lay.fmin(ch, q);
                let l = lay.label(ch, k, q);
                let q2 = proj1(&|x| lay.psi(ch, k, l, x), q, fd);
                if q2.distance(q) > grid * 2.0 {
                    continue;
                }
                pts.push((q2, k, l));
            }
            let n = pts.len();
            if n < 3 {
                continue;
            }
            // nodes where the curve or the face across changes
            let mut marks: Vec<(usize, DVec2)> = vec![];
            for i in 0..n {
                let (qa, ka, la) = pts[i];
                let (qb, kb, lb) = pts[(i + 1) % n];
                if ka == kb && la == lb {
                    continue;
                }
                let fa = |x: DVec2| lay.psi(ch, ka, la, x);
                let fb = |x: DVec2| lay.psi(ch, kb, lb, x);
                let mut node = solve_cross(&fa, &fb, (qa + qb) * 0.5, fd).filter(|x| x.distance((qa + qb) * 0.5) < grid * 3.0);
                if node.is_none() {
                    // tangent change: bisection on which curve is under the point
                    let (mut s0, mut s1) = (0.0, 1.0);
                    let mut last = qa;
                    for _ in 0..50 {
                        let m = (s0 + s1) * 0.5;
                        let x = qa.lerp(qb, m);
                        let (_, k) = lay.fmin(ch, x);
                        let l = lay.label(ch, k, x);
                        let xp = proj1(&|y| lay.psi(ch, k, l, y), x, fd);
                        if k == ka && l == la {
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
        for (label, pts) in raw {
            let a = lay.node(pts[0]);
            let b = lay.node(*pts.last().unwrap());
            if a == b {
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
                if !matches!(sm, Map::Cyl { .. } | Map::Ring { .. }) {
                    continue;
                }
                let pp = lay.hpts(e, fwd, ci);
                let total: f64 = pp.windows(3).map(|w| turn(w[1] - w[0], w[2] - w[1])).sum();
                let k = (total.abs() / FRAC_PI_2).round() as usize;
                if k < 2 {
                    continue;
                }
                let (qa, qb) = (sm.to2(lay.nodes[lay.edges[e].a]), sm.to2(lay.nodes[lay.edges[e].b]));
                let me = lay.charts[ci].id;
                let Some(sh) = lay.shape_of(me.prim) else { continue };
                let cuts: Vec<DVec3> = (1..k).map(|j| sm.to3(proj1(&|x| sh.face_fn(me.face, sm.to3(x)), qa.lerp(qb, j as f64 / k as f64), fd))).collect();
                let mut cur = e;
                for x in cuts {
                    lay.split(cur, x);
                    cur = lay.edges.len() - 1;
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
    // round caps seen whole (a blind hole's floor, the top of a cylinder):
    // an o-grid, a square in the middle whose corners face the middles of
    // the four quarter arcs, where the wall's lines come in; nothing
    // crosses the disc
    let mut ogrid = vec![false; nc];
    for ci in 0..nc {
        let Some((rad, _)) = lay.charts[ci].disc else { continue };
        if matches!(lay.charts[ci].map, Map::Rev { .. }) {
            continue;
        }
        let loops = lay.bnd[ci].clone();
        if loops.len() != 1 || loops[0].len() != 4 {
            continue;
        }
        let me = lay.charts[ci].id;
        let map = lay.charts[ci].map;
        let quarter = loops[0].iter().all(|&(e, fwd)| {
            let other = lay.edges[e].charts.iter().copied().find(|&c| c != ci).unwrap_or(ci);
            let pp = lay.hpts(e, fwd, ci);
            let total: f64 = pp.windows(3).map(|w| turn(w[1] - w[0], w[2] - w[1])).sum();
            lay.charts[other].id.prim == me.prim && matches!(lay.charts[other].map, Map::Cyl { .. }) && (total.abs() - FRAC_PI_2).abs() < 0.2
        });
        if !quarter {
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
    for ci in 0..nc {
        if lay.charts[ci].strip.is_some() || ogrid[ci] || matches!(lay.charts[ci].map, Map::Rev { .. }) {
            continue;
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
                if planar && (matches!(lay.charts[other].map, Map::Cyl { .. } | Map::Ring { .. }) || rev) && total.abs() > 0.6 {
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
                    let m = lay.split(e, sm.to3(mid));
                    if total < 0.0 && st.diagonal {
                        // concave fillet hooked diagonally: from its middle a
                        // line runs to a grid node one quad away from the
                        // corner, where the two loops of the face cross
                        lay.origins.push(m);
                        let map = lay.charts[ci].map;
                        let (ps, pe) = if fwd { (na, nb) } else { (nb, na) };
                        let (p0, q0) = (map.to2(lay.nodes[ps]), map.to2(lay.nodes[pe]));
                        let (a1, a2) = (t_in.normalize(), -t_out.normalize());
                        // corner where the two walls would meet
                        let den = a1.perp_dot(t_out);
                        if den.abs() < 1e-6 {
                            return Err("fillet entre deux murs parallèles".into());
                        }
                        let s = (q0 - p0).perp_dot(t_out) / den;
                        let x = p0 + a1 * s;
                        // one quad away, nearer when the face ends before
                        // (a hole or a junction close to the face's edge)
                        let mut hook = st.size;
                        let mut g2 = x + (a1 + a2) * hook;
                        for _ in 0..8 {
                            if lay.fmin(&lay.charts[ci], g2).0 > hook * 0.1 {
                                break;
                            }
                            hook *= 0.7;
                            g2 = x + (a1 + a2) * hook;
                        }
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
                                if c > hook * 0.35 && c < hook * 1.6 && best.is_none_or(|b| (c - hook).abs() < (b - hook).abs()) {
                                    best = Some(c);
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
        // sharp re-entrant corners
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
        for (poly, lines) in &keep[ci] {
            if let Some(t) = enter_poly(poly, lines, o, d, snap) {
                if t > extent * 1e-6 && best.is_none_or(|b| t < b.0 - snap) && tee.is_none_or(|x| t < x.0) {
                    tee = Some((t, o + d * t));
                }
            }
        }
        let mut new_tee = false;
        if let Some((t, q)) = tee {
            if stop.is_none_or(|x| t < x.0 + snap) {
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
                if d.y.abs() > d.x.abs() {
                    fixed[e] = true;
                    let rows = d.y.abs() / ch.row();
                    count[e] = (if ch.strip.is_some() { rows.round() } else { (rows - 0.1).ceil() } as usize).max(1);
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
        let down = big.iter().filter(|x| !fixed[x.0] && count[x.0] > 1).min_by(|x, y| (target[x.0] / count[x.0] as f64).total_cmp(&(target[y.0] / count[y.0] as f64)));
        match down {
            Some(&(e, _)) => count[e] -= 1,
            None => {
                settled = false;
                break;
            }
        }
    }

    if !settled {
        min_conflicts(&patches, &mut count, &fixed, &target).map_err(|why| {
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
            if fixed[e] || par[e].is_empty() {
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
            let cum: Vec<f64> = std::iter::once(0.0)
                .chain(ed.pts.windows(2).scan(0.0, |s, w| {
                    *s += w[0].distance(w[1]);
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
fn min_conflicts(patches: &[(usize, [Vec<(usize, bool)>; 4])], count: &mut [usize], fixed: &[bool], target: &[f64]) -> Result<(), String> {
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
                if (count[e] as i64 + delta) < 1 || (tabu[e].0 > it && tabu[e].1 == -delta) {
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
