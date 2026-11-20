//! The document: features applied in order. Add = union with what is
//! there, Carve = subtraction. A feature is a primitive standing on its base:
//! `origin` is the centre of its base, it rises along its local +Y (the
//! rotation `rot` turns local into world). Millimetres, Y up.
//!
//! A sketched feature (`Prim::Profile`) is a closed outline of corners and
//! curve points (`Profile`) drawn on a plane, pulled to a height.
//!
//! A feature can also be a fillet or a chamfer (`round`) of edges of what is
//! built before it. Its edges are named by the tags of the two faces each
//! one joins (`EdgeRef`), so they are found again when a shape moves.

use glam::{DMat4, DQuat, DVec3};
use serde::{Deserialize, Serialize};

pub type FeatureId = u32;

/// Format version written in the file.
pub const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Add,
    Carve,
}

impl Op {
    pub fn label(self) -> &'static str {
        match self {
            Op::Add => "Add",
            Op::Carve => "Carve",
        }
    }
    pub fn other(self) -> Op {
        match self {
            Op::Add => Op::Carve,
            Op::Carve => Op::Add,
        }
    }
}

/// The primitives (sizes in mm, full sizes).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Prim {
    /// Width (local X), height (local Y), depth (local Z).
    Box { x: f64, y: f64, z: f64 },
    Cylinder { radius: f64, height: f64 },
    /// A sketched outline (`Feature::profile`) pulled to a height: the
    /// outline's box width (local X), the height (local Y), its box depth
    /// (local Z). Resizing it stretches the outline.
    Profile { x: f64, y: f64, z: f64 },
    /// A path (Romain 18 nov.: "un path sans épaisseur"): a curve of
    /// corners and curve points (`Feature::profile`, open or closed) with
    /// no thickness, its box width (local X) and depth (local Z). The
    /// kernel builds nothing from it.
    Path { x: f64, z: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrimKind {
    Box,
    Cylinder,
    /// The Sketch tool's outlines.
    Profile,
    /// The Path tool's curves.
    Path,
    /// The Surface tool's sheets (a closed outline filled, no thickness).
    Surface,
}

impl PrimKind {
    pub fn label(self) -> &'static str {
        match self {
            PrimKind::Box => "Cube",
            PrimKind::Cylinder => "Cylinder",
            PrimKind::Profile => "Sketch",
            PrimKind::Path => "Path",
            PrimKind::Surface => "Surface",
        }
    }
    /// Default primitive: 20 mm.
    pub fn default_prim(self) -> Prim {
        match self {
            PrimKind::Box => Prim::Box { x: 20.0, y: 20.0, z: 20.0 },
            PrimKind::Cylinder => Prim::Cylinder { radius: 10.0, height: 20.0 },
            PrimKind::Profile => Prim::Profile { x: 20.0, y: 20.0, z: 20.0 },
            PrimKind::Path | PrimKind::Surface => Prim::Path { x: 20.0, z: 20.0 },
        }
    }
}

impl Prim {
    pub fn kind(&self) -> PrimKind {
        match self {
            Prim::Box { .. } => PrimKind::Box,
            Prim::Cylinder { .. } => PrimKind::Cylinder,
            Prim::Profile { .. } => PrimKind::Profile,
            Prim::Path { .. } => PrimKind::Path,
        }
    }
    pub fn height(&self) -> f64 {
        match *self {
            Prim::Box { y, .. } | Prim::Profile { y, .. } => y,
            Prim::Cylinder { height, .. } => height,
            Prim::Path { .. } => 0.0,
        }
    }
    pub fn set_height(&mut self, h: f64) {
        match self {
            Prim::Box { y, .. } | Prim::Profile { y, .. } => *y = h,
            Prim::Cylinder { height, .. } => *height = h,
            Prim::Path { .. } => {}
        }
    }
    /// Half extents in the local frame, around the base centre: x, height, z.
    pub fn half_xz(&self) -> (f64, f64) {
        match *self {
            Prim::Box { x, z, .. } | Prim::Profile { x, z, .. } | Prim::Path { x, z } => (x * 0.5, z * 0.5),
            Prim::Cylinder { radius, .. } => (radius, radius),
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn yes() -> bool {
    true
}

/// A sketched outline: its points in the unit square [-0.5, 0.5]² of the
/// feature's local X and Z (scaled by the profile's box), each a corner or
/// a curve point. A curve point rounds the outline into a quadratic curve
/// from the middle of its incoming side to the middle of its outgoing one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub points: Vec<[f64; 2]>,
    pub smooth: Vec<bool>,
    /// Each corner's cut (Romain 18 nov.: "Bevel fillet sur un path"): a
    /// fillet radius (> 0) or a chamfer distance (< 0, its size along each
    /// side), in mm; 0 or missing: a sharp corner. Curve points ignore it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corner: Vec<f64>,
    /// Each fillet corner's tension (Romain 19 nov.: "SHIFT maintenu :
    /// tension set"): the conic's rho in (0, 1), the cut's middle at
    /// M + rho (corner − M), M the middle of its two ends; fuller towards 1,
    /// flatter towards 0; 0 or missing: a circular arc. On a curve point:
    /// its curve the conic of that rho (0: the quadratic, rho 0.5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tension: Vec<f64>,
    /// An open path (`Prim::Path` only): no side from the last point back
    /// to the first, and its two ends are plain points.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
    /// A curve through its points (Romain 20 nov., 13 h 05: "créer
    /// proprement une courbe… fermée, et que ça puisse faire une surface"):
    /// the uniform Catmull-Rom of SplitIt's spec (the Surface tool's CURVE),
    /// each span an exact cubic (`spline_spans`); `smooth[i]` false makes
    /// point i sharp (the reflected control there). Corners are not cut.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub spline: bool,
}

/// A corner of an outline cut by a fillet (an arc) or a chamfer (a straight
/// side): where the cut leaves the incoming side, its middle, where it
/// joins the outgoing side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub chamfer: bool,
    pub a: [f64; 2],
    pub m: [f64; 2],
    pub b: [f64; 2],
    /// The size it is made at (smaller than asked when the sides are too short).
    pub size: f64,
    /// The largest size the sides allow.
    pub max: f64,
    /// The outline's corner (the conic's middle control point).
    pub corner: [f64; 2],
    /// A conic (rho in (0, 1)) instead of the circular arc; 0: the arc.
    pub rho: f64,
}

/// The rho of the circular arc at a corner whose sides meet at `theta`
/// (radians): the arc as a conic.
pub fn circle_rho(theta: f64) -> f64 {
    let w = (theta * 0.5).sin();
    w / (1.0 + w)
}

impl Cut {
    /// The conic's rho, the circle's when it is a plain arc.
    pub fn rho_or_circle(&self) -> f64 {
        if self.rho > 0.0 {
            return self.rho;
        }
        let mid = [(self.a[0] + self.b[0]) * 0.5, (self.a[1] + self.b[1]) * 0.5];
        let full = (self.corner[0] - mid[0]).hypot(self.corner[1] - mid[1]);
        if full < 1e-12 { 0.5 } else { (self.m[0] - mid[0]).hypot(self.m[1] - mid[1]) / full }
    }
}

/// The cut of corner `i` of an outline (points already scaled, mm). It
/// takes at most half of each side, so cuts and curve points never cross.
pub fn corner_cut(p: &[[f64; 2]], smooth: &[bool], corner: &[f64], i: usize) -> Option<Cut> {
    let n = p.len();
    let c = corner.get(i).copied().unwrap_or(0.0);
    if n < 3 || c.abs() < 1e-9 || smooth.get(i).copied().unwrap_or(false) {
        return None;
    }
    let q = p[i];
    let (prev, next) = (p[(i + n - 1) % n], p[(i + 1) % n]);
    let u = [prev[0] - q[0], prev[1] - q[1]];
    let w = [next[0] - q[0], next[1] - q[1]];
    let (lu, lw) = ((u[0] * u[0] + u[1] * u[1]).sqrt(), (w[0] * w[0] + w[1] * w[1]).sqrt());
    if lu < 1e-9 || lw < 1e-9 {
        return None;
    }
    let (du, dw) = ([u[0] / lu, u[1] / lu], [w[0] / lw, w[1] / lw]);
    let theta = (du[0] * dw[0] + du[1] * dw[1]).clamp(-1.0, 1.0).acos();
    if theta < 1e-3 || std::f64::consts::PI - theta < 1e-3 {
        return None;
    }
    // at most half of each side: two corners at their most on one side
    // meet at its middle (a slot's round end), the side between them gone
    // (`pieces_t` and `focc_prism_cut` join the two cuts there)
    let tmax = 0.5 * lu.min(lw);
    let at = |t: f64| ([q[0] + du[0] * t, q[1] + du[1] * t], [q[0] + dw[0] * t, q[1] + dw[1] * t]);
    if c < 0.0 {
        let t = (-c).min(tmax);
        let (a, b) = at(t);
        return Some(Cut { chamfer: true, a, m: [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5], b, size: t, max: tmax, corner: q, rho: 0.0 });
    }
    let half = (theta * 0.5).tan();
    let t = (c / half).min(tmax);
    let r = t * half;
    let (a, b) = at(t);
    let bl = ((du[0] + dw[0]).powi(2) + (du[1] + dw[1]).powi(2)).sqrt();
    let bis = [(du[0] + dw[0]) / bl, (du[1] + dw[1]) / bl];
    let d = r / (theta * 0.5).sin();
    let centre = [q[0] + bis[0] * d, q[1] + bis[1] * d];
    let m = [centre[0] - bis[0] * r, centre[1] - bis[1] * r];
    Some(Cut { chamfer: false, a, m, b, size: r, max: tmax * half, corner: q, rho: 0.0 })
}

/// The cut with its tension (`Profile::tension`): a fillet becomes the
/// conic of that rho between the same two ends.
pub fn corner_cut_t(p: &[[f64; 2]], smooth: &[bool], corner: &[f64], tension: &[f64], i: usize, open: bool) -> Option<Cut> {
    let mut c = corner_cut_open(p, smooth, corner, i, open)?;
    let rho = tension.get(i).copied().unwrap_or(0.0);
    if !c.chamfer && rho > 1e-6 {
        let rho = rho.clamp(0.02, 0.98);
        let mid = [(c.a[0] + c.b[0]) * 0.5, (c.a[1] + c.b[1]) * 0.5];
        c.m = [mid[0] + rho * (c.corner[0] - mid[0]), mid[1] + rho * (c.corner[1] - mid[1])];
        c.rho = rho;
    }
    Some(c)
}

/// The same on an outline that may be open: its two ends have no cut.
pub fn corner_cut_open(p: &[[f64; 2]], smooth: &[bool], corner: &[f64], i: usize, open: bool) -> Option<Cut> {
    if open && (i == 0 || i + 1 >= p.len()) {
        return None;
    }
    corner_cut(p, smooth, corner, i)
}

/// Points along a cut: the arc through a, m, b, or the straight chamfer.
fn cut_points(c: &Cut) -> Vec<[f64; 2]> {
    if c.chamfer {
        return vec![c.a, c.b];
    }
    if c.rho > 0.0 {
        // the rational quadratic through a, b, with the corner as its
        // middle control point, weight rho / (1 − rho)
        let w = c.rho / (1.0 - c.rho);
        let k = 16;
        return (0..=k)
            .map(|j| {
                let t = j as f64 / k as f64;
                let (u, v, x) = ((1.0 - t) * (1.0 - t), 2.0 * t * (1.0 - t) * w, t * t);
                let s = u + v + x;
                [(u * c.a[0] + v * c.corner[0] + x * c.b[0]) / s, (u * c.a[1] + v * c.corner[1] + x * c.b[1]) / s]
            })
            .collect();
    }
    // the circle through the three points
    let (a, m, b) = (c.a, c.m, c.b);
    let d = 2.0 * (a[0] * (m[1] - b[1]) + m[0] * (b[1] - a[1]) + b[0] * (a[1] - m[1]));
    if d.abs() < 1e-12 {
        return vec![a, b];
    }
    let sq = |p: [f64; 2]| p[0] * p[0] + p[1] * p[1];
    let ux = (sq(a) * (m[1] - b[1]) + sq(m) * (b[1] - a[1]) + sq(b) * (a[1] - m[1])) / d;
    let uy = (sq(a) * (b[0] - m[0]) + sq(m) * (a[0] - b[0]) + sq(b) * (m[0] - a[0])) / d;
    let r = ((a[0] - ux).powi(2) + (a[1] - uy).powi(2)).sqrt();
    let ang = |p: [f64; 2]| (p[1] - uy).atan2(p[0] - ux);
    let (t0, tm, t1) = (ang(a), ang(m), ang(b));
    // the way round from a to b that passes m
    let wrap = |x: f64| x.rem_euclid(std::f64::consts::TAU);
    let sweep = if wrap(tm - t0) <= wrap(t1 - t0) { wrap(t1 - t0) } else { wrap(t1 - t0) - std::f64::consts::TAU };
    let k = 16;
    (0..=k).map(|j| { let t = t0 + sweep * j as f64 / k as f64; [ux + r * t.cos(), uy + r * t.sin()] }).collect()
}

/// One piece of an outline: a curve around a curve point, or a straight
/// side between two pieces.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    /// 2 i for the curve around point i, 2 i + 1 for the side after it.
    pub id: usize,
    pub pts: Vec<[f64; 2]>,
}

impl Profile {
    /// The points scaled to a box of `w` × `d`.
    pub fn scaled(&self, w: f64, d: f64) -> Vec<[f64; 2]> {
        self.points.iter().map(|p| [p[0] * w, p[1] * d]).collect()
    }
    /// Its pieces, in a box of `w` × `d`, curves sampled (as the kernel
    /// builds them: a curve between the middles of the two sides; a
    /// spline's spans).
    pub fn pieces(&self, w: f64, d: f64) -> Vec<Piece> {
        let p = self.scaled(w, d);
        if self.spline {
            return spline_pieces(&p, &self.smooth, self.open);
        }
        pieces_t(&p, &self.smooth, &self.corner, &self.tension, self.open)
    }
    /// Each point's cut, in a box of `w` × `d` (none on a spline).
    pub fn cuts(&self, w: f64, d: f64) -> Vec<Option<Cut>> {
        let p = self.scaled(w, d);
        if self.spline {
            return vec![None; p.len()];
        }
        (0..p.len()).map(|i| corner_cut_t(&p, &self.smooth, &self.corner, &self.tension, i, self.open)).collect()
    }
    /// A spline's spans in a box of `w` × `d`: span i (point i to the next)
    /// as its two inner Bezier controls.
    pub fn spans(&self, w: f64, d: f64) -> Vec<([f64; 2], [f64; 2])> {
        spline_spans(&self.scaled(w, d), &self.smooth, self.open)
    }
    /// The outline as one polyline (closed: last point = first).
    pub fn polyline(&self, w: f64, d: f64) -> Vec<[f64; 2]> {
        let mut out: Vec<[f64; 2]> = vec![];
        for piece in self.pieces(w, d) {
            for p in piece.pts {
                if out.last().map(|q| (q[0] - p[0]).abs() + (q[1] - p[1]).abs() > 1e-9).unwrap_or(true) {
                    out.push(p);
                }
            }
        }
        if !self.open {
            if let Some(f) = out.first().copied() {
                out.push(f);
            }
        }
        out
    }
    /// Mirrored across local X (the feature turned over to grow downwards).
    /// Mirrored across its local Z axis (x → −x): with the frame turned
    /// half round its Z, the outline stays in place.
    pub fn mirrored(&self) -> Profile {
        Profile { points: self.points.iter().map(|p| [-p[0], p[1]]).collect(), smooth: self.smooth.clone(), corner: self.corner.clone(), tension: self.tension.clone(), open: self.open, spline: self.spline }
    }
    pub fn flipped(&self) -> Profile {
        Profile { points: self.points.iter().map(|p| [p[0], -p[1]]).collect(), smooth: self.smooth.clone(), corner: self.corner.clone(), tension: self.tension.clone(), open: self.open, spline: self.spline }
    }
}

/// The uniform Catmull-Rom through `p` (`Profile::spline`, SplitIt's
/// spec, as the app's `catmull_rom`): for span i (point i to point i + 1)
/// its cubic Bezier's two inner controls, P1 + (P2 − P0) / 6 and
/// P2 − (P3 − P1) / 6; at a sharp point (`smooth[i]` false) and an open
/// curve's ends the reflected control (P0 = 2 P1 − P2), so the curve leaves
/// it along the chord.
pub fn spline_spans(p: &[[f64; 2]], smooth: &[bool], open: bool) -> Vec<([f64; 2], [f64; 2])> {
    let n = p.len();
    if n < 2 || (!open && n < 3) {
        return vec![];
    }
    let sharp = |i: usize| !smooth.get(i).copied().unwrap_or(true);
    let spans = if open { n - 1 } else { n };
    let (add, sub, mul) = (|a: [f64; 2], b: [f64; 2]| [a[0] + b[0], a[1] + b[1]], |a: [f64; 2], b: [f64; 2]| [a[0] - b[0], a[1] - b[1]], |a: [f64; 2], k: f64| [a[0] * k, a[1] * k]);
    (0..spans)
        .map(|i| {
            let (p1, p2) = (p[i], p[(i + 1) % n]);
            let mut p0 = if open { p[i.saturating_sub(1)] } else { p[(i + n - 1) % n] };
            let mut p3 = if open { p[(i + 2).min(n - 1)] } else { p[(i + 2) % n] };
            if sharp(i) || (open && i == 0) {
                p0 = sub(mul(p1, 2.0), p2);
            }
            if sharp((i + 1) % n) || (open && i + 2 == n) {
                p3 = sub(mul(p2, 2.0), p1);
            }
            (add(p1, mul(sub(p2, p0), 1.0 / 6.0)), sub(p2, mul(sub(p3, p1), 1.0 / 6.0)))
        })
        .collect()
}

/// A cubic Bezier at t.
pub fn bezier3(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2], t: f64) -> [f64; 2] {
    let u = 1.0 - t;
    let (k0, k1, k2, k3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [k0 * a[0] + k1 * b[0] + k2 * c[0] + k3 * d[0], k0 * a[1] + k1 * b[1] + k2 * c[1] + k3 * d[1]]
}

/// A spline's pieces: span i is piece 2 i + 1 (a side), sampled.
pub fn spline_pieces(p: &[[f64; 2]], smooth: &[bool], open: bool) -> Vec<Piece> {
    let n = p.len();
    spline_spans(p, smooth, open)
        .into_iter()
        .enumerate()
        .map(|(i, (c1, c2))| {
            let (a, d) = (p[i], p[(i + 1) % n]);
            Piece { id: 2 * i + 1, pts: (0..=24).map(|k| bezier3(a, c1, c2, d, k as f64 / 24.0)).collect() }
        })
        .collect()
}

/// The pieces of a closed outline of points (corners or curve points).
pub fn pieces(p: &[[f64; 2]], smooth: &[bool]) -> Vec<Piece> {
    pieces_cut(p, smooth, &[])
}

/// The same with the corners cut (`corner_cut`): the cut is the corner's
/// piece (id 2 i).
pub fn pieces_cut(p: &[[f64; 2]], smooth: &[bool], corner: &[f64]) -> Vec<Piece> {
    pieces_open(p, smooth, corner, false)
}

/// The same on an outline that may be open (a path): no closing side, and
/// its two ends are plain points.
pub fn pieces_open(p: &[[f64; 2]], smooth: &[bool], corner: &[f64], open: bool) -> Vec<Piece> {
    pieces_t(p, smooth, corner, &[], open)
}

/// The same with each fillet corner's tension.
pub fn pieces_t(p: &[[f64; 2]], smooth: &[bool], corner: &[f64], tension: &[f64], open: bool) -> Vec<Piece> {
    let n = p.len();
    let end = |i: usize| open && (i == 0 || i + 1 == n);
    let mut out = vec![];
    if n < 2 {
        return out;
    }
    let mid = |a: [f64; 2], b: [f64; 2]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    let far = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() + (a[1] - b[1]).abs() > 1e-9;
    let round = |i: usize| !end(i) && smooth.get(i).copied().unwrap_or(false) && far(p[(i + n - 1) % n], p[i]) && far(p[i], p[(i + 1) % n]);
    let cuts: Vec<Option<Cut>> = (0..n).map(|i| corner_cut_t(p, smooth, corner, tension, i, open)).collect();
    // where each point's piece starts and ends (the point itself at a corner)
    let ends = |i: usize| {
        if round(i) {
            (mid(p[(i + n - 1) % n], p[i]), mid(p[i], p[(i + 1) % n]))
        } else if let Some(c) = &cuts[i] {
            (c.a, c.b)
        } else {
            (p[i], p[i])
        }
    };
    for i in 0..n {
        let (a, b) = ends(i);
        if round(i) {
            // a quadratic; with a tension, the conic of that rho (an
            // ellipse's quarter at the circle's rho, `Curve`)
            let rho = tension.get(i).copied().unwrap_or(0.0);
            let wm = if rho > 1e-6 { let r = rho.clamp(0.02, 0.98); r / (1.0 - r) } else { 1.0 };
            let k = 16;
            let pts = (0..=k)
                .map(|j| {
                    let t = j as f64 / k as f64;
                    let (u, v, w) = ((1.0 - t) * (1.0 - t), 2.0 * t * (1.0 - t) * wm, t * t);
                    let s = u + v + w;
                    [(u * a[0] + v * p[i][0] + w * b[0]) / s, (u * a[1] + v * p[i][1] + w * b[1]) / s]
                })
                .collect();
            out.push(Piece { id: 2 * i, pts });
        } else if let Some(c) = &cuts[i] {
            out.push(Piece { id: 2 * i, pts: cut_points(c) });
        }
        if open && i + 1 == n {
            break;
        }
        let (next, _) = ends((i + 1) % n);
        if far(b, next) {
            out.push(Piece { id: 2 * i + 1, pts: vec![b, next] });
        }
    }
    out
}

/// The Curves set (Romain, 20 nov., 8 h 08: "tout un set de composant
/// « curves »… dans le feeling de 3dsMax"): a path made from settings, as
/// 3ds Max's Shapes. Its size is its box (`Prim::Path { x, z }`, the W / D
/// handles, Scale); its outline (`Feature::profile`) is made again from its
/// settings and that box at every change (`Doc::regen_curves`). An edit of
/// its points or corners by hand makes it a plain path (3ds Max's Convert
/// to Editable Spline).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub kind: CurveKind,
    /// Rectangle, Polygon, Star (its outer corners): the corners' fillet
    /// radius in mm (< 0: a chamfer of that size); 0 sharp.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub corner: f64,
    /// Star: its inner corners' fillet.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub corner2: f64,
    /// Polygon: its sides; Star: its points.
    #[serde(default, skip_serializing_if = "is_zero_u")]
    pub sides: u32,
    /// Star: the inner radius over the outer one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub inner: f64,
}

/// The Curves set's 2D booleans (step 2, 3ds Max's Shape Boolean): closed
/// curves on one plane combined, kept live (a curve changed, the result
/// follows). The first operand is the base: Subtract takes the others away
/// from it. The kernel makes it exactly (`build::region`) and sends its
/// edges back (`Model.curves`); the operands stay, drawn dimmer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShapeBool {
    pub op: CurveOp,
    pub operands: Vec<FeatureId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CurveOp {
    Union,
    Subtract,
    Intersect,
}

impl CurveOp {
    pub fn label(self) -> &'static str {
        match self {
            CurveOp::Union => "Union",
            CurveOp::Subtract => "Subtract",
            CurveOp::Intersect => "Intersect",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CurveKind {
    Rectangle,
    Circle,
    Ellipse,
    Polygon,
    Star,
}

fn is_zero_u(x: &u32) -> bool {
    *x == 0
}

impl CurveKind {
    pub fn label(self) -> &'static str {
        match self {
            CurveKind::Rectangle => "Rectangle",
            CurveKind::Circle => "Circle",
            CurveKind::Ellipse => "Ellipse",
            CurveKind::Polygon => "Polygon",
            CurveKind::Star => "Star",
        }
    }
    /// Drawn from its centre out (Circle, Polygon, Star), else corner to
    /// corner (as 3ds Max).
    pub fn from_centre(self) -> bool {
        matches!(self, CurveKind::Circle | CurveKind::Polygon | CurveKind::Star)
    }
    /// Its settings when made.
    pub fn new_curve(self) -> Curve {
        let (sides, inner) = match self {
            CurveKind::Polygon => (6, 0.0),
            CurveKind::Star => (5, 0.5),
            _ => (0, 0.0),
        };
        Curve { kind: self, corner: 0.0, corner2: 0.0, sides, inner }
    }
}

impl Curve {
    /// Points or sides as used (3 to 100; a star 3 to 100 points).
    pub fn n(&self) -> usize {
        (self.sides.max(3) as usize).min(100)
    }
    /// The star's inner radius over its outer one, kept in (0.02, 0.98).
    pub fn ratio(&self) -> f64 {
        if self.inner > 0.0 { self.inner.clamp(0.02, 0.98) } else { 0.5 }
    }
    /// Its corners at radius 1 round its centre, in the plane's (x, z): a
    /// polygon with a flat side towards +z, a star with a point towards −z.
    /// None: a box (Rectangle, Circle, Ellipse).
    pub fn star_points(&self) -> Option<Vec<[f64; 2]>> {
        use std::f64::consts::{FRAC_PI_2, PI, TAU};
        let n = self.n();
        match self.kind {
            CurveKind::Polygon => Some((0..n).map(|k| { let a = FRAC_PI_2 + PI / n as f64 + TAU * k as f64 / n as f64; [a.cos(), a.sin()] }).collect()),
            CurveKind::Star => {
                let r2 = self.ratio();
                Some((0..2 * n).map(|k| { let a = -FRAC_PI_2 + PI * k as f64 / n as f64; let r = if k % 2 == 0 { 1.0 } else { r2 }; [r * a.cos(), r * a.sin()] }).collect())
            }
            _ => None,
        }
    }
    /// At radius 1: its box's size and where its centre lies from the
    /// box's centre (a pentagon's centre is not its box's).
    pub fn unit_box(&self) -> ([f64; 2], [f64; 2]) {
        let Some(p) = self.star_points() else { return ([2.0, 2.0], [0.0, 0.0]) };
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for q in &p {
            for k in 0..2 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
        ([hi[0] - lo[0], hi[1] - lo[1]], [-(lo[0] + hi[0]) * 0.5, -(lo[1] + hi[1]) * 0.5])
    }
    /// Its radius in a box of `x` × `z` (a circle's half width, a polygon's
    /// or star's outer radius; its box stretched: the mean).
    pub fn radius(&self, x: f64, z: f64) -> f64 {
        let (b, _) = self.unit_box();
        0.5 * (x / b[0] + z / b[1])
    }
    /// The largest fillet its outer corners (`inner`: a star's inner
    /// ones) take in a box of `x` × `z` (a polygon's at its most: a circle).
    pub fn corner_max(&self, x: f64, z: f64, inner: bool) -> f64 {
        let mut big = *self;
        big.corner = 1e9;
        big.corner2 = 1e9;
        let pr = big.profile(x, z);
        pr.cuts(x, z).iter().enumerate().filter(|(i, _)| self.kind != CurveKind::Star || (i % 2 == 1) == inner).filter_map(|(_, c)| c.map(|c| c.max)).fold(f64::MAX, f64::min).min(1e6)
    }
    /// The outline in a box of `x` × `z` (the unit square stretched).
    pub fn profile(&self, x: f64, z: f64) -> Profile {
        let square = vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]];
        let plain = |points: Vec<[f64; 2]>, corner: Vec<f64>| { let n = points.len(); Profile { points, smooth: vec![false; n], corner, tension: vec![], open: false, spline: false } };
        match self.kind {
            CurveKind::Rectangle => plain(square, vec![self.corner; 4]),
            CurveKind::Circle | CurveKind::Ellipse => {
                if (x - z).abs() <= 1e-9 * x.abs().max(z.abs()).max(1.0) {
                    // a circle: four exact quarter arcs meeting at the sides' middles
                    plain(square, vec![0.5 * x.min(z); 4])
                } else {
                    // an ellipse: four curve points, each a conic quarter
                    // between the sides' middles (a circle's, stretched)
                    Profile { points: square, smooth: vec![true; 4], corner: vec![], tension: vec![circle_rho(std::f64::consts::FRAC_PI_2); 4], open: false, spline: false }
                }
            }
            CurveKind::Polygon | CurveKind::Star => {
                let p = self.star_points().unwrap_or_default();
                let (b, c) = self.unit_box();
                let points = p.iter().map(|q| [(q[0] + c[0]) / b[0], (q[1] + c[1]) / b[1]]).collect::<Vec<_>>();
                let corner = (0..points.len()).map(|i| if self.kind == CurveKind::Star && i % 2 == 1 { self.corner2 } else { self.corner }).collect();
                plain(points, corner)
            }
        }
    }
}

/// An edge of the part, found again at every build: the packed tags
/// (`model::FaceTag::pack`) of the two faces it joins, in any order, and a
/// point on it to choose between edges joining the same two faces.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeRef {
    pub faces: [i64; 2],
    pub at: DVec3,
}

impl EdgeRef {
    /// The two tags in increasing order.
    pub fn key(&self) -> [i64; 2] {
        let [a, b] = self.faces;
        if a <= b { [a, b] } else { [b, a] }
    }
}

/// A fillet (or a chamfer) of edges of what is built before it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Round {
    /// Fillet radius, or chamfer distance (mm).
    pub size: f64,
    #[serde(default)]
    pub chamfer: bool,
    pub edges: Vec<EdgeRef>,
    /// Blend corners: made in one operation with the fillets just before it
    /// that it touches, so they meet in a rounded corner instead of each
    /// ending on its own (fillets only).
    #[serde(default)]
    pub blend: bool,
    /// Tension (Romain 19 nov.): a fillet's section a conic of this rho in
    /// (0, 1) instead of a circular arc (fuller towards 1); 0: circular.
    /// Straight fillets between flat ends only (`build::round`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tension: f64,
    /// Only Selected (Romain 20 nov., 10 h 54): the picked edges alone,
    /// not the edges tangent to them OpenCascade carries the fillet along
    /// (it ends square where the edge ends); not blended.
    #[serde(default, skip_serializing_if = "is_false")]
    pub only: bool,
}

fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

impl Round {
    pub fn label(&self) -> &'static str {
        if self.chamfer { "Chamfer" } else { "Fillet" }
    }
}

/// A face of the part, found again at every build: its packed tag and a
/// point on it (to choose between faces with the same tag).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FaceRef {
    pub tag: i64,
    pub at: DVec3,
}

/// A flat face pushed or pulled (the Inset and Extrude tools): its
/// outline moved in by `inset` (0: the whole face), then moved along its
/// normal by `depth`: out (> 0, material added), in (< 0, carved), or not
/// at all (0: the inset is only drawn on the face).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Push {
    pub face: FaceRef,
    #[serde(default)]
    pub inset: f64,
    #[serde(default)]
    pub depth: f64,
}

impl Push {
    pub fn label(&self) -> &'static str {
        if self.inset > 0.0 { "Inset" } else { "Extrude" }
    }
}

/// A component of the part (Vertex, Edge and Face modes): found again at
/// every build, a face by its tag and a point, an edge by the tags of its
/// two faces and its middle, a vertex by its point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Comp {
    Face(FaceRef),
    Edge(EdgeRef),
    Vertex(DVec3),
}

/// A component moved by `delta` (direct modelling, Romain 18 nov.:
/// "Manipulation de component"): its vertices move, the faces round them
/// follow.
/// The Knife (Romain 19 nov., "like the Maya Multicut tool") and Point
/// on edge: points cut into the part's edges, and straight lines between
/// pairs of them (indices into `pts`) cut into the flat faces they cross.
/// Both halves of a face keep its tag.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Knife {
    pub pts: Vec<KnifePoint>,
    #[serde(default)]
    pub segs: Vec<[usize; 2]>,
    /// Delete (Del in Face or Edge mode, Romain 19 nov. 23 h 22): faces
    /// taken out (the part left open), edges taken out (their two faces made
    /// one). A step of its own, with no points.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_faces: Vec<FaceRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_edges: Vec<EdgeRef>,
}

/// A point of the Knife: on an edge, at a fraction of its curve counted
/// from the end nearer `at` (where it was put).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KnifePoint {
    pub edge: EdgeRef,
    pub f: f64,
    pub at: DVec3,
}

impl Knife {
    pub fn label(&self) -> &'static str {
        if !self.remove_faces.is_empty() {
            "Delete faces"
        } else if !self.remove_edges.is_empty() {
            "Delete edges"
        } else if self.segs.is_empty() {
            "Points"
        } else {
            "Knife"
        }
    }
    /// A Delete step.
    pub fn is_delete(&self) -> bool {
        !self.remove_faces.is_empty() || !self.remove_edges.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tweak {
    pub comp: Comp,
    pub delta: DVec3,
    /// More components moved with it (a multiple selection, 19 nov.).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub more: Vec<Comp>,
    /// Scaled along X, Y, Z round their vertices' centre (the R gizmo in
    /// a component mode), before the move.
    #[serde(default = "one3", skip_serializing_if = "is_one3")]
    pub scale: DVec3,
    /// Duplicate faces (Ctrl + D in Face mode, 20 nov.): the picked faces
    /// copied as a piece (closed by flat faces where they are open), moved
    /// and scaled as a move would, then fused with the part; the part
    /// itself does not change.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub copy: bool,
    /// The point it is scaled about (the scale cage, R, 20 nov.: the side
    /// facing the handle dragged stays): else its vertices' centre.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<DVec3>,
}

fn one3() -> DVec3 {
    DVec3::ONE
}
fn is_one3(v: &DVec3) -> bool {
    *v == DVec3::ONE
}

impl Tweak {
    pub fn label(&self) -> &'static str {
        if self.copy {
            return "Duplicate faces";
        }
        if self.scale != DVec3::ONE {
            return match self.comp {
                Comp::Face(_) => "Scale faces",
                Comp::Edge(_) => "Scale edges",
                Comp::Vertex(_) => "Scale vertices",
            };
        }
        match self.comp {
            Comp::Face(_) => "Move face",
            Comp::Edge(_) => "Move edge",
            Comp::Vertex(_) => "Move vertex",
        }
    }
    /// A move of these components (the first one, then the others).
    pub fn new(comps: Vec<Comp>) -> Option<Tweak> {
        let mut it = comps.into_iter();
        let comp = it.next()?;
        Some(Tweak { comp, delta: DVec3::ZERO, more: it.collect(), scale: DVec3::ONE, copy: false, about: None })
    }
    /// All its components.
    pub fn comps(&self) -> impl Iterator<Item = &Comp> {
        std::iter::once(&self.comp).chain(self.more.iter())
    }
    /// Where the components were before the move (their points' mean).
    pub fn at(&self) -> DVec3 {
        let pt = |c: &Comp| match c {
            Comp::Face(f) => f.at,
            Comp::Edge(e) => e.at,
            Comp::Vertex(p) => *p,
        };
        let n = 1 + self.more.len();
        self.comps().map(pt).sum::<DVec3>() / n as f64
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: FeatureId,
    pub name: String,
    pub op: Op,
    pub prim: Prim,
    /// Centre of the base (world).
    pub origin: DVec3,
    /// Local to world rotation (the primitive rises along local +Y).
    pub rot: DQuat,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub material: u32,
    /// Past its element's history marker (the history strip went back
    /// before it): left out of the build, kept in the document, back when
    /// the marker moves past it again. (An operation turned off by hand is
    /// `visible: false`.)
    #[serde(default, skip_serializing_if = "is_false")]
    pub suspended: bool,
    /// A fillet or chamfer step: `op`, `prim` and `rot` are not used (a box
    /// of size 0), `origin` is the middle of its edges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<Round>,
    /// A sketched feature's outline (`Prim::Profile`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
    /// A face pushed or pulled: like `round`, a step on what is built
    /// before it (`origin` is the face point).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<Push>,
    /// A face, edge or vertex moved: like `round`, a step on what is built
    /// before it (`origin` is where the component was).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tweak: Option<Tweak>,
    /// The Knife or Point on edge (from Forme Rectify): like `round`, a
    /// step on what is built before it (`origin` is its first point).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut: Option<Knife>,
    /// A surface with no thickness (the Surface tool, Romain 19 nov.,
    /// 21 h 29: "tracer tout un contour comme avec le Sketch et ensuite ça
    /// remplit une surface"): `prim` is a `Path` of its box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<Sheet>,
    /// A curve made from settings (`Prim::Path`, the Curves set): its
    /// outline follows them (`Doc::regen_curves`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<Curve>,
    /// A 2D boolean of closed curves (`Prim::Path` of its first operand's
    /// box, at its place): its outline is the kernel's (`Model.curves`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape_bool: Option<ShapeBool>,
    /// A closed curve filled with a flat face (Fill; Romain 20 nov.,
    /// 13 h 05: "une courbe… fermée, et que ça puisse faire une surface"):
    /// still a curve to edit, its face built beside the part.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fill: bool,
    /// The curve turned round an axis in its plane (Revolve, 3ds Max's
    /// Lathe): closed, or open with both ends on the axis, a solid (Add /
    /// Carve); else a sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revolve: Option<Revolve>,
    /// A loft through curves (`Prim::Path` of their box, in the first
    /// one's frame, as a 2D boolean): all closed, a solid; else a sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loft: Option<Loft>,
    /// A form sculpted with brushes (Forms; Romain 20 nov., 13 h 05: "le
    /// feeling… d'un Nomad ou un ZBrush… avec la propriété nurbs"): one
    /// smooth BSpline surface, its poles in the unit box of `prim`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<Form>,
}

/// Revolve's settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Revolve {
    /// Degrees (360: a full turn).
    pub angle: f64,
    /// The axis along the curve's local X (else its local Z).
    #[serde(default, skip_serializing_if = "is_false")]
    pub along_x: bool,
    /// Where the axis lies across the curve's box: −1 its min side, 0 its
    /// middle, 1 its max side.
    #[serde(default)]
    pub align: i8,
}

impl Default for Revolve {
    fn default() -> Self {
        Revolve { angle: 360.0, along_x: false, align: -1 }
    }
}

/// Loft's settings: the curves in order (each a Path of the document).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Loft {
    pub sections: Vec<FeatureId>,
    /// Straight between the curves (else smooth through them).
    #[serde(default, skip_serializing_if = "is_false")]
    pub ruled: bool,
}

/// What a form started as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormKind {
    Sphere,
    Cylinder,
}

/// A sculpted form: a bicubic BSpline surface, periodic round (u, `nu`
/// columns) and clamped from bottom to top (v, `nv` rows; its first and
/// last rows each one point: the poles of a sphere, the middles of a
/// cylinder's ends). `poles[j * nu + i]` in the unit box of its
/// `Prim::Box` (x and z from −0.5 to 0.5, y from 0 to 1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Form {
    pub kind: FormKind,
    pub nu: usize,
    pub nv: usize,
    pub poles: Vec<[f64; 3]>,
}

/// What the kernel makes of a feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Makes {
    /// A path: nothing.
    Nothing,
    /// A solid (or a step) in the chain of booleans.
    Solid,
    /// A surface with no thickness, beside the part.
    Sheet,
}

/// A surface's closed outline (straight sides) and the scan points it
/// passes near inside (none: on its plane), both local to the feature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    pub pts: Vec<DVec3>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inner: Vec<DVec3>,
    /// On the scan: `inner` is a grid of nx × nz points (x major) over the
    /// outline's box, the surface follows its heights.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<[usize; 2]>,
}

impl Feature {
    /// A primitive (Add / Carve), not a step on the part (fillet,
    /// chamfer, inset, extrude).
    pub fn is_solid(&self) -> bool {
        self.round.is_none() && self.push.is_none() && self.tweak.is_none() && self.cut.is_none()
    }
    /// A path: a curve with no thickness, nothing for the kernel.
    pub fn is_path(&self) -> bool {
        matches!(self.prim, Prim::Path { .. }) && self.sheet.is_none() && self.loft.is_none()
    }
    /// The axis a Revolve turns round, in the curve's frame (a point on it
    /// and its direction, in its plane).
    pub fn revolve_axis(&self) -> Option<(DVec3, DVec3)> {
        let r = self.revolve?;
        let Prim::Path { x, z } = self.prim else { return None };
        let a = r.align.clamp(-1, 1) as f64 * 0.5;
        Some(if r.along_x { (DVec3::new(0.0, 0.0, a * z), DVec3::X) } else { (DVec3::new(a * x, 0.0, 0.0), DVec3::Z) })
    }
    /// An open curve turned whose two ends lie on the axis (2 % of its
    /// box across it, at least 0.05 mm): it closes into a solid.
    pub fn revolve_closes(&self) -> bool {
        let (Some((o, d)), Some(pr), Prim::Path { x, z }) = (self.revolve_axis(), &self.profile, self.prim) else { return false };
        if !pr.open || pr.points.len() < 2 {
            return false;
        }
        let p = pr.scaled(x, z);
        let across = if d.x.abs() > 0.5 { z } else { x };
        let tol = (across * 0.02).max(0.05);
        let off = |q: [f64; 2]| if d.x.abs() > 0.5 { (q[1] - o.z).abs() } else { (q[0] - o.x).abs() };
        off(p[0]) < tol && off(p[p.len() - 1]) < tol
    }
    /// A curve of the Curves set (or a 2D boolean) pulled into a solid
    /// (step 3, 3ds Max's Extrude): a Profile that keeps its curve.
    pub fn is_extruded_curve(&self) -> bool {
        matches!(self.prim, Prim::Profile { .. }) && (self.curve.is_some() || self.shape_bool.is_some())
    }
    /// A surface with no thickness (built apart, next to the part).
    pub fn is_sheet(&self) -> bool {
        self.sheet.is_some()
    }
    /// What it is: "Cube", "Cylinder", "Fillet", "Chamfer", "Inset", "Extrude".
    pub fn kind_label(&self) -> &'static str {
        if let Some(k) = &self.cut {
            return k.label();
        }
        if self.sheet.is_some() {
            return "Surface";
        }
        if self.form.is_some() {
            return "Form";
        }
        if self.loft.is_some() {
            return "Loft";
        }
        if self.revolve.is_some() {
            return "Revolve";
        }
        if let Some(c) = &self.curve {
            return c.kind.label();
        }
        if let Some(b) = &self.shape_bool {
            return b.op.label();
        }
        match (&self.round, &self.push, &self.tweak) {
            (Some(r), _, _) => r.label(),
            (_, Some(p), _) => p.label(),
            (_, _, Some(t)) => t.label(),
            _ => self.prim.kind().label(),
        }
    }
    /// Local (base centre at 0, height along +Y) to world.
    pub fn matrix(&self) -> DMat4 {
        DMat4::from_rotation_translation(self.rot, self.origin)
    }
    pub fn axis(&self) -> DVec3 {
        self.rot * DVec3::Y
    }
    /// Centre of the volume (world).
    pub fn centre(&self) -> DVec3 {
        self.origin + self.axis() * self.prim.height() * 0.5
    }
    /// The 8 corners of its box (world).
    pub fn corners(&self) -> [DVec3; 8] {
        let (hx, hz) = self.prim.half_xz();
        let h = self.prim.height();
        let m = self.matrix();
        let mut out = [DVec3::ZERO; 8];
        for (i, c) in out.iter_mut().enumerate() {
            let l = DVec3::new(if i & 1 == 0 { -hx } else { hx }, if i & 2 == 0 { 0.0 } else { h }, if i & 4 == 0 { -hz } else { hz });
            *c = m.transform_point3(l);
        }
        out
    }
    /// World bounds.
    pub fn bounds(&self) -> (DVec3, DVec3) {
        let c = self.corners();
        let lo = c.iter().fold(DVec3::splat(f64::MAX), |a, b| a.min(*b));
        let hi = c.iter().fold(DVec3::splat(f64::MIN), |a, b| a.max(*b));
        (lo, hi)
    }
    /// Its outline as polylines (world): a box's 12 edges, a cylinder's two
    /// rims and four lines between them. Drawn over the part for the
    /// selected cutter and while it is being created.
    pub fn outline(&self) -> Vec<Vec<DVec3>> {
        let m = self.matrix();
        let h = self.prim.height();
        match self.prim {
            Prim::Path { .. } if self.sheet.is_some() => {
                let sh = self.sheet.as_ref().unwrap();
                let mut l: Vec<DVec3> = sh.pts.iter().map(|q| m.transform_point3(*q)).collect();
                if let Some(f) = l.first().copied() {
                    l.push(f);
                }
                vec![l]
            }
            Prim::Path { x, z } => {
                let Some(pr) = &self.profile else { return vec![] };
                vec![pr.polyline(x, z).iter().map(|q| m.transform_point3(DVec3::new(q[0], 0.0, q[1]))).collect()]
            }
            Prim::Profile { x, z, .. } => {
                let Some(pr) = &self.profile else { return vec![] };
                let line = pr.polyline(x, z);
                let ring = |y: f64| -> Vec<DVec3> { line.iter().map(|q| m.transform_point3(DVec3::new(q[0], y, q[1]))).collect() };
                let mut out = vec![ring(0.0), ring(h)];
                // the corners' uprights
                let cuts = pr.cuts(x, z);
                for (i, q) in pr.scaled(x, z).iter().enumerate() {
                    if !pr.smooth.get(i).copied().unwrap_or(false) && cuts[i].is_none() {
                        let b = m.transform_point3(DVec3::new(q[0], 0.0, q[1]));
                        out.push(vec![b, b + self.axis() * h]);
                    }
                }
                out
            }
            Prim::Box { .. } => {
                let c = self.corners();
                let pairs = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
                pairs.iter().map(|(a, b)| vec![c[*a], c[*b]]).collect()
            }
            Prim::Cylinder { radius, .. } => {
                let n = 64;
                let ring = |y: f64| -> Vec<DVec3> {
                    (0..=n)
                        .map(|i| {
                            let a = std::f64::consts::TAU * i as f64 / n as f64;
                            m.transform_point3(DVec3::new(radius * a.cos(), y, radius * a.sin()))
                        })
                        .collect()
                };
                let mut out = vec![ring(0.0), ring(h)];
                for k in 0..4 {
                    let a = std::f64::consts::FRAC_PI_2 * k as f64;
                    let p = DVec3::new(radius * a.cos(), 0.0, radius * a.sin());
                    out.push(vec![m.transform_point3(p), m.transform_point3(p + DVec3::Y * h)]);
                }
                out
            }
        }
    }
    /// A curve given new settings (sides, points) and radius, its centre
    /// kept where it was (a pentagon's box is not round its centre).
    pub fn reshape_curve(&mut self, c: Curve, r: f64) {
        let (Prim::Path { x, z } | Prim::Profile { x, z, .. }, Some(old)) = (self.prim, self.curve) else { return };
        let r0 = old.radius(x, z);
        let (_, off0) = old.unit_box();
        let (b, off) = c.unit_box();
        let centre = self.origin + self.rot * DVec3::new(off0[0] * r0, 0.0, off0[1] * r0);
        self.origin = centre - self.rot * DVec3::new(off[0] * r, 0.0, off[1] * r);
        let (nx, nz) = ((r * b[0]).max(0.01), (r * b[1]).max(0.01));
        self.prim = match self.prim {
            Prim::Profile { y, .. } => Prim::Profile { x: nx, y, z: nz },
            _ => Prim::Path { x: nx, z: nz },
        };
        self.curve = Some(c);
    }
    /// Content hash (the kernel's caches, and the name of a step to skip).
    pub fn hash(&self) -> u64 {
        let json = serde_json::to_vec(self).unwrap_or_default();
        fnv(&json)
    }
}

/// FNV-1a over bytes.
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Doc {
    #[serde(default)]
    pub version: u32,
    pub features: Vec<Feature>,
    pub next_id: FeatureId,
    /// Faces given their own material (Face mode, or a ball dropped on a
    /// face), by their packed tag. The kernel does not use it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paint: Vec<Paint>,
    /// A mesh shown in the view as a reference (a scan, an AI-made mesh) to
    /// model on, in its own coordinates (millimetres, Y up). The kernel does
    /// not use it; the app loads it and its tools read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<RefMesh>,
}

/// The reference mesh: its file (.obj).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RefMesh {
    pub path: String,
}

/// A face's own material.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Paint {
    pub tag: i64,
    pub material: u32,
}

impl Default for Doc {
    fn default() -> Self {
        Doc { version: VERSION, features: vec![], next_id: 1, paint: vec![], reference: None }
    }
}

impl Doc {
    pub fn feature(&self, id: FeatureId) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id)
    }
    pub fn feature_mut(&mut self, id: FeatureId) -> Option<&mut Feature> {
        self.features.iter_mut().find(|f| f.id == id)
    }
    pub fn index_of(&self, id: FeatureId) -> Option<usize> {
        self.features.iter().position(|f| f.id == id)
    }
    /// Adds a feature at the end, with a fresh id and a name like "Cube 2".
    pub fn add(&mut self, op: Op, prim: Prim, origin: DVec3, rot: DQuat) -> FeatureId {
        let id = self.next_id;
        self.next_id += 1;
        let name = self.fresh_name(prim.kind().label());
        self.features.push(Feature { id, name, op, prim, origin, rot, visible: true, material: 0, round: None, profile: None, push: None, tweak: None, cut: None, sheet: None, curve: None, shape_bool: None, suspended: false, fill: false, revolve: None, loft: None, form: None });
        id
    }
    /// Adds a fillet (or chamfer) of these edges at the end.
    pub fn add_round(&mut self, size: f64, chamfer: bool, edges: Vec<EdgeRef>) -> FeatureId {
        let id = self.next_id;
        self.next_id += 1;
        let round = Round { size, chamfer, edges, blend: !chamfer, tension: 0.0, only: false };
        let name = self.fresh_name(round.label());
        let origin = round.edges.iter().map(|e| e.at).sum::<DVec3>() / round.edges.len().max(1) as f64;
        self.features.push(Feature { id, name, op: Op::Add, prim: Prim::Box { x: 0.0, y: 0.0, z: 0.0 }, origin, rot: DQuat::IDENTITY, visible: true, material: 0, round: Some(round), profile: None, push: None, tweak: None, cut: None, sheet: None, curve: None, shape_bool: None, suspended: false, fill: false, revolve: None, loft: None, form: None });
        id
    }
    /// Adds an inset or extrusion of a face at the end.
    pub fn add_push(&mut self, push: Push) -> FeatureId {
        let id = self.next_id;
        self.next_id += 1;
        let name = self.fresh_name(push.label());
        let origin = push.face.at;
        self.features.push(Feature { id, name, op: Op::Add, prim: Prim::Box { x: 0.0, y: 0.0, z: 0.0 }, origin, rot: DQuat::IDENTITY, visible: true, material: 0, round: None, profile: None, push: Some(push), tweak: None, cut: None, sheet: None, curve: None, shape_bool: None, suspended: false, fill: false, revolve: None, loft: None, form: None });
        id
    }
    /// Adds a move of a face, edge or vertex at the end.
    pub fn add_tweak(&mut self, tweak: Tweak) -> FeatureId {
        let id = self.next_id;
        self.next_id += 1;
        let name = self.fresh_name(tweak.label());
        let origin = tweak.at();
        self.features.push(Feature { id, name, op: Op::Add, prim: Prim::Box { x: 0.0, y: 0.0, z: 0.0 }, origin, rot: DQuat::IDENTITY, visible: true, material: 0, round: None, profile: None, push: None, tweak: Some(tweak), cut: None, sheet: None, curve: None, shape_bool: None, suspended: false, fill: false, revolve: None, loft: None, form: None });
        id
    }
    /// Adds a Knife step (its first point) at the end.
    pub fn add_knife(&mut self, cut: Knife, name: &str) -> FeatureId {
        let id = self.next_id;
        self.next_id += 1;
        let name = self.fresh_name(name);
        let origin = cut.pts.first().map(|p| p.at).or(cut.remove_faces.first().map(|r| r.at)).or(cut.remove_edges.first().map(|e| e.at)).unwrap_or(DVec3::ZERO);
        self.features.push(Feature { id, name, op: Op::Add, prim: Prim::Box { x: 0.0, y: 0.0, z: 0.0 }, origin, rot: DQuat::IDENTITY, visible: true, material: 0, round: None, profile: None, push: None, tweak: None, cut: Some(cut), sheet: None, curve: None, shape_bool: None, suspended: false, fill: false, revolve: None, loft: None, form: None });
        id
    }
    /// The curves' outlines made again from their settings and boxes (a
    /// circle stretched becomes an ellipse). True when one changed.
    pub fn regen_curves(&mut self) -> bool {
        let mut changed = false;
        for f in self.features.iter_mut() {
            // flat, or extruded (step 3: a Profile of its height)
            let (x, z) = match f.prim {
                Prim::Path { x, z } | Prim::Profile { x, z, .. } => (x, z),
                _ => continue,
            };
            let Some(c) = f.curve.as_mut() else { continue };
            if c.kind == CurveKind::Circle && (x - z).abs() > 1e-9 * x.abs().max(z.abs()).max(1.0) {
                c.kind = CurveKind::Ellipse;
                changed = true;
            }
            let pr = c.profile(x, z);
            if f.profile.as_ref() != Some(&pr) {
                f.profile = Some(pr);
                changed = true;
            }
        }
        // a 2D boolean's frame is its first operand's, its box all of its
        // operands' (its operands come before it; twice for one of another);
        // a loft's likewise, its middle half-way up its curves
        for _ in 0..2 {
            for k in 0..self.features.len() {
                let loft = self.features[k].loft.is_some();
                let list = match (&self.features[k].shape_bool, &self.features[k].loft) {
                    (Some(b), _) => b.operands.clone(),
                    (_, Some(l)) => l.sections.clone(),
                    _ => continue,
                };
                let ops: Vec<&Feature> = list.iter().filter_map(|id| self.feature(*id)).collect();
                let Some(first) = ops.first() else { continue };
                // extruded down: turned over round its own Z (its curves
                // stay where they are)
                let flip = self.features[k].axis().dot(first.axis()) < 0.0;
                let rot = if flip { first.rot * DQuat::from_rotation_z(std::f64::consts::PI) } else { first.rot };
                let inv = first.matrix().inverse();
                let (mut lo, mut hi) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
                for g in &ops {
                    for c in g.corners() {
                        let q = inv.transform_point3(c);
                        lo = lo.min(q);
                        hi = hi.max(q);
                    }
                }
                let mid = first.matrix().transform_point3(DVec3::new((lo.x + hi.x) * 0.5, if loft { (lo.y + hi.y) * 0.5 } else { 0.0 }, (lo.z + hi.z) * 0.5));
                let (bx, bz) = ((hi.x - lo.x).max(0.01), (hi.z - lo.z).max(0.01));
                let prim = match self.features[k].prim {
                    Prim::Profile { y, .. } => Prim::Profile { x: bx, y, z: bz },
                    _ => Prim::Path { x: bx, z: bz },
                };
                let f = &mut self.features[k];
                if f.rot != rot || f.origin.distance(mid) > 1e-9 || f.prim != prim {
                    f.rot = rot;
                    f.origin = mid;
                    f.prim = prim;
                    changed = true;
                }
            }
        }
        changed
    }
    /// The 2D boolean that takes this curve in (the first, when several).
    pub fn boolean_of(&self, id: FeatureId) -> Option<FeatureId> {
        self.features.iter().find(|f| f.shape_bool.as_ref().map(|b| b.operands.contains(&id)).unwrap_or(false)).map(|f| f.id)
    }
    /// A closed curve, or a 2D boolean: what a 2D boolean can take in.
    pub fn is_closed_curve(&self, id: FeatureId) -> bool {
        self.feature(id).map(|f| f.is_path() && (f.shape_bool.is_some() || f.profile.as_ref().map(|p| !p.open && p.points.len() >= 3).unwrap_or(false))).unwrap_or(false)
    }
    /// What the kernel makes of it: nothing (a path), a solid in the chain
    /// (primitives, steps, a curve turned closed, a loft of closed curves),
    /// or a sheet beside the part (a surface, a curve filled, a curve
    /// turned open, a loft of open curves).
    pub fn makes(&self, f: &Feature) -> Makes {
        if f.sheet.is_some() {
            return Makes::Sheet;
        }
        if let Some(l) = &f.loft {
            let closed = l.sections.len() >= 2 && l.sections.iter().all(|s| self.is_closed_curve(*s));
            return if closed { Makes::Solid } else { Makes::Sheet };
        }
        if f.is_path() {
            if f.revolve.is_some() {
                return if self.is_closed_curve(f.id) || f.revolve_closes() { Makes::Solid } else { Makes::Sheet };
            }
            return if f.fill { Makes::Sheet } else { Makes::Nothing };
        }
        Makes::Solid
    }
    /// The loft that takes this curve in (the first, when several).
    pub fn loft_of(&self, id: FeatureId) -> Option<FeatureId> {
        self.features.iter().find(|f| f.loft.as_ref().map(|l| l.sections.contains(&id)).unwrap_or(false)).map(|f| f.id)
    }
    /// "Cube", then "Cube 2", "Cube 3"…
    pub fn fresh_name(&self, base: &str) -> String {
        if !self.features.iter().any(|f| f.name == base) {
            return base.to_string();
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| !self.features.iter().any(|f| &f.name == n)).unwrap()
    }
    /// Takes these features out. A 2D boolean takes its operands with it
    /// (those no other boolean left takes in); an operand taken out leaves
    /// its boolean's list, a boolean left with none goes too.
    pub fn remove(&mut self, ids: &[FeatureId]) {
        let mut out: Vec<FeatureId> = ids.to_vec();
        // (a loft keeps its curves: they are the user's)
        let booleans: Vec<FeatureId> = ids.iter().copied().filter(|id| self.feature(*id).map(|f| f.loft.is_none()).unwrap_or(false)).collect();
        for id in self.with_operands(&booleans) {
            let others = self.features.iter().any(|f| !out.contains(&f.id) && f.shape_bool.as_ref().map(|b| b.operands.contains(&id)).unwrap_or(false));
            if !others && !out.contains(&id) {
                out.push(id);
            }
        }
        self.features.retain(|f| !out.contains(&f.id));
        loop {
            for f in self.features.iter_mut() {
                if let Some(b) = f.shape_bool.as_mut() {
                    b.operands.retain(|o| !out.contains(o));
                }
                if let Some(l) = f.loft.as_mut() {
                    l.sections.retain(|o| !out.contains(o));
                }
            }
            let empty: Vec<FeatureId> = self.features.iter().filter(|f| f.shape_bool.as_ref().map(|b| b.operands.is_empty()).unwrap_or(false) || f.loft.as_ref().map(|l| l.sections.len() < 2).unwrap_or(false)).map(|f| f.id).collect();
            if empty.is_empty() {
                break;
            }
            out.extend(empty.iter().copied());
            self.features.retain(|f| !empty.contains(&f.id));
        }
    }
    /// Copies of the shapes (after the last feature), with fresh ids; returns
    /// the new ids. Fillets and chamfers are not copied (their edges belong
    /// to the original).
    pub fn duplicate(&mut self, ids: &[FeatureId], offset: DVec3) -> Vec<FeatureId> {
        // a 2D boolean takes its operands with it
        let all = self.with_operands(ids);
        let src: Vec<Feature> = self.features.iter().filter(|f| all.contains(&f.id) && f.is_solid()).cloned().collect();
        let mut out = vec![];
        let mut map = std::collections::HashMap::new();
        for mut f in src {
            let old = f.id;
            f.id = self.next_id;
            self.next_id += 1;
            map.insert(old, f.id);
            f.name = self.fresh_name(f.kind_label());
            f.origin += offset;
            if ids.contains(&old) {
                out.push(f.id);
            }
            self.features.push(f);
        }
        for f in self.features.iter_mut().filter(|f| map.values().any(|v| *v == f.id)) {
            if let Some(b) = f.shape_bool.as_mut() {
                for o in b.operands.iter_mut() {
                    if let Some(n) = map.get(o) {
                        *o = *n;
                    }
                }
            }
            if let Some(l) = f.loft.as_mut() {
                for o in l.sections.iter_mut() {
                    if let Some(n) = map.get(o) {
                        *o = *n;
                    }
                }
            }
        }
        out
    }
    /// The features with the operands of the 2D booleans among them (and
    /// theirs), each once.
    pub fn with_operands(&self, ids: &[FeatureId]) -> Vec<FeatureId> {
        let mut out: Vec<FeatureId> = ids.to_vec();
        let mut k = 0;
        while k < out.len() {
            if let Some(f) = self.feature(out[k]) {
                let ops = f.shape_bool.iter().flat_map(|b| b.operands.iter()).chain(f.loft.iter().flat_map(|l| l.sections.iter()));
                for o in ops {
                    if !out.contains(o) {
                        out.push(*o);
                    }
                }
            }
            k += 1;
        }
        out
    }
    /// Bounds of the visible features (None when empty).
    pub fn bounds(&self) -> Option<(DVec3, DVec3)> {
        self.features.iter().filter(|f| f.visible).map(|f| f.bounds()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
    pub fn from_json(s: &str) -> Result<Doc, String> {
        let d: Doc = serde_json::from_str(s).map_err(|e| e.to_string())?;
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let mut d = Doc::default();
        let a = d.add(Op::Add, PrimKind::Box.default_prim(), DVec3::ZERO, DQuat::IDENTITY);
        let b = d.add(Op::Carve, Prim::Cylinder { radius: 4.0, height: 30.0 }, DVec3::new(0.0, -5.0, 0.0), DQuat::IDENTITY);
        assert_eq!((a, b), (1, 2));
        assert_eq!(d.features[1].name, "Cylinder");
        assert_eq!(d.fresh_name("Cube"), "Cube 2");
        let r = d.add_round(2.0, false, vec![EdgeRef { faces: [65538, 65536], at: DVec3::new(10.0, 20.0, 0.0) }]);
        assert_eq!(d.feature(r).unwrap().name, "Fillet");
        assert_eq!(d.feature(r).unwrap().round.as_ref().unwrap().edges[0].key(), [65536, 65538]);
        assert!(d.feature(1).unwrap().round.is_none() && !d.to_json().contains("\"round\": null"));
        let back = Doc::from_json(&d.to_json()).unwrap();
        assert_eq!(back, d);
        let (lo, hi) = d.feature(1).unwrap().bounds();
        assert!((lo - DVec3::new(-10.0, 0.0, -10.0)).length() < 1e-9 && (hi - DVec3::new(10.0, 20.0, 10.0)).length() < 1e-9);
    }
}
