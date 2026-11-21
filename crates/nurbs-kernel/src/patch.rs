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
    /// The control points in the mesh's unit box (as its points). One
    /// patch (a surface from a path): its 16, `cv[j * 4 + i]`, i along u, j
    /// along v. Several (a closed object, `quads`): every patch's, those on
    /// a seam shared.
    pub cv: Vec<[f32; 3]>,
    pub state: PatchState,
    /// Each point of the Poly layer on the surface: (u, v, a, b, h), its
    /// place and its offset a Tu + b Tv + h N there (mm).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bind: Vec<[f32; 5]>,
    /// A closed object (a cube's 6 patches, a cylinder's 6): each patch's 16
    /// control points as indices into `cv`, `[j * 4 + i]`; empty: one patch,
    /// `cv` itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quads: Vec<[u32; 16]>,
    /// The patch each point of `bind` lies on (empty: all on the first).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<u16>,
    /// The Sculpt state's details, kept as offsets over the Poly layer made
    /// smooth (so they ride on the contour and on the Poly layer's edits).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sculpt: Option<SculptLayer>,
    /// In the Sculpt state: the Poly layer set aside (the mesh then holds
    /// the clay).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<PolyKeep>,
}

/// The Sculpt state's details (Romain, 20 nov., 23 h: "faire aussi le lien
/// avec le Sculpt mode"): the clay is the Poly layer subdivided `levels`
/// times (Catmull-Clark: the base), each of its points moved by `disp`, in
/// the base's own frame there (along its first edge, across, its normal; mm).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SculptLayer {
    pub levels: u32,
    /// The Poly layer's faces it was made on (`topo_key`): the same, the
    /// offsets go on as they are; else they are carried over by place.
    pub topo: u64,
    pub disp: Vec<[f32; 3]>,
    /// Where each base point sat on the surface (patch, u, v, a, b, h), to
    /// carry the offsets over to a new base after the Poly layer's faces
    /// changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub base_bind: Vec<[f32; 6]>,
    /// The same offsets in the surface's frame at `base_bind` (Tu, Tv, N),
    /// for that carrying over.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disp_s: Vec<[f32; 3]>,
}

/// Each point's nearest among others (a uniform grid of cells; brute force
/// past a few rings): the index into `from` for each of `to`.
pub fn nearest_of(from: &[Vec3], to: &[Vec3]) -> Vec<usize> {
    if from.is_empty() {
        return vec![0; to.len()];
    }
    let lo = from.iter().fold(Vec3::splat(f32::MAX), |a, p| a.min(*p));
    let hi = from.iter().fold(Vec3::splat(f32::MIN), |a, p| a.max(*p));
    let ext = (hi - lo).max(Vec3::splat(1e-3));
    let cell = ((ext.x * ext.y * ext.z) / from.len() as f32).cbrt().max(ext.max_element() / 256.0) * 1.5;
    let key = |p: Vec3| {
        let q = (p - lo) / cell;
        (q.x.floor() as i32, q.y.floor() as i32, q.z.floor() as i32)
    };
    let mut grid: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
    for (i, p) in from.iter().enumerate() {
        grid.entry(key(*p)).or_default().push(i as u32);
    }
    to.iter()
        .map(|p| {
            let c = key(*p);
            let mut best: Option<(f32, usize)> = None;
            for ring in 0..4i32 {
                for dx in -ring..=ring {
                    for dy in -ring..=ring {
                        for dz in -ring..=ring {
                            if dx.abs().max(dy.abs()).max(dz.abs()) != ring {
                                continue;
                            }
                            if let Some(l) = grid.get(&(c.0 + dx, c.1 + dy, c.2 + dz)) {
                                for &j in l {
                                    let d = from[j as usize].distance_squared(*p);
                                    if best.map(|b| d < b.0).unwrap_or(true) {
                                        best = Some((d, j as usize));
                                    }
                                }
                            }
                        }
                    }
                }
                // found within this ring: nothing nearer past the next one
                if let Some((d, _)) = best {
                    if d.sqrt() <= ring as f32 * cell {
                        break;
                    }
                }
            }
            match best {
                Some((_, j)) => j,
                None => from.iter().enumerate().min_by(|a, b| a.1.distance_squared(*p).total_cmp(&b.1.distance_squared(*p))).map(|x| x.0).unwrap_or(0),
            }
        })
        .collect()
}

/// The Poly layer while the mesh holds the clay (Sculpt state): its points
/// (unit box, kept in step by `remap`), faces, binding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolyKeep {
    pub verts: Vec<[f32; 3]>,
    pub faces: Vec<Vec<u32>>,
    pub bind: Vec<[f32; 5]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<u16>,
}

/// A key of a polygon list (the Sculpt layer's base is the same while it
/// holds).
pub fn topo_key(faces: &[Vec<u32>]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    faces.hash(&mut h);
    h.finish()
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
        let mut p = Patch { cv: cv.iter().map(|v| v.to_array()).collect(), state: PatchState::Nurbs, bind: vec![], quads: vec![], on: vec![], sculpt: None, keep: None };
        p.fill_inside();
        p
    }

    /// A closed object of several patches, each given by its 16 control
    /// points (mm or unit, as `cv`): the points on seams (the same place)
    /// shared, so a contour point moved moves every patch that holds it.
    pub fn closed(patches: &[[Vec3; 16]]) -> Patch {
        let mut cv: Vec<Vec3> = vec![];
        let mut quads = vec![];
        let scale = patches.iter().flat_map(|q| q.iter()).fold(0.0f32, |a, v| a.max(v.abs().max_element())).max(1e-6);
        let eps = scale * 1e-5;
        for q in patches {
            let mut idx = [0u32; 16];
            for k in 0..16 {
                let inner = matches!(k, 5 | 6 | 9 | 10);
                let found = if inner { None } else { cv.iter().position(|c| c.distance(q[k]) < eps) };
                idx[k] = match found {
                    Some(i) => i as u32,
                    None => {
                        cv.push(q[k]);
                        (cv.len() - 1) as u32
                    }
                };
            }
            quads.push(idx);
        }
        let mut p = Patch { cv: cv.iter().map(|v| v.to_array()).collect(), state: PatchState::Nurbs, bind: vec![], quads, on: vec![], sculpt: None, keep: None };
        p.fill_inside();
        p
    }

    /// Each patch's 16 control points (indices into `cv`).
    pub fn patches(&self) -> Vec<[usize; 16]> {
        if self.quads.is_empty() {
            vec![std::array::from_fn(|k| k)]
        } else {
            self.quads.iter().map(|q| q.map(|k| k as usize)).collect()
        }
    }

    pub fn count(&self) -> usize {
        self.quads.len().max(1)
    }

    /// The patch a point of the binding lies on.
    pub fn on_of(&self, i: usize) -> usize {
        self.on.get(i).copied().unwrap_or(0) as usize
    }

    /// The contour's control points (the ones edited: every patch's
    /// border, each once), in order round each patch.
    pub fn contour(&self) -> Vec<usize> {
        let mut out = vec![];
        for q in self.patches() {
            for k in CONTOUR {
                if !out.contains(&q[k]) {
                    out.push(q[k]);
                }
            }
        }
        out
    }

    /// A control point at a corner of a patch (drawn as a square; moved, it
    /// takes the tangent points next to it).
    pub fn corner(&self, cv: usize) -> bool {
        self.patches().iter().any(|q| [0, 3, 12, 15].iter().any(|k| q[*k] == cv))
    }

    /// The tangent points a corner carries (every patch that holds it).
    pub fn handles(&self, cv: usize) -> Vec<usize> {
        let mut out = vec![];
        for q in self.patches() {
            for (c, t) in [(0, [1, 4]), (3, [2, 7]), (15, [14, 11]), (12, [13, 8])] {
                if q[c] == cv {
                    for k in t {
                        if !out.contains(&q[k]) {
                            out.push(q[k]);
                        }
                    }
                }
            }
        }
        out
    }

    /// The pairs (corner, tangent point) of every patch's hull (the lines
    /// drawn in Vertex mode), each once.
    pub fn hull(&self) -> Vec<(usize, usize)> {
        let mut out: Vec<(usize, usize)> = vec![];
        for q in self.patches() {
            for (c, t) in [(0, 1), (3, 2), (3, 7), (15, 11), (15, 14), (12, 13), (12, 8), (0, 4)] {
                let e = (q[c], q[t]);
                if !out.contains(&e) {
                    out.push(e);
                }
            }
        }
        out
    }

    /// The 4 inner control points of each patch made from its contour: the
    /// bilinearly blended Coons patch of its four cubic sides, a bicubic
    /// itself.
    pub fn fill_inside(&mut self) {
        for q in self.patches() {
            let p = |i: usize, j: usize| Vec3::from(self.cv[q[j * 4 + i]]);
            let mut inner = vec![];
            for j in 1..3 {
                for i in 1..3 {
                    let (s, t) = (i as f32 / 3.0, j as f32 / 3.0);
                    let v = p(i, 0) * (1.0 - t) + p(i, 3) * t + p(0, j) * (1.0 - s) + p(3, j) * s
                        - (p(0, 0) * (1.0 - s) * (1.0 - t) + p(3, 0) * s * (1.0 - t) + p(0, 3) * (1.0 - s) * t + p(3, 3) * s * t);
                    inner.push((q[j * 4 + i], v));
                }
            }
            for (k, v) in inner {
                self.cv[k] = v.to_array();
            }
        }
    }

    /// The first patch's control points in mm (the feature's frame), for
    /// these sizes.
    pub fn cvs(&self, size: Vec3) -> [Vec3; 16] {
        let q = self.patches()[0];
        std::array::from_fn(|k| Vec3::from(self.cv[q[k]]) * size)
    }

    /// Every patch's control points in mm, for these sizes.
    pub fn surfaces(&self, size: Vec3) -> Vec<[Vec3; 16]> {
        self.patches().iter().map(|q| std::array::from_fn(|k| Vec3::from(self.cv[q[k]]) * size)).collect()
    }

    /// The unit box fitted again (`MeshData::renormalized`): the control
    /// points (and the Poly layer set aside in the Sculpt state) stay where
    /// they are.
    pub fn remap(&mut self, old: Vec3, base: Vec3, new: Vec3) {
        for c in self.cv.iter_mut() {
            *c = ((Vec3::from(*c) * old - base) / new).to_array();
        }
        if let Some(k) = self.keep.as_mut() {
            for c in k.verts.iter_mut() {
                *c = ((Vec3::from(*c) * old - base) / new).to_array();
            }
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

/// The point and its frame at (u, v); where the patch folds flat (a disc's
/// four corners, where two arcs meet tangent) the frame is read a hair
/// inside, towards the middle.
pub fn eval_frame(cv: &[Vec3; 16], u: f32, v: f32) -> (Vec3, (Vec3, Vec3, Vec3)) {
    let (s, su, sv) = eval(cv, u, v);
    let c = su.cross(sv);
    if c.length() > 1e-6 * (su.length() * sv.length() + 1e-12) {
        return (s, frame(su, sv));
    }
    let (u2, v2) = (u + (0.5 - u).signum() * 2e-3, v + (0.5 - v).signum() * 2e-3);
    let (_, su2, sv2) = eval(cv, u2, v2);
    (s, frame(su2, sv2))
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
    uv.iter().map(|t| eval_frame(cv, t[0], t[1]).1 .2).collect()
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
    let (s, (tu, tv, n)) = eval_frame(cv, u, v);
    let r = p - s;
    [u, v, r.dot(tu), r.dot(tv), r.dot(n)]
}

/// A bound point put back on the surface as it is now (mm).
pub fn place(cv: &[Vec3; 16], b: [f32; 5]) -> Vec3 {
    let (s, (tu, tv, n)) = eval_frame(cv, b[0], b[1]);
    s + tu * b[2] + tv * b[3] + n * b[4]
}

// ------------------------------------------------------- closed objects

/// Seeds on every patch: (point, patch, (u, v)).
pub fn seeds_multi(surfs: &[[Vec3; 16]], n: usize) -> Vec<(Vec3, usize, [f32; 2])> {
    let mut out = vec![];
    for (k, cv) in surfs.iter().enumerate() {
        for (p, uv) in seeds(cv, n) {
            out.push((p, k, uv));
        }
    }
    out
}

/// Where a point sits on a closed object: the patch whose nearest point is
/// nearest (tried on the patches of its nearest seeds), (u, v) and offset
/// there.
pub fn bind_multi(surfs: &[[Vec3; 16]], p: Vec3, seeds: &[(Vec3, usize, [f32; 2])]) -> (usize, [f32; 5]) {
    if surfs.len() == 1 {
        let s1: Vec<(Vec3, [f32; 2])> = seeds.iter().map(|s| (s.0, s.2)).collect();
        return (0, bind(&surfs[0], p, &s1));
    }
    // the nearest seeds, a few patches
    let mut near: Vec<(f32, usize, [f32; 2])> = seeds.iter().map(|s| (s.0.distance_squared(p), s.1, s.2)).collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut tried: Vec<usize> = vec![];
    let mut best: Option<(f32, usize, [f32; 5])> = None;
    for (_, k, uv) in near.iter().take(24) {
        if tried.contains(k) {
            continue;
        }
        tried.push(*k);
        let b = bind(&surfs[*k], p, &[(eval(&surfs[*k], uv[0], uv[1]).0, *uv)]);
        // the tangent offsets count most: a point off the patch's border
        // belongs to the patch past it
        let d = b[2] * b[2] + b[3] * b[3] + 1e-3 * b[4] * b[4];
        if best.map(|x| d < x.0).unwrap_or(true) {
            best = Some((d, *k, b));
        }
        if tried.len() >= 3 {
            break;
        }
    }
    let (_, k, b) = best.unwrap_or((0.0, 0, [0.5, 0.5, 0.0, 0.0, 0.0]));
    (k, b)
}

/// `settle_on` on a closed object: the cell from the layer's edges lying on
/// one patch.
pub fn settle_on_multi(surfs: &[[Vec3; 16]], bind: &mut [[f32; 5]], on: &[u16], faces: &[Vec<u32>]) -> usize {
    let at = |i: usize| on.get(i).copied().unwrap_or(0) as usize;
    let flat = |b: &[f32; 5]| b[2].abs() + b[3].abs() + b[4].abs() <= OFF;
    let (mut du, mut dv) = (0.0f32, 0.0f32);
    for f in faces {
        for i in 0..f.len() {
            let (a, b) = (f[i] as usize, f[(i + 1) % f.len()] as usize);
            if a < bind.len() && b < bind.len() && at(a) == at(b) && flat(&bind[a]) && flat(&bind[b]) {
                du = du.max((bind[a][0] - bind[b][0]).abs());
                dv = dv.max((bind[a][1] - bind[b][1]).abs());
            }
        }
    }
    if du <= 0.0 || dv <= 0.0 {
        return 0;
    }
    let mut n = 0;
    for (i, b) in bind.iter_mut().enumerate() {
        if flat(b) || b[2].abs() + b[3].abs() > OFF {
            continue;
        }
        let cv = &surfs[at(i).min(surfs.len() - 1)];
        if b[4].abs() <= OFF + 1.25 * sag(cv, b[0], b[1], du, dv) {
            b[4] = 0.0;
            n += 1;
        }
    }
    n
}

/// A cubic Bézier quarter (or any) arc of a circle in the XZ plane at
/// height y, from angle a0 to a1 (radians; x = r cos, z = -r sin, so the
/// angle grows counter-clockwise seen from +Y... towards -Z).
fn arc(r: f32, y: f32, a0: f32, a1: f32) -> [Vec3; 4] {
    let at = |a: f32| Vec3::new(r * a.cos(), y, -r * a.sin());
    let tan = |a: f32| Vec3::new(-a.sin(), 0.0, -a.cos());
    let k = 4.0 / 3.0 * ((a1 - a0) / 4.0).tan() * r;
    [at(a0), at(a0) + tan(a0) * k, at(a1) - tan(a1) * k, at(a1)]
}

/// A patch whose (u, v) are swapped (its normal turned over).
fn transposed(q: [Vec3; 16]) -> [Vec3; 16] {
    std::array::from_fn(|k| q[(k % 4) * 4 + k / 4])
}

/// The patch facing `out` (its middle's normal), else turned over.
fn facing(q: [Vec3; 16], out: Vec3) -> [Vec3; 16] {
    let (_, (_, _, n)) = eval_frame(&q, 0.5, 0.5);
    if n.dot(out) < 0.0 { transposed(q) } else { q }
}

/// A box's six patches (mm, its base's centre at the origin, y up), flat,
/// their normals out.
pub fn box_patches(size: Vec3) -> Vec<[Vec3; 16]> {
    let h = size * 0.5;
    let c = |x: f32, y: f32, z: f32| Vec3::new(x * h.x, (y + 1.0) * h.y, z * h.z);
    // each face: its corners round it, and its outward normal
    let faces: [([Vec3; 4], Vec3); 6] = [
        ([c(1., -1., 1.), c(1., -1., -1.), c(1., 1., -1.), c(1., 1., 1.)], Vec3::X),
        ([c(-1., -1., -1.), c(-1., -1., 1.), c(-1., 1., 1.), c(-1., 1., -1.)], -Vec3::X),
        ([c(-1., 1., 1.), c(1., 1., 1.), c(1., 1., -1.), c(-1., 1., -1.)], Vec3::Y),
        ([c(-1., -1., -1.), c(1., -1., -1.), c(1., -1., 1.), c(-1., -1., 1.)], -Vec3::Y),
        ([c(-1., -1., 1.), c(1., -1., 1.), c(1., 1., 1.), c(-1., 1., 1.)], Vec3::Z),
        ([c(1., -1., -1.), c(-1., -1., -1.), c(-1., 1., -1.), c(1., 1., -1.)], -Vec3::Z),
    ];
    faces.iter().map(|(k, n)| facing(Patch::new(*k).cvs(Vec3::ONE), *n)).collect()
}

/// A cylinder's six patches (mm, its base's centre at the origin, y up):
/// its side in four quarters (each a quarter arc swept straight up), its
/// two caps each a disc of the four quarter arcs; seams shared.
pub fn cylinder_patches(r: f32, h: f32) -> Vec<[Vec3; 16]> {
    use std::f32::consts::FRAC_PI_2;
    let mut out = vec![];
    for q in 0..4 {
        let (a0, a1) = (q as f32 * FRAC_PI_2, (q + 1) as f32 * FRAC_PI_2);
        let mut p = [Vec3::ZERO; 16];
        for j in 0..4 {
            let row = arc(r, h * j as f32 / 3.0, a0, a1);
            for i in 0..4 {
                p[j * 4 + i] = row[i];
            }
        }
        out.push(facing(p, Vec3::new(((a0 + a1) * 0.5).cos(), 0.0, -((a0 + a1) * 0.5).sin())));
    }
    for (y, n) in [(h, Vec3::Y), (0.0, -Vec3::Y)] {
        // corners at 0°, 90°, 180°, 270°: v = 0 runs 0° → 90°, u = 1 runs
        // 90° → 180°, v = 1 runs 270° → 180°, u = 0 runs 0° → 270°
        let s0 = arc(r, y, 0.0, FRAC_PI_2);
        let s1 = arc(r, y, FRAC_PI_2, 2.0 * FRAC_PI_2);
        let s2 = arc(r, y, 3.0 * FRAC_PI_2, 2.0 * FRAC_PI_2);
        let s3 = arc(r, y, 4.0 * FRAC_PI_2, 3.0 * FRAC_PI_2);
        let mut p = [Vec3::ZERO; 16];
        for i in 0..4 {
            p[i] = s0[i];
            p[12 + i] = s2[i];
            p[i * 4 + 3] = s1[i];
            p[i * 4] = s3[i];
        }
        // the 4 corners exactly the arcs' ends (360° is 0°)
        p[0] = s0[0];
        let mut pp = Patch { cv: p.iter().map(|v| v.to_array()).collect(), state: PatchState::Nurbs, bind: vec![], quads: vec![], on: vec![], sculpt: None, keep: None };
        pp.fill_inside();
        out.push(facing(pp.cvs(Vec3::ONE), n));
    }
    out
}

/// The Poly layer of a closed object: an iso grid on each patch (`counts`
/// its quads along u and v), the points on seams welded; each point's patch
/// and (u, v). Patches sharing a side must have the same count along it.
pub fn grid_closed(surfs: &[[Vec3; 16]], counts: &[(usize, usize)]) -> (Vec<Vec3>, Vec<Vec<u32>>, Vec<u16>, Vec<[f32; 2]>) {
    let scale = surfs.iter().flat_map(|q| q.iter()).fold(0.0f32, |a, v| a.max(v.abs().max_element())).max(1e-6);
    let eps = scale * 1e-4;
    let (mut pts, mut on, mut uvs): (Vec<Vec3>, Vec<u16>, Vec<[f32; 2]>) = (vec![], vec![], vec![]);
    let mut faces = vec![];
    // a coarse hash of the points so far, for the welding
    let cell = eps * 8.0;
    let key = |p: Vec3| ((p.x / cell).round() as i64, (p.y / cell).round() as i64, (p.z / cell).round() as i64);
    let mut hash: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    for (k, cv) in surfs.iter().enumerate() {
        let (nu, nv) = counts[k];
        let (gp, gf, guv) = grid(cv, nu, nv);
        let mut map = vec![0u32; gp.len()];
        for (i, p) in gp.iter().enumerate() {
            let c = key(*p);
            let mut found = None;
            'look: for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(l) = hash.get(&(c.0 + dx, c.1 + dy, c.2 + dz)) {
                            for &j in l {
                                if pts[j as usize].distance(*p) < eps {
                                    found = Some(j);
                                    break 'look;
                                }
                            }
                        }
                    }
                }
            }
            map[i] = match found {
                Some(j) => j,
                None => {
                    pts.push(*p);
                    on.push(k as u16);
                    uvs.push(guv[i]);
                    let j = (pts.len() - 1) as u32;
                    hash.entry(c).or_default().push(j);
                    j
                }
            };
        }
        for f in gf {
            let q: Vec<u32> = f.iter().map(|v| map[*v as usize]).collect();
            // a cap's corner cell folds to three points: kept as a triangle
            let mut d = q.clone();
            d.dedup();
            if d.len() > 1 && d[0] == d[d.len() - 1] {
                d.pop();
            }
            if d.len() >= 3 {
                faces.push(d);
            }
        }
    }
    (pts, faces, on, uvs)
}

/// How far the surface leaves the chords of a cell `du` × `dv` (in
/// parameter) round (u, v), in mm: its curvature along the normal there
/// (finite differences of the tangents) over the cell, an eighth of it.
pub fn sag(cv: &[Vec3; 16], u: f32, v: f32, du: f32, dv: f32) -> f32 {
    let h = 1e-3;
    let (u0, u1) = ((u - h).max(0.0), (u + h).min(1.0));
    let (v0, v1) = ((v - h).max(0.0), (v + h).min(1.0));
    let (_, su, sv) = eval(cv, u, v);
    let n = frame(su, sv).2;
    let (_, sua, _) = eval(cv, u0, v);
    let (_, sub, _) = eval(cv, u1, v);
    let (_, suc, svc) = eval(cv, u, v0);
    let (_, sud, svd) = eval(cv, u, v1);
    let suu = n.dot(sub - sua) / (u1 - u0);
    let svv = n.dot(svd - svc) / (v1 - v0);
    let suv = n.dot(sud - suc) / (v1 - v0);
    (suu.abs() * du * du + 2.0 * suv.abs() * du * dv + svv.abs() * dv * dv) / 8.0
}

/// Back from Poly: the points that sit off the surface by no more than its
/// facets do (an inset's ring, a loop or a cut across a face: they lie on
/// the layer's flat faces, which cut the curve's chords) are put on it, so
/// they ride on it as the grid does; what stands off further (an
/// extrusion, a point pulled by hand) keeps its height. The cell is the
/// largest step in u and in v along the layer's edges lying on the surface.
/// Returns how many were put on it.
pub fn settle_on(cv: &[Vec3; 16], bind: &mut [[f32; 5]], faces: &[Vec<u32>]) -> usize {
    let on = |b: &[f32; 5]| b[2].abs() + b[3].abs() + b[4].abs() <= OFF;
    let (mut du, mut dv) = (0.0f32, 0.0f32);
    for f in faces {
        for i in 0..f.len() {
            let (a, b) = (f[i] as usize, f[(i + 1) % f.len()] as usize);
            if a < bind.len() && b < bind.len() && on(&bind[a]) && on(&bind[b]) {
                du = du.max((bind[a][0] - bind[b][0]).abs());
                dv = dv.max((bind[a][1] - bind[b][1]).abs());
            }
        }
    }
    if du <= 0.0 || dv <= 0.0 {
        return 0;
    }
    let mut n = 0;
    for b in bind.iter_mut() {
        if on(b) || b[2].abs() + b[3].abs() > OFF {
            continue;
        }
        if b[4].abs() <= OFF + 1.25 * sag(cv, b[0], b[1], du, dv) {
            b[4] = 0.0;
            n += 1;
        }
    }
    n
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

    /// Every edge of a polygon list held by exactly two polygons, each way
    /// once (closed and oriented).
    fn closed_and_oriented(faces: &[Vec<u32>]) -> bool {
        let mut half: HashMap<(u32, u32), u32> = HashMap::new();
        for f in faces {
            for i in 0..f.len() {
                *half.entry((f[i], f[(i + 1) % f.len()])).or_default() += 1;
            }
        }
        half.iter().all(|((a, b), n)| *n == 1 && half.get(&(*b, *a)) == Some(&1))
    }

    #[test]
    fn closed_objects_share_their_seams() {
        // a cube: 8 corners and 2 tangent points per edge, shared
        let surfs = box_patches(Vec3::new(20.0, 10.0, 16.0));
        let p = Patch::closed(&surfs);
        assert_eq!(p.count(), 6);
        assert_eq!(p.contour().len(), 8 + 24);
        assert_eq!(p.cv.len(), 8 + 24 + 24);
        // each face's middle outwards, its normal the face's
        let mid = Vec3::new(0.0, 5.0, 0.0);
        for cv in p.surfaces(Vec3::ONE) {
            let (s, (_, _, n)) = eval_frame(&cv, 0.5, 0.5);
            assert!((s - mid).normalize().dot(n) > 0.99, "{s} {n}");
        }
        // a cylinder: two rings of 12 and 2 tangent points on each of its 4
        // seams up the side
        let surfs = cylinder_patches(10.0, 30.0);
        let p = Patch::closed(&surfs);
        assert_eq!(p.count(), 6);
        assert_eq!(p.contour().len(), 24 + 8);
        for (k, cv) in p.surfaces(Vec3::ONE).iter().enumerate() {
            for (u, v) in [(0.5, 0.5), (0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.2, 0.7)] {
                let (s, (_, _, n)) = eval_frame(cv, u, v);
                if k < 4 {
                    // the side: on the circle, its normal out from the axis
                    let r = Vec3::new(s.x, 0.0, s.z);
                    assert!((r.length() - 10.0).abs() < 0.03, "radius {}", r.length());
                    assert!(r.normalize().dot(n) > 0.99, "side {k} at {u} {v}: {n}");
                } else {
                    // a cap: flat, its normal along the axis, out
                    let want = if s.y > 15.0 { Vec3::Y } else { -Vec3::Y };
                    assert!(n.dot(want) > 0.99, "cap {k} at {u} {v}: {n}");
                    assert!(Vec3::new(s.x, 0.0, s.z).length() < 10.03);
                }
            }
        }
        // the cap's middle at the axis
        let (s, _) = eval_frame(&p.surfaces(Vec3::ONE)[4], 0.5, 0.5);
        assert!(Vec3::new(s.x, 0.0, s.z).length() < 0.05, "{s}");
    }

    #[test]
    fn closed_layers_are_watertight() {
        for (surfs, counts) in [
            (box_patches(Vec3::new(20.0, 10.0, 16.0)), None),
            (cylinder_patches(10.0, 30.0), Some(vec![(4, 6), (4, 6), (4, 6), (4, 6), (4, 4), (4, 4)])),
        ] {
            let c = counts.unwrap_or_else(|| surfs.iter().map(|_| (3, 3)).collect());
            let (pts, faces, on, uv) = grid_closed(&surfs, &c);
            assert_eq!(pts.len(), on.len());
            assert_eq!(pts.len(), uv.len());
            assert!(closed_and_oriented(&faces), "{} points, {} faces", pts.len(), faces.len());
            // Euler: a sphere's
            let edges = faces.iter().map(|f| f.len()).sum::<usize>() / 2;
            assert_eq!(pts.len() as i64 - edges as i64 + faces.len() as i64, 2);
            // the faces turn outwards (the solid's volume positive)
            let vol: f32 = faces.iter().map(|f| (1..f.len() - 1).map(|k| pts[f[0] as usize].dot(pts[f[k] as usize].cross(pts[f[k + 1] as usize]))).sum::<f32>()).sum::<f32>() / 6.0;
            assert!(vol > 0.0, "volume {vol}");
        }
        // the cube's 3 x 3 grids: 6 * 16 points less the seams'
        let (pts, faces, _, _) = grid_closed(&box_patches(Vec3::splat(10.0)), &[(3, 3); 6]);
        assert_eq!((pts.len(), faces.len()), (56, 54));
    }

    #[test]
    fn a_seam_moved_stays_closed() {
        // a cube's top corner raised, with its tangent points: every patch
        // holding it follows, the layer's points on the seams stay on all of
        // them
        let surfs = box_patches(Vec3::new(20.0, 10.0, 16.0));
        let mut p = Patch::closed(&surfs);
        let (pts, faces, on, uv) = grid_closed(&surfs, &[(4, 4); 6]);
        p.bind = uv.iter().map(|t| [t[0], t[1], 0.0, 0.0, 0.0]).collect();
        p.on = on.clone();
        let top = (0..p.cv.len()).find(|k| Vec3::from(p.cv[*k]).distance(Vec3::new(10.0, 10.0, 8.0)) < 1e-4).unwrap();
        assert!(p.corner(top));
        let hs = p.handles(top);
        assert_eq!(hs.len(), 3);
        for k in std::iter::once(top).chain(hs) {
            p.cv[k][1] += 6.0;
        }
        p.fill_inside();
        let s = p.surfaces(Vec3::ONE);
        let now: Vec<Vec3> = p.bind.iter().enumerate().map(|(i, b)| place(&s[p.on_of(i)], *b)).collect();
        // each seam point: where any other patch's border puts it too
        for (i, q) in now.iter().enumerate() {
            let b = p.bind[i];
            let on_border = b[0] < 1e-4 || b[0] > 1.0 - 1e-4 || b[1] < 1e-4 || b[1] > 1.0 - 1e-4;
            if !on_border {
                continue;
            }
            let seeds = seeds_multi(&s, 12);
            let (k, bb) = bind_multi(&s, *q, &seeds);
            assert!(place(&s[k], bb).distance(*q) < 1e-3);
            assert!(bb[4].abs() < 1e-3, "point {i} off the object by {}", bb[4]);
        }
        assert!(closed_and_oriented(&faces));
        assert!(now.iter().zip(&pts).any(|(a, b)| a.distance(*b) > 1.0));
    }

    #[test]
    fn bound_on_a_cylinder() {
        // points on its side, its caps (their corners too) and 2 mm out of
        // it: bound and placed back where they were
        let s = cylinder_patches(10.0, 30.0);
        let seeds = seeds_multi(&s, 12);
        let mut tries = vec![];
        for a in [0.1f32, 0.8, 1.6, 2.5, 3.9, 5.5] {
            for y in [0.0, 4.0, 15.0, 29.0, 30.0] {
                let r = if y == 0.0 || y == 30.0 { 6.0 } else { 10.0 };
                tries.push(Vec3::new(r * a.cos(), y, -r * a.sin()));
            }
            tries.push(Vec3::new(12.0 * a.cos(), 12.0, -12.0 * a.sin()));
            tries.push(Vec3::new(10.0 * a.cos(), 30.0, -10.0 * a.sin()));
        }
        tries.push(Vec3::new(0.0, 30.0, 0.0));
        tries.push(Vec3::new(10.0, 30.0, 0.0));
        for q in tries {
            let (k, b) = bind_multi(&s, q, &seeds);
            assert!(place(&s[k], b).distance(q) < 0.02, "{q}: patch {k} {b:?} gives {}", place(&s[k], b));
        }
    }

    #[test]
    fn nearest_points() {
        let from: Vec<Vec3> = (0..500).map(|i| Vec3::new((i % 10) as f32, ((i / 10) % 10) as f32, (i / 100) as f32 * 3.0)).collect();
        let to = vec![Vec3::new(4.2, 7.9, 6.1), Vec3::new(-5.0, 0.0, 0.0), Vec3::new(9.4, 9.6, 30.0)];
        let got = nearest_of(&from, &to);
        for (t, g) in to.iter().zip(&got) {
            let best = from.iter().map(|f| f.distance(*t)).fold(f32::MAX, f32::min);
            assert!((from[*g].distance(*t) - best).abs() < 1e-5);
        }
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
    fn facet_points_settle_on_the_surface() {
        // a domed patch, its 8 x 8 grid bound; a point in the middle of a
        // cell on its flat quad (an inset's corner) and one 1.5 mm over the
        // surface (pulled by hand)
        let mut p = flat();
        for k in [5, 6, 9, 10] {
            p.cv[k][1] = 18.0;
        }
        let cv = p.cvs(Vec3::ONE);
        let (pts, faces, uv) = grid(&cv, 8, 8);
        let mut bs: Vec<[f32; 5]> = uv.iter().map(|t| [t[0], t[1], 0.0, 0.0, 0.0]).collect();
        let seeds = seeds(&cv, 24);
        let q = &faces[2 * 8 + 3];
        let mid = q.iter().map(|v| pts[*v as usize]).sum::<Vec3>() / 4.0;
        let bm = bind(&cv, mid, &seeds);
        assert!(bm[4].abs() > OFF, "the facet's middle is off the dome: {}", bm[4]);
        bs.push(bm);
        let (s, su, sv) = eval(&cv, 0.6, 0.4);
        let high = s + frame(su, sv).2 * 1.5;
        bs.push(bind(&cv, high, &seeds));
        let n = settle_on(&cv, &mut bs, &faces);
        assert_eq!(n, 1);
        let k = bs.len();
        assert_eq!(bs[k - 2][4], 0.0);
        assert!((bs[k - 1][4] - 1.5).abs() < 1e-3);
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
