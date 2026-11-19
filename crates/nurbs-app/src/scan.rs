//! Forme's tools reading the reference mesh (retopology, plan point 22).
//!
//! Step 2, the Cylinder read from the scan: with the Cylinder tool (C), a
//! drag that starts on the reference (`scan_under`) goes across a
//! round part of it, rim to rim, as Forme Rectify's Circle. Released, the
//! scan is read there once (`rectify_core::slot::Slot::detect_round`: the
//! wall going round, its centre, radius, axis and depth) and a real
//! Cylinder step goes in the history, sized and turned as read: a Carve for
//! a hole (begun a little outside its rim, so it cuts cleanly), an Add for a
//! boss, a button, a rivet. Nothing round there: nothing made, and a toast
//! says so.
//!
//! Step 3, the deviation: the part's mesh coloured by its distance to the
//! scan (green within the tolerance, yellow to red outside it, cyan to blue
//! inside), measured in a thread for each finished model (the BVH's closest
//! point within `DEV_REACH`, signed by the scan triangle's normal there).
//!
//! Step 5, Slot and Box read from the scan: the Slot tool (S), a drag along
//! a slot (or a raised pill) of the scan, end to end, and the Cube's base
//! drawn on the scan corner to corner across a block (or a pocket), read on
//! release (`Slot::detect`, `Slot::detect_box`, as Rectify's Slot and Box):
//! a Sketch step, a rectangle whose corners are rounded as read (a slot's
//! at half its width: round ends), a Carve when cut in, an Add when raised.
//! Nothing read: the Slot makes nothing, the Cube goes on to its height as
//! drawn.
//!
//! Step 6, a fillet's radius read on the scan: in Edge mode, Fit to scan
//! (the operation window, the marking menu) on sharp edges of the part
//! laid on the scan, or on a fillet between flat faces: the scan cut in
//! slices across each edge (`rectify_core::fillet::read`: the scan's two
//! faces and the arc tangent to both in each slice), the fillet made or
//! sized at the slices' median radius, the arcs read shown while it is
//! selected.
//!
//! Step 4, the plane under the pointer: Cube, Sketch and Path start on the
//! scan's flat face under the pointer (`rectify_core::planes::plane_under`, as Forme Rectify's Pen),
//! else its tangent plane there, when the part is not in front of it.

use crate::render::MeshVertex;
use crate::state::{AppState, BasePlane, Creation, Tool};
use glam::{DQuat, DVec3, Vec3};
use nurbs_kernel::doc::Profile;
use nurbs_kernel::model::SurfKind;
use nurbs_kernel::{FaceTag, FeatureId, Model, Op, Prim};
use rectify_core::fillet::{EdgeSample, FilletRead};
use rectify_core::planes::{dominant_normal, plane_under, tangent_under};
use rectify_core::primary::{Primaries, Primary};
use rectify_core::slot::Slot;

/// A drag across a round part of the scan.
#[derive(Clone, Debug)]
pub struct ScanRound {
    /// The triangle pressed, and the scan under the press and under the
    /// pointer: the diameter drawn.
    pub tri: u32,
    pub a: DVec3,
    pub b: DVec3,
    /// The way most of the scan faces round the press (towards the eye):
    /// the ring is drawn square to it.
    pub n: DVec3,
}

impl ScanRound {
    /// The drawn ring's axis: the plane's normal turned square to the
    /// diameter, so the ring goes through both ends.
    pub fn ring_normal(&self) -> DVec3 {
        let u = (self.b - self.a).normalize_or_zero();
        let m = (self.n - u * self.n.dot(u)).normalize_or_zero();
        if m == DVec3::ZERO {
            self.n
        } else {
            m
        }
    }

    pub fn radius(&self) -> f64 {
        (self.b - self.a).length() * 0.5
    }

    /// The ring drawn (closed polyline).
    pub fn ring(&self, segs: usize) -> Vec<DVec3> {
        let c = (self.a + self.b) * 0.5;
        let r = self.radius();
        let n = self.ring_normal();
        let x = (self.a - c).normalize_or_zero();
        let x = if x == DVec3::ZERO { n.any_orthonormal_vector() } else { x };
        let y = n.cross(x);
        (0..=segs).map(|i| (i as f64 / segs as f64) * std::f64::consts::TAU).map(|t| c + (x * t.cos() + y * t.sin()) * r).collect()
    }
}

/// What the scan gave for a round part: the cylinder step to make.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoundRead {
    pub op: Op,
    pub radius: f64,
    pub height: f64,
    /// The centre of the cylinder's base, and its axis (+Y turned there).
    pub origin: DVec3,
    pub axis: DVec3,
    /// How deep the hole or how high the boss was read (mm).
    pub depth: f64,
}

/// A hole's cylinder begins this far outside its rim (mm, at most).
const CLEAR: f64 = 0.5;

/// An axis or a face's normal read within this of a world axis is taken as
/// that axis (an AI-made mesh is a little off: the pistol's holes read 1.4°
/// off X, its side's flat faces 1.5° to 3.7° off Z).
const AXIS_SNAP_DEG: f64 = 5.0;

/// The world axis near `a` (within `AXIS_SNAP_DEG`), else `a`.
pub fn snap_axis(a: DVec3) -> DVec3 {
    let cos = AXIS_SNAP_DEG.to_radians().cos();
    for w in [DVec3::X, DVec3::Y, DVec3::Z] {
        let c = a.dot(w);
        if c.abs() >= cos {
            return w * c.signum();
        }
    }
    a
}

/// From the scan's round part to a cylinder step.
pub fn round_step(s: &Slot) -> Option<RoundRead> {
    let axis = snap_axis(s.axis.normalize_or_zero());
    if axis == DVec3::ZERO || !(s.r > 0.0) {
        return None;
    }
    // a depth the scan cannot hold (a wall read past its end): none
    if !s.depth.is_finite() || s.depth > (s.r * 20.0).max(30.0) || s.depth < 0.05 {
        return None;
    }
    Some(if s.inward {
        let clear = (s.r * 0.1).clamp(0.1, CLEAR);
        RoundRead { op: Op::Carve, radius: s.r, height: s.depth + clear, origin: s.a - axis * clear, axis, depth: s.depth }
    } else {
        RoundRead { op: Op::Add, radius: s.r, height: s.depth, origin: s.a, axis, depth: s.depth }
    })
}

/// From the scan's round part to its cylinder steps: one, or one per step
/// of a round part in steps (a button on its ring, a counterbore), from the
/// outer one in, all round the outer one's centre when they are within a
/// tenth of its radius of it (one part turned: its steps share an axis),
/// each inner one begun a little inside the one before (no faces laid flush
/// on each other).
pub fn round_steps(s: &Slot) -> Vec<RoundRead> {
    let Some(one) = round_step(s) else { return vec![] };
    if s.tiers.len() < 2 {
        return vec![one];
    }
    let axis = one.axis;
    let c0 = s.tiers[0].centre;
    let mut out = vec![];
    for (k, t) in s.tiers.iter().enumerate() {
        let h = t.to - t.from;
        if !(h > 0.05) || !(t.r > 0.0) || t.to > (s.r * 20.0).max(30.0) {
            return vec![one];
        }
        let off = t.centre - c0;
        let c = if (off - axis * axis.dot(off)).length() < s.tiers[0].r * 0.1 { c0 } else { t.centre };
        out.push(if k == 0 {
            if s.inward {
                let clear = (t.r * 0.1).clamp(0.1, CLEAR);
                RoundRead { op: Op::Carve, radius: t.r, height: h + clear, origin: c - axis * clear, axis, depth: h }
            } else {
                RoundRead { op: Op::Add, radius: t.r, height: h, origin: c, axis, depth: h }
            }
        } else {
            let into = ((s.tiers[k - 1].to - s.tiers[k - 1].from) * 0.2).min(0.1);
            RoundRead { op: if s.inward { Op::Carve } else { Op::Add }, radius: t.r, height: h + into, origin: c + axis * (t.from - into), axis, depth: h }
        });
    }
    out
}

/// A drag along a slot of the scan (the Slot tool): the triangle pressed,
/// the scan under the press and under the pointer, and the way most of the
/// scan faces round the press (towards the eye).
#[derive(Clone, Debug)]
pub struct ScanSlot {
    pub tri: u32,
    pub a: DVec3,
    pub b: DVec3,
    pub n: DVec3,
}

/// A slot or a box read on the scan, as a Sketch step: a rectangle (`x`
/// along the centre line, `z` across) whose corners are rounded at `r` (a
/// slot's at half its width: round ends), from `origin` along `rot`'s Y.
#[derive(Clone, Debug)]
pub struct OutlineRead {
    pub op: Op,
    pub x: f64,
    pub z: f64,
    pub r: f64,
    pub height: f64,
    pub origin: DVec3,
    pub rot: DQuat,
    /// How deep the slot or high the block was read (mm).
    pub depth: f64,
}

impl OutlineRead {
    pub fn profile(&self) -> Profile {
        Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![self.r; 4], tension: vec![], open: false }
    }
}

/// From the scan's slot or box to a sketch step: its axis and its length's
/// direction each taken as a world axis within 5°; a cut-in one begun a
/// little outside its rim (as a hole).
pub fn outline_step(s: &Slot) -> Option<OutlineRead> {
    let axis = snap_axis(s.axis.normalize_or_zero());
    if axis == DVec3::ZERO || !(s.r > 0.0) || !(s.w >= 0.0) {
        return None;
    }
    let flat = |v: DVec3| v - axis * axis.dot(v);
    let d = flat(s.b - s.a);
    let size = d.length() + 2.0 * (s.r + s.w);
    if !s.depth.is_finite() || s.depth > (size * 10.0).max(30.0) || s.depth < 0.05 {
        return None;
    }
    let e = if d.length() > 1e-9 { d.normalize() } else { BasePlane::new(DVec3::ZERO, axis, None).u };
    let k = flat(snap_axis(e));
    let e = if k.length() > 0.9 { k.normalize() } else { e };
    let len = d.dot(e).abs();
    let mid = (s.a + s.b) * 0.5;
    let rot = crate::state::basis_rotation(e, axis);
    let (x, z) = (len + 2.0 * s.r, 2.0 * (s.w + s.r));
    Some(if s.inward {
        let clear = (s.r * 0.1).clamp(0.1, CLEAR);
        OutlineRead { op: Op::Carve, x, z, r: s.r, height: s.depth + clear, origin: mid - axis * clear, rot, depth: s.depth }
    } else {
        OutlineRead { op: Op::Add, x, z, r: s.r, height: s.depth, origin: mid, rot, depth: s.depth }
    })
}

/// A primary shape read on the scan (the "scan → shapes" button's first
/// stage), as a step: a Cube when its outline is a rectangle, else a Sketch
/// of its outline, pulled along the side axis from its back to its front.
#[derive(Clone, Debug)]
pub struct PrimaryRead {
    pub prim: Prim,
    pub profile: Option<Profile>,
    pub origin: DVec3,
    pub rot: DQuat,
}

pub fn primary_step(p: &Primaries, s: &Primary) -> Option<PrimaryRead> {
    let h = s.thickness();
    if !(h > 0.05) || s.outline.len() < 3 {
        return None;
    }
    let to3 = |q: glam::DVec2| p.u * q.x + p.v * q.y;
    if s.rect {
        let c = &s.outline;
        let (a, b) = (c[1] - c[0], c[3] - c[0]);
        let centre = (c[0] + c[2]) * 0.5;
        let rot = crate::state::basis_rotation(to3(a), p.w);
        return Some(PrimaryRead { prim: Prim::Box { x: a.length(), y: h, z: b.length() }, profile: None, origin: p.at(centre, s.back), rot });
    }
    let (lo, hi) = s.outline.iter().fold((glam::DVec2::splat(f64::INFINITY), glam::DVec2::splat(f64::NEG_INFINITY)), |(a, b), q| (a.min(*q), b.max(*q)));
    let size = hi - lo;
    if !(size.x > 0.1 && size.y > 0.1) {
        return None;
    }
    let centre = (lo + hi) * 0.5;
    let rot = crate::state::basis_rotation(p.u, p.w);
    // the sketch's local Z is X × Y: along ±v
    let flip = p.u.cross(p.w).dot(p.v).signum();
    let points: Vec<[f64; 2]> = s.outline.iter().map(|q| [(q.x - centre.x) / size.x, flip * (q.y - centre.y) / size.y]).collect();
    let n = points.len();
    // the corners read rounded (> 0, a radius) or chamfered (< 0)
    let corner = if s.corner.iter().any(|c| *c != 0.0) { s.corner.iter().copied().take(n).collect() } else { vec![] };
    let profile = Profile { points, smooth: vec![false; n], corner, tension: vec![], open: false };
    Some(PrimaryRead { prim: Prim::Profile { x: size.x, y: h, z: size.y }, profile: Some(profile), origin: p.at(centre, s.back), rot })
}

/// Past this distance from the scan (mm), a point of the part is not
/// coloured (a face inside the scan, a part modelled beside it).
pub const DEV_REACH: f64 = 5.0;

/// The part's distance to the reference, for one model.
#[derive(Clone)]
pub struct Deviation {
    /// The model's serial and the reference's load it was measured on.
    pub serial: u64,
    pub ref_gen: u64,
    /// The part's triangles cut finer (`refine`), each corner with its
    /// distance (mm, + outside the scan; `NO_DEV` past the reach).
    pub verts: Vec<MeshVertex>,
    /// Of the points measured within reach: how many, how many within the
    /// tolerance it was asked with, the median and the 95th percentile of
    /// their distance (mm).
    pub near: usize,
    pub within: usize,
    pub tol: f64,
    pub p50: f64,
    pub p95: f64,
    pub ms: f32,
}

/// Each point's signed distance to the scan (in 4 threads): the values
/// (`NO_DEV` past `DEV_REACH`), how many within reach, how many of those
/// within `tol`, the median and 95th percentile of their distance.
pub fn measure(r: &crate::reference::Reference, pts: &[[f32; 3]], tol: f64) -> (Vec<f32>, usize, usize, f64, f64) {
    let n = pts.len();
    let mut values = vec![crate::render::NO_DEV; n];
    let chunk = n.div_ceil(4).max(1);
    std::thread::scope(|s| {
        for (k, out) in values.chunks_mut(chunk).enumerate() {
            s.spawn(move || {
                for (i, o) in out.iter_mut().enumerate() {
                    let p = DVec3::from(pts[k * chunk + i].map(|c| c as f64));
                    if let Some((t, q)) = r.bvh.closest(&r.mesh, p, DEV_REACH) {
                        let d = (p - q).length();
                        let side = (p - q).dot(r.mesh.tri_normal(t as usize));
                        *o = if side < 0.0 { -d } else { d } as f32;
                    }
                }
            });
        }
    });
    let near: Vec<f64> = values.iter().filter(|v| v.abs() < 1e8).map(|v| *v as f64).collect();
    let within = near.iter().filter(|v| v.abs() <= tol).count();
    let mut abs: Vec<f64> = near.iter().map(|v| v.abs()).collect();
    abs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = abs.get(((abs.len() as f64 * 0.95) as usize).min(abs.len().saturating_sub(1))).copied().unwrap_or(0.0);
    let p50 = abs.get(abs.len() / 2).copied().unwrap_or(0.0);
    (values, near.len(), within, p50, p95)
}

/// The kernel's triangles are long (a cylinder's side runs rim to rim):
/// measured at their corners only, a hole would be coloured by its rims.
/// Each triangle is cut in k × k (its longest side / k near `target`, at
/// most 64), the points shared inside it, and each point measured.
pub fn refine_measure(r: &crate::reference::Reference, m: &Model, tol: f64) -> Deviation {
    let t0 = std::time::Instant::now();
    let p = |v: u32| DVec3::from(m.positions[v as usize].map(|c| c as f64));
    let nv = |v: u32| DVec3::from(m.normals[v as usize].map(|c| c as f64));
    // the cut: about 0.3 mm on a 120 mm part, at most ~300 k triangles
    let diag = m.bounds().map(|(lo, hi)| (DVec3::from(hi.map(|c| c as f64)) - DVec3::from(lo.map(|c| c as f64))).length()).unwrap_or(10.0);
    let mut target = (diag / 400.0).max(0.2);
    let cuts = |target: f64| -> Vec<usize> {
        m.triangles
            .iter()
            .map(|t| {
                let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                let l = (a - b).length().max((b - c).length()).max((c - a).length());
                ((l / target).ceil() as usize).clamp(1, 64)
            })
            .collect()
    };
    let mut ks = cuts(target);
    while ks.iter().map(|k| k * k).sum::<usize>() > 300_000 && target < diag {
        target *= 1.4;
        ks = cuts(target);
    }
    // each triangle's grid of points (barycentric), then all measured
    let mut pts: Vec<[f32; 3]> = vec![];
    let mut nrm: Vec<[f32; 3]> = vec![];
    let mut first: Vec<usize> = Vec::with_capacity(ks.len());
    for (t, tri) in m.triangles.iter().enumerate() {
        let k = ks[t];
        first.push(pts.len());
        let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
        let (na, nb, nc) = (nv(tri[0]), nv(tri[1]), nv(tri[2]));
        for i in 0..=k {
            for j in 0..=(k - i) {
                let (u, v) = (i as f64 / k as f64, j as f64 / k as f64);
                let w = 1.0 - u - v;
                pts.push((a * w + b * u + c * v).as_vec3().to_array());
                nrm.push((na * w + nb * u + nc * v).normalize_or_zero().as_vec3().to_array());
            }
        }
    }
    let (values, near, within, p50, p95) = measure(r, &pts, tol);
    // the grid's triangles
    let mut verts = Vec::with_capacity(ks.iter().map(|k| k * k * 3).sum());
    for (t, &k) in ks.iter().enumerate() {
        let face = m.tri_face[t];
        let base = first[t];
        // row i (its points j = 0 ..= k - i) starts after the rows before it
        let row = |i: usize| base + (0..i).map(|q| k + 1 - q).sum::<usize>();
        let vx = |q: usize| MeshVertex { pos: pts[q], nrm: nrm[q], face, dev: values[q] };
        for i in 0..k {
            let (r0, r1) = (row(i), row(i + 1));
            for j in 0..(k - i) {
                // (i, j), (i + 1, j), (i, j + 1)
                verts.extend_from_slice(&[vx(r0 + j), vx(r1 + j), vx(r0 + j + 1)]);
                if j + 1 < k - i {
                    // (i + 1, j), (i + 1, j + 1), (i, j + 1)
                    verts.extend_from_slice(&[vx(r1 + j), vx(r1 + j + 1), vx(r0 + j + 1)]);
                }
            }
        }
    }
    Deviation { serial: m.serial, ref_gen: 0, verts, near, within, tol, p50, p95, ms: t0.elapsed().as_secs_f32() * 1000.0 }
}

impl AppState {
    /// The deviation of the model shown, when it is on and measured.
    pub fn deviation_shown(&self) -> Option<&[MeshVertex]> {
        if !self.dev_on {
            return None;
        }
        let m = self.model.as_ref()?;
        let d = self.deviation.as_ref()?;
        (d.serial == m.serial && d.ref_gen == self.ref_gen && self.reference.is_some()).then_some(d.verts.as_slice())
    }

    /// The deviation on or off (the part is uploaded again).
    pub fn set_deviation(&mut self, on: bool) {
        self.dev_on = on;
        self.model_gen += 1;
    }

    /// Being measured.
    pub fn dev_busy(&self) -> bool {
        self.dev_on && self.reference.is_some() && (self.dev_job.is_some() || self.deviation_shown().is_none())
    }

    /// Once per frame: takes the measure made, starts one for a new
    /// finished model.
    pub(crate) fn deviation_tick(&mut self) {
        if let Some(rx) = &self.dev_job {
            match rx.try_recv() {
                Ok(d) => {
                    self.dev_job = None;
                    self.deviation = Some(d);
                    self.model_gen += 1;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(_) => self.dev_job = None,
            }
        }
        if !self.dev_on {
            return;
        }
        let (Some(m), Some(r)) = (self.model.clone(), self.reference.clone()) else { return };
        if !m.fine || self.is_live() || m.serial / 4 != self.version {
            return;
        }
        if self.deviation.as_ref().map(|d| d.serial == m.serial && d.ref_gen == self.ref_gen && d.tol == self.dev_tol).unwrap_or(false) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (ref_gen, tol) = (self.ref_gen, self.dev_tol);
        std::thread::spawn(move || {
            let mut d = refine_measure(&r, &m, tol);
            d.ref_gen = ref_gen;
            let _ = tx.send(d);
        });
        self.dev_job = Some(rx);
    }

    /// The reference under a point of the screen when it is shown: the
    /// triangle and the point. The part modelled on it lies on it or a
    /// little off it: the scan counts unless the part is clearly in front
    /// (by 2 % of the scan's size, at least 0.5 mm); seen through (Ghost),
    /// it always counts.
    pub fn scan_under(&self, p: [f32; 2]) -> Option<(u32, DVec3)> {
        let r = self.reference.as_ref().filter(|_| self.ref_shown)?;
        let (o, d) = self.ray(p);
        let d = d.normalize();
        let (t, x, s) = r.hit_tri(o, d)?;
        if !self.ref_ghost {
            let tol = ((r.hi - r.lo).length() * 0.02).max(0.5);
            if let Some(h) = self.pick_at(p) {
                if (h.t as f64) < s - tol {
                    return None;
                }
            }
        }
        Some((t, x))
    }

    /// The Cylinder pressed on the scan: true when it was (the drag goes
    /// across a round part of it), false to create as usual.
    pub fn scan_round_press(&mut self, p: [f32; 2]) -> bool {
        let Some((t, x)) = self.scan_under(p) else { return false };
        let Some(r) = self.reference.clone() else { return false };
        // the face round the rim, not the fillet or the wall the press is
        // on: the ring is drawn square to it
        let reach = (self.mm_per_point(x) * 60.0).clamp(1.0, 20.0);
        let mut n = dominant_normal(&r.mesh, &r.topo, t, x, reach);
        if n.dot(self.ray(p).1) > 0.0 {
            n = -n;
        }
        self.scan_round = Some(ScanRound { tri: t, a: x, b: x, n });
        true
    }

    /// The pointer moving: the ring follows it (the scan under it, else
    /// the plane through the press).
    pub fn scan_round_move(&mut self, p: [f32; 2]) {
        let Some(sr) = &self.scan_round else { return };
        let (o, d) = self.ray(p);
        let d = d.normalize();
        let on_scan = self.reference.as_ref().and_then(|r| r.hit(o, d)).map(|h| h.0);
        let on_plane = {
            let den = d.dot(sr.n);
            (den.abs() > 1e-9).then(|| o + d * ((sr.a - o).dot(sr.n) / den)).filter(|q| (*q - o).dot(d) > 0.0)
        };
        if let (Some(b), Some(sr)) = (on_scan.or(on_plane), &mut self.scan_round) {
            sr.b = b;
        }
    }

    /// Released: the scan read once there; a cylinder step made as read.
    pub fn scan_round_release(&mut self) {
        let Some(sr) = self.scan_round.take() else { return };
        let Some(r) = self.reference.clone() else { return };
        let rad = sr.radius();
        if rad * 2.0 < self.mm_per_point(sr.a) * 4.0 {
            return;
        }
        let c = (sr.a + sr.b) * 0.5;
        let found = Slot::detect_round(&r.mesh, &r.topo, sr.tri, sr.a, sr.b, r.edge);
        if std::env::var_os("FORME_SCAN_DEBUG").is_some() {
            match &found {
                Ok(s) => eprintln!("scan round: drawn Ø {:.2} at {c:.2?}; found Ø {:.2} at {:.2?} axis {:.3?} depth {:.2} inward {}", rad * 2.0, s.r * 2.0, s.a, s.axis, s.depth, s.inward),
                Err(e) => eprintln!("scan round: drawn Ø {:.2} at {c:.2?}: {e}", rad * 2.0),
            }
        }
        // about the size drawn, about where it was drawn
        let found = found.ok().filter(|s| {
            let off = s.a - c;
            let off = off - s.axis * s.axis.dot(off);
            (s.r - rad).abs() <= rad * 0.5 && off.length() <= rad
        });
        let reads = found.as_ref().map(round_steps).unwrap_or_default();
        let Some(read) = reads.first().copied() else {
            let why = if found.is_some() { "its depth could not be read" } else { "no wall goes round the circle drawn" };
            self.toast(&format!("Nothing round read there ({why}) · drag across it, rim to rim"), true);
            return;
        };
        let kind = if read.op == Op::Carve { "Hole" } else { "Boss" };
        let rot = DQuat::from_rotation_arc(DVec3::Y, read.axis);
        let mat = self.material;
        let mut ids = vec![];
        self.edit(&format!("{kind} from the scan"), |d| {
            for r in &reads {
                let id = d.add(r.op, Prim::Cylinder { radius: r.radius, height: r.height }, r.origin, rot);
                let name = d.fresh_name(kind);
                if let Some(f) = d.feature_mut(id) {
                    f.name = name;
                    f.material = mat;
                }
                ids.push(id);
            }
        });
        self.selection = ids;
        let fmt = |v: f64| crate::widgets::fmt_mm(v as f32);
        let way = if read.op == Op::Carve { "deep" } else { "high" };
        if reads.len() > 1 {
            let steps: Vec<String> = reads.iter().map(|r| format!("Ø {} × {}", fmt(r.radius * 2.0), fmt(r.depth))).collect();
            self.toast(&format!("{kind} read on the scan in {} steps: {} mm", reads.len(), steps.join(", ")), false);
        } else {
            self.toast(&format!("{kind} read on the scan: Ø {} mm, {} mm {way}", fmt(read.radius * 2.0), fmt(read.depth)), false);
        }
    }

    pub fn scan_round_cancel(&mut self) {
        self.scan_round = None;
    }

    /// The scan under the pointer while a drag on it goes on: on it, else
    /// on the plane through `a` facing `n`.
    fn scan_drag_point(&self, a: DVec3, n: DVec3, p: [f32; 2]) -> Option<DVec3> {
        let (o, d) = self.ray(p);
        let d = d.normalize();
        let on_scan = self.reference.as_ref().and_then(|r| r.hit(o, d)).map(|h| h.0);
        let den = d.dot(n);
        let on_plane = (den.abs() > 1e-9).then(|| o + d * ((a - o).dot(n) / den)).filter(|q| (*q - o).dot(d) > 0.0);
        on_scan.or(on_plane)
    }

    /// The Slot tool (S): reads the scan only.
    pub fn arm_slot(&mut self) {
        self.abort_gestures();
        self.tool = Tool::Slot;
        self.edge_sel.clear();
        self.face_sel.clear();
        self.vert_sel.clear();
    }

    /// A press with the Slot tool: on the scan, the drag along a slot
    /// begins (true); elsewhere a toast says what it wants.
    pub fn scan_slot_press(&mut self, p: [f32; 2]) -> bool {
        let Some((t, x)) = self.scan_under(p) else {
            self.toast("The Slot reads the scan: drag along a slot on it, from one end to the other", true);
            return false;
        };
        let Some(r) = self.reference.clone() else { return false };
        let reach = (self.mm_per_point(x) * 60.0).clamp(1.0, 20.0);
        let mut n = dominant_normal(&r.mesh, &r.topo, t, x, reach);
        if n.dot(self.ray(p).1) > 0.0 {
            n = -n;
        }
        self.scan_slot = Some(ScanSlot { tri: t, a: x, b: x, n });
        true
    }

    pub fn scan_slot_move(&mut self, p: [f32; 2]) {
        let Some(ss) = &self.scan_slot else { return };
        if let Some(b) = self.scan_drag_point(ss.a, ss.n, p) {
            if let Some(ss) = &mut self.scan_slot {
                ss.b = b;
            }
        }
    }

    /// Released: the scan read once along the drag; a sketch step made as
    /// read (a Carve "Slot" cut in, an Add "Pill" raised).
    pub fn scan_slot_release(&mut self) {
        let Some(ss) = self.scan_slot.take() else { return };
        let Some(r) = self.reference.clone() else { return };
        let len = (ss.b - ss.a).length();
        if len < self.mm_per_point(ss.a) * 6.0 {
            return;
        }
        let c = (ss.a + ss.b) * 0.5;
        let found = Slot::detect(&r.mesh, &r.topo, ss.tri, ss.a, ss.b, r.edge);
        if std::env::var_os("FORME_SCAN_DEBUG").is_some() {
            match &found {
                Ok(s) => eprintln!("scan slot: drawn {len:.2} at {c:.2?}; found {:.2} × {:.2} r {:.2} at {:.2?} axis {:.3?} depth {:.2} inward {}", (s.b - s.a).length() + 2.0 * s.r, 2.0 * (s.w + s.r), s.r, (s.a + s.b) * 0.5, s.axis, s.depth, s.inward),
                Err(e) => eprintln!("scan slot: drawn {len:.2} at {c:.2?}: {e}"),
            }
        }
        // about the length drawn, about where it was drawn
        let found = found.ok().filter(|s| {
            let off = (s.a + s.b) * 0.5 - c;
            let off = off - s.axis * s.axis.dot(off);
            let l = (s.b - s.a).length() + 2.0 * s.r;
            l >= len * 0.4 && l <= len * 2.5 && off.length() <= len * 0.5
        });
        let Some(read) = found.as_ref().and_then(outline_step) else {
            let why = if found.is_some() { "its depth could not be read" } else { "no slot along the drag" };
            self.toast(&format!("No slot read there ({why}) · drag along it, from one end to the other"), true);
            return;
        };
        let kind = if read.op == Op::Carve { "Slot" } else { "Pill" };
        let id = self.make_outline(&read, kind, None);
        self.selection = vec![id];
        let fmt = |v: f64| crate::widgets::fmt_mm(v as f32);
        let way = if read.op == Op::Carve { "deep" } else { "high" };
        self.toast(&format!("{kind} read on the scan: {} × {} mm, {} mm {way}", fmt(read.x), fmt(read.z), fmt(read.depth)), false);
    }

    /// The step read, made: a new feature (one undo step), or the feature
    /// being created turned into it (its live edit ended).
    fn make_outline(&mut self, read: &OutlineRead, kind: &str, live: Option<u32>) -> u32 {
        let mat = self.material;
        let prim = Prim::Profile { x: read.x, y: read.height, z: read.z };
        let pr = read.profile();
        let fill = |d: &mut nurbs_kernel::Doc, id: u32| {
            let name = d.fresh_name(kind);
            if let Some(f) = d.feature_mut(id) {
                f.name = name;
                f.op = read.op;
                f.prim = prim;
                f.origin = read.origin;
                f.rot = read.rot;
                f.profile = Some(pr.clone());
                f.material = mat;
                f.visible = true;
            }
        };
        match live {
            Some(id) => {
                self.live(|d| fill(d, id));
                self.end_live();
                id
            }
            None => {
                let mut id = 0;
                self.edit(&format!("{kind} from the scan"), |d| {
                    id = d.add(read.op, prim, read.origin, read.rot);
                    fill(d, id);
                });
                id
            }
        }
    }

    /// The Cube's base let go on the scan, corner to corner: the block or
    /// pocket there read (`Slot::detect_box`); when one is found about where
    /// and as big as drawn, the shape becomes it at once (true). `from`: the
    /// scan under the press.
    pub fn scan_box_read(&mut self, c: &Creation, from: (u32, DVec3), p: [f32; 2]) -> bool {
        let Some(r) = self.reference.clone() else { return false };
        let (o, d) = self.ray(p);
        let Some((_, to, _)) = r.hit_tri(o, d.normalize()) else { return false };
        let found = Slot::detect_box(&r.mesh, &r.topo, from.0, from.1, to, r.edge);
        // the drag as drawn on the scan, seen along the axis read (the base
        // plane may be the slope under the press, tilted: the corners on it
        // say less)
        let axis = found.as_ref().map(|s| s.axis).unwrap_or(c.plane.n);
        let drawn = (from.1 + to) * 0.5;
        let diag = (to - from.1).reject_from(axis).length();
        if std::env::var_os("FORME_SCAN_DEBUG").is_some() {
            match &found {
                Ok(s) => eprintln!("scan box: drawn {diag:.2} at {drawn:.2?}; found {:.2} × {:.2} r {:.2} at {:.2?} axis {:.3?} depth {:.2} inward {}", (s.b - s.a).length() + 2.0 * s.r, 2.0 * (s.w + s.r), s.r, (s.a + s.b) * 0.5, s.axis, s.depth, s.inward),
                Err(e) => eprintln!("scan box: drawn {diag:.2} at {drawn:.2?}: {e}"),
            }
        }
        let Some(read) = found.ok().and_then(|s| outline_step(&s)) else { return false };
        // about as big as drawn, about where it was drawn
        let size = read.x.hypot(read.z);
        let centre = read.origin + read.rot * DVec3::Y * if read.op == Op::Carve { read.height - read.depth } else { 0.0 };
        let off = (centre - drawn).reject_from(axis);
        if size < diag * 0.4 || size > diag * 2.0 || off.length() > diag * 0.4 {
            if std::env::var_os("FORME_SCAN_DEBUG").is_some() {
                eprintln!("scan box: not taken (size {size:.2} for {diag:.2} drawn, {:.2} off)", off.length());
            }
            return false;
        }
        let kind = if read.op == Op::Carve { "Pocket" } else { "Block" };
        self.creation = None;
        self.snap_shown = None;
        let id = self.make_outline(&read, kind, Some(c.id));
        self.selection = vec![id];
        let fmt = |v: f64| crate::widgets::fmt_mm(v as f32);
        let way = if read.op == Op::Carve { "deep" } else { "high" };
        self.toast(&format!("{kind} read on the scan: {} × {} mm, corners {} mm, {} mm {way}", fmt(read.x), fmt(read.z), fmt(read.r), fmt(read.depth)), false);
        true
    }
}

/// The arcs read on the scan for a fillet sized by Fit to scan.
#[derive(Clone, Debug)]
pub struct FilletShow {
    pub id: FeatureId,
    pub arcs: Vec<Vec<DVec3>>,
}

impl AppState {
    /// A model face's way away from an edge at `p` (square to the edge's
    /// direction `t` and to the face's normal `n`), and how far the face
    /// runs that way.
    fn face_away(&self, face: usize, p: DVec3, t: DVec3, n: DVec3) -> Option<(DVec3, f64)> {
        let m = self.model.as_ref()?;
        let side = t.cross(n).try_normalize()?;
        let pos = |i: u32| Vec3::from(m.positions[i as usize]).as_dvec3();
        let mut near = (f64::MAX, 0.0);
        let (mut hi, mut lo) = (0.0f64, 0.0f64);
        for (tri, fi) in m.triangles.iter().zip(&m.tri_face) {
            if *fi as usize != face {
                continue;
            }
            let c = (pos(tri[0]) + pos(tri[1]) + pos(tri[2])) / 3.0;
            let d = c.distance_squared(p);
            if d < near.0 {
                near = (d, (c - p).dot(side));
            }
            for &v in tri {
                let s = (pos(v) - p).dot(side);
                hi = hi.max(s);
                lo = lo.min(s);
            }
        }
        if near.1.abs() < 1e-9 {
            return None;
        }
        Some(if near.1 > 0.0 { (side, hi) } else { (-side, -lo) })
    }

    /// The edge sample at `p` along `t` between two model faces.
    fn edge_sample(&self, fa: usize, fb: usize, p: DVec3, t: DVec3) -> Option<EdgeSample> {
        let n = [self.normal_near(fa, p)?, self.normal_near(fb, p)?];
        let (d0, r0) = self.face_away(fa, p, t, n[0])?;
        let (d1, r1) = self.face_away(fb, p, t, n[1])?;
        Some(EdgeSample { p, t, n, d: [d0, d1], reach: [r0, r1] })
    }

    /// Up to ten samples along a sharp model edge (its ends left out: other
    /// faces meet there), at least 0.5 mm apart.
    fn edge_samples(&self, edge: usize) -> Vec<EdgeSample> {
        let Some(m) = self.model.as_ref() else { return vec![] };
        let Some(e) = m.edges.get(edge) else { return vec![] };
        let (fa, fb) = (e.faces[0], e.faces[1]);
        if fa < 0 || fb < 0 || fa == fb || e.pts.len() < 2 {
            return vec![];
        }
        let pts: Vec<DVec3> = e.pts.iter().map(|p| Vec3::from(*p).as_dvec3()).collect();
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            cum.push(cum.last().unwrap() + (w[1] - w[0]).length());
        }
        let len = *cum.last().unwrap();
        if len < 1e-6 {
            return vec![];
        }
        let closed = pts[0].distance(pts[pts.len() - 1]) < 1e-4;
        let k = ((len / 0.5).round() as usize).clamp(3, 10);
        let mut out = vec![];
        for i in 0..k {
            let f = if closed { (i as f64 + 0.5) / k as f64 } else { 0.12 + 0.76 * i as f64 / (k - 1) as f64 };
            let s = f * len;
            let j = cum.windows(2).position(|c| s <= c[1]).unwrap_or(pts.len() - 2);
            let seg = cum[j + 1] - cum[j];
            let u = if seg > 0.0 { (s - cum[j]) / seg } else { 0.0 };
            let p = pts[j].lerp(pts[j + 1], u);
            let Some(t) = (pts[j + 1] - pts[j]).try_normalize() else { continue };
            if let Some(es) = self.edge_sample(fa as usize, fb as usize, p, t) {
                out.push(es);
            }
        }
        out
    }

    /// Samples along each edge a fillet rounds, when both its faces are
    /// flat: the line where their planes meet, over the fillet's own faces'
    /// span along it.
    fn fillet_samples(&self, id: FeatureId) -> Vec<Vec<EdgeSample>> {
        let (Some(m), Some(f)) = (self.model.as_ref(), self.doc.feature(id)) else { return vec![] };
        let Some(round) = &f.round else { return vec![] };
        let own = FaceTag { feature: id, sub: 0 }.pack();
        let pos = |i: u32| Vec3::from(m.positions[i as usize]).as_dvec3();
        let mut out = vec![];
        for e in &round.edges {
            // the flat face of each tag whose plane passes nearest the edge
            let plane = |tag: i64| {
                m.faces
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| x.kind == SurfKind::Plane && x.tag.map(|t| t.pack()) == Some(tag))
                    .map(|(i, x)| (i, DVec3::from_array(x.point), DVec3::from_array(x.normal).normalize()))
                    .min_by(|a, b| (e.at - a.1).dot(a.2).abs().total_cmp(&(e.at - b.1).dot(b.2).abs()))
            };
            let (Some((fa, pa, na)), Some((fb, pb, nb))) = (plane(e.faces[0]), plane(e.faces[1])) else { continue };
            let Some(t) = na.cross(nb).try_normalize() else { continue };
            // the point of both planes nearest the edge's middle
            let g = na.dot(nb);
            let (ha, hb) = (na.dot(pa - e.at), nb.dot(pb - e.at));
            let den = 1.0 - g * g;
            let o = e.at + na * ((ha - g * hb) / den) + nb * ((hb - g * ha) / den);
            // the fillet's faces along that line, near it
            let reach = round.size * 2.0 + 1.0;
            let mut span: Option<(f64, f64)> = None;
            for (tri, fi) in m.triangles.iter().zip(&m.tri_face) {
                if m.faces[*fi as usize].tag.map(|t| t.pack()) != Some(own) {
                    continue;
                }
                for &v in tri {
                    let q = pos(v) - o;
                    let s = q.dot(t);
                    if (q - t * s).length() <= reach {
                        span = Some(span.map(|(a, b)| (a.min(s), b.max(s))).unwrap_or((s, s)));
                    }
                }
            }
            let Some((a, b)) = span else { continue };
            let len = b - a;
            let k = ((len / 0.5).round() as usize).clamp(3, 10);
            let v: Vec<EdgeSample> = (0..k).filter_map(|i| self.edge_sample(fa, fb, o + t * (a + len * (0.12 + 0.76 * i as f64 / (k - 1) as f64)), t)).collect();
            if !v.is_empty() {
                out.push(v);
            }
        }
        out
    }

    /// The scan's primary shapes (the "scan → shapes" button's first stage,
    /// Romain 19 nov., 18 h 09: the big volumes first): its big blocks read
    /// as slabs seen from its side (`rectify_core::primary`), made as Cube
    /// and Sketch steps in one undo step, the biggest first; the deviation
    /// is turned on to show what later stages have to read.
    pub fn read_primaries(&mut self) {
        let Some(r) = self.reference.clone() else {
            self.toast("Import a reference mesh first (drop an .obj on the window)", true);
            return;
        };
        let t0 = std::time::Instant::now();
        let pr = rectify_core::primary::read(&r.mesh, r.edge);
        let reads: Vec<(PrimaryRead, f64)> = pr.parts.iter().filter_map(|s| primary_step(&pr, s).map(|x| (x, s.flat))).collect();
        if std::env::var_os("FORME_SCAN_DEBUG").is_some() {
            eprintln!("primary: side {:?}, step {:.3} mm, {} parts, {:.0} ms", pr.w, pr.px, pr.parts.len(), t0.elapsed().as_secs_f64() * 1e3);
            for s in &pr.parts {
                eprintln!("  {:>2} corners, {:.2} .. {:.2}, area {:.0} mm², flat {:.2}, rect {}", s.outline.len(), s.back, s.front, s.area, s.flat, s.rect);
            }
        }
        if reads.is_empty() {
            self.toast("No primary shape read on the scan", true);
            return;
        }
        let mat = self.material;
        let mut ids = vec![];
        self.edit("Primary shapes from the scan", |d| {
            for (x, _) in &reads {
                let id = d.add(Op::Add, x.prim, x.origin, x.rot);
                let name = d.fresh_name("Block");
                if let Some(f) = d.feature_mut(id) {
                    f.name = name;
                    f.profile = x.profile.clone();
                    f.material = mat;
                }
                ids.push(id);
            }
        });
        self.selection = vec![];
        let cubes = reads.iter().filter(|(x, _)| matches!(x.prim, Prim::Box { .. })).count();
        let axis = if pr.w.x.abs() > 0.5 { "X" } else if pr.w.y.abs() > 0.5 { "Y" } else { "Z" };
        let cut = |f: fn(f64) -> bool| reads.iter().filter_map(|(x, _)| x.profile.as_ref()).map(|p| p.corner.iter().filter(|c| f(**c)).count()).sum::<usize>();
        let (rounded, chamfered) = (cut(|c| c > 0.0), cut(|c| c < 0.0));
        self.toast(
            &format!(
                "{} primary shapes read on the scan, seen along {axis}: {cubes} cubes, {} outlines ({rounded} corners rounded, {chamfered} chamfered) · the deviation shows what is left",
                reads.len(),
                reads.len() - cubes
            ),
            false,
        );
        self.set_deviation(true);
    }

    /// Fit to scan: the picked sharp edges (or the selected fillet's) read
    /// on the scan; the fillet made, or sized, at the radius read.
    pub fn fit_round_to_scan(&mut self) {
        let Some(r) = self.reference.clone() else {
            self.toast("Fit to scan: import a reference mesh first", true);
            return;
        };
        let existing = self.selected_round().filter(|f| f.round.as_ref().map(|r| !r.chamfer).unwrap_or(false)).map(|f| f.id);
        let edges: Vec<Vec<EdgeSample>> = match existing {
            Some(id) => self.fillet_samples(id),
            None => self.edge_sel.clone().iter().filter_map(|e| self.model_edge_of(e)).map(|i| self.edge_samples(i)).filter(|v| !v.is_empty()).collect(),
        };
        if edges.is_empty() {
            self.toast(if existing.is_some() { "Fit to scan: this fillet's faces must be flat" } else { "Fit to scan: pick sharp edges of the part laid on the scan" }, true);
            return;
        }
        let t0 = std::time::Instant::now();
        let reads: Vec<Result<FilletRead, String>> = edges.iter().map(|s| rectify_core::fillet::read(&r.mesh, &r.topo, &r.bvh, s, r.edge)).collect();
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let ok: Vec<&FilletRead> = reads.iter().filter_map(|x| x.as_ref().ok()).collect();
        if ok.is_empty() {
            let e = reads.iter().find_map(|x| x.as_ref().err()).cloned().unwrap_or_default();
            self.toast(&format!("Fit to scan: {e}"), true);
            return;
        }
        let mut rs: Vec<f64> = ok.iter().flat_map(|f| f.slices.iter().map(|s| s.r)).collect();
        rs.sort_by(f64::total_cmp);
        let size = (rs[rs.len() / 2] * 100.0).round() / 100.0;
        if std::env::var_os("FORME_SCAN_DEBUG").is_some() {
            for (i, x) in reads.iter().enumerate() {
                match x {
                    Ok(f) => eprintln!("fit to scan: edge {i}: r {:.3} ({:.3} to {:.3}), {} of {} slices, convex {}", f.r, f.lo, f.hi, f.slices.len(), f.tried, f.convex),
                    Err(e) => eprintln!("fit to scan: edge {i}: {e}"),
                }
            }
            eprintln!("fit to scan: {size} mm in {ms:.1} ms");
        }
        self.set_round(size, false, true);
        self.end_live();
        let Some(id) = self.selected_round().map(|f| f.id) else { return };
        self.fillet_show = Some(FilletShow { id, arcs: ok.iter().flat_map(|f| f.slices.iter().map(|s| s.arc(16))).collect() });
        let fmt = |v: f64| format!("{v:.2}").replace('.', ",");
        let (lo, hi) = (rs[0], rs[rs.len() - 1]);
        let tried: usize = edges.iter().map(|v| v.len()).sum();
        let mut text = format!("Fillet read on the scan: {} mm ({} of {tried} slices, {} to {})", fmt(size), rs.len(), fmt(lo), fmt(hi));
        if ok.len() < reads.len() {
            text += &format!(" · {} of {} edges read", ok.len(), reads.len());
        }
        if hi - lo > (0.25 * size).max(0.1) {
            text += " · it varies along the edges";
        }
        self.toast(&text, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prints the scan's planes found from a few directions over a grid
    /// (`FORME_REF` = an .obj; ignored: it needs the pistol).
    #[test]
    #[ignore]
    fn scan_planes_probe() {
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        let r = crate::reference::Reference::load(&path).expect("reference");
        println!("edge {:.3}, box {:.1?} {:.1?}", r.edge, r.lo, r.hi);
        for (name, d) in [("+Z", DVec3::new(0.0, 0.0, -1.0)), ("top", DVec3::new(0.0, -1.0, 0.0)), ("oblique", DVec3::new(-0.3, -0.5, -0.8).normalize())] {
            let (u, v) = if d.y.abs() > 0.9 { (DVec3::X, DVec3::Z) } else { (DVec3::X, DVec3::Y) };
            for i in 0..12 {
                for j in 0..8 {
                    let c = (r.lo + r.hi) * 0.5;
                    let size = r.hi - r.lo;
                    let off = u * ((i as f64 + 0.5) / 12.0 - 0.5) * size.dot(u) + v * ((j as f64 + 0.5) / 8.0 - 0.5) * size.dot(v);
                    let o = c + off - d * 200.0;
                    let Some((t, x, _)) = r.hit_tri(o, d) else { continue };
                    let t0 = std::time::Instant::now();
                    let (pl, flat) = plane_under(&r.mesh, &r.topo, t, x, 12.0, r.edge);
                    let ms = t0.elapsed().as_secs_f64() * 1e3;
                    if flat {
                        println!("{name} at {x:.1?}: n {:.3?} rms {:.3} area {:.0} ({ms:.1} ms)", pl.n, pl.rms, pl.area);
                    }
                }
            }
        }
    }

    /// Writes the scan's height seen from +Z and from -Z as PGM images
    /// (`FORME_HMAP` = a folder), 4 pixels per mm (ignored: the pistol).
    #[test]
    #[ignore]
    fn scan_height_map() {
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        let out = std::env::var("FORME_HMAP").unwrap_or_else(|_| "/tmp".into());
        let r = crate::reference::Reference::load(&path).expect("reference");
        for (name, sign) in [("pz", 1.0), ("nz", -1.0)] {
            let (w, h) = (((r.hi.x - r.lo.x) * 4.0) as usize, ((r.hi.y - r.lo.y) * 4.0) as usize);
            let mut img = vec![0u8; w * h];
            for j in 0..h {
                for i in 0..w {
                    let x = r.lo.x + (i as f64 + 0.5) / 4.0;
                    let y = r.hi.y - (j as f64 + 0.5) / 4.0;
                    if let Some((q, _)) = r.hit(DVec3::new(x, y, sign * 100.0), DVec3::new(0.0, 0.0, -sign)) {
                        let z = (q.z * sign - (r.lo.z.min(-r.hi.z)).abs() * 0.0).clamp(-20.0, 20.0);
                        img[j * w + i] = (((z + 20.0) / 40.0) * 255.0) as u8;
                    }
                }
            }
            let mut f = format!("P5\n{w} {h}\n255\n").into_bytes();
            f.extend(img);
            std::fs::write(format!("{out}/hmap-{name}.pgm"), f).unwrap();
            println!("{name}: {w} × {h}, x from {:.1}, y from {:.1} (top) at 4 px / mm", r.lo.x, r.hi.y);
        }
    }

    /// Writes the scan's heights seen from +Z over a box (ignored: the
    /// pistol). `FORME_CROP="x0,y0,x1,y1,px_per_mm,out.f32"`: row-major f32,
    /// top row first (y1), NaN where nothing is hit.
    #[test]
    #[ignore]
    fn scan_crop() {
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        let r = crate::reference::Reference::load(&path).expect("reference");
        let v: Vec<String> = std::env::var("FORME_CROP").unwrap().split(',').map(|x| x.to_string()).collect();
        let f = |i: usize| v[i].parse::<f64>().unwrap();
        let (x0, y0, x1, y1, k) = (f(0), f(1), f(2), f(3), f(4));
        let (w, h) = (((x1 - x0) * k) as usize, ((y1 - y0) * k) as usize);
        let mut out = Vec::with_capacity(w * h * 4);
        for j in 0..h {
            for i in 0..w {
                let x = x0 + (i as f64 + 0.5) / k;
                let y = y1 - (j as f64 + 0.5) / k;
                let z = r.hit(DVec3::new(x, y, 100.0), DVec3::new(0.0, 0.0, -1.0)).map(|(q, _)| q.z as f32).unwrap_or(f32::NAN);
                out.extend_from_slice(&z.to_le_bytes());
            }
        }
        std::fs::write(&v[5], out).unwrap();
        println!("{w} × {h}");
    }

    /// Reads the square button and the small pocket of the pistol's +Z side
    /// with Cube drags of a few sizes (ignored: the pistol).
    #[test]
    #[ignore]
    fn scan_box_probe() {
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        let r = crate::reference::Reference::load(&path).expect("reference");
        let side = |x: f64, y: f64| r.hit_tri(DVec3::new(x, y, 100.0), DVec3::new(0.0, 0.0, -1.0)).unwrap();
        // (name, its corners, then margins outside them)
        for (name, lo, hi) in [("button", (-15.75, 75.75), (-10.25, 80.75)), ("pocket", (30.25, 74.25), (33.75, 78.25))] {
            for m in [-0.5, 0.0, 0.4, 0.8, 1.2, 1.8] {
                let (t0, p0, _) = side(lo.0 - m, lo.1 - m);
                let (_, p1, _) = side(hi.0 + m, hi.1 + m);
                match Slot::detect_box(&r.mesh, &r.topo, t0, p0, p1, r.edge) {
                    Ok(s) => {
                        let st = outline_step(&s);
                        println!("{name} margin {m}: {:?} centre {:.2?} depth {:.2} inward {}", st.map(|o| format!("{:.2} × {:.2} r {:.2}", o.x, o.z, o.r)), (s.a + s.b) * 0.5, s.depth, s.inward)
                    }
                    Err(e) => println!("{name} margin {m}: {e}"),
                }
            }
        }
    }

    #[test]
    fn axis_snaps_near_world_axes() {
        // the pistol's muzzle hole read 1.4° off -X: taken as -X
        let a = DVec3::new(-1.0, 0.005, 0.024).normalize();
        assert_eq!(snap_axis(a), -DVec3::X);
        // 4° off: taken as Z; 7° off: kept as read
        let a4 = DVec3::new(0.0, 4f64.to_radians().sin(), 4f64.to_radians().cos());
        assert_eq!(snap_axis(a4), DVec3::Z);
        let b = DVec3::new(0.0, 7f64.to_radians().sin(), 7f64.to_radians().cos());
        assert_eq!(snap_axis(b), b);
    }

    /// Prints the scan's vertices in a thin slab (ignored: the pistol).
    /// `FORME_SLAB="axis,at,lo1,hi1,lo2,hi2"`: axis 0 x, 1 y, 2 z; the
    /// other two coordinates' boxes in order.
    #[test]
    #[ignore]
    fn slab_dump() {
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        let r = crate::reference::Reference::load(&path).expect("reference");
        let v: Vec<f64> = std::env::var("FORME_SLAB").unwrap().split(',').map(|x| x.parse().unwrap()).collect();
        let ax = v[0] as usize;
        let (o1, o2) = ((ax + 1) % 3, (ax + 2) % 3);
        let mut pts: Vec<(f64, f64)> = r.mesh.pos.iter().filter(|p| (p[ax] - v[1]).abs() < 0.08 && p[o1] >= v[2] && p[o1] <= v[3] && p[o2] >= v[4] && p[o2] <= v[5]).map(|p| (p[o1], p[o2])).collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (a, b) in pts {
            println!("{a:.3} {b:.3}");
        }
    }

    /// Reads a few edges of the pistol with samples laid by hand (ignored:
    /// the pistol; `FORME_SCAN_DEBUG=1` prints each slice).
    #[test]
    #[ignore]
    fn fillet_probe() {
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        let r = crate::reference::Reference::load(&path).expect("reference");
        let (x, y, z) = (DVec3::X, DVec3::Y, DVec3::Z);
        // (name, edge start, end, normals, ways away, reaches)
        let cases: Vec<(&str, DVec3, DVec3, [DVec3; 2], [DVec3; 2], [f64; 2])> = vec![
            ("receiver top / side, x 3 to 13", DVec3::new(3.0, 99.45, 10.3), DVec3::new(13.0, 99.45, 10.3), [z, y], [-y, -z], [9.5, 16.0]),
            ("receiver top / side, x -10 to -5", DVec3::new(-10.0, 102.0, 13.3), DVec3::new(-5.0, 102.0, 13.3), [z, y], [-y, -z], [20.0, 16.0]),
            ("receiver top / side, x -9.5 to -4.5 (box)", DVec3::new(-9.5, 101.8, 13.6), DVec3::new(-4.5, 101.8, 13.6), [z, y], [-y, -z], [26.0, 20.0]),
            ("button top / +X side", DVec3::new(-10.25, 76.5, 16.2), DVec3::new(-10.25, 80.0, 16.2), [z, x], [-x, -z], [5.0, 2.4]),
            ("button top / +Y side", DVec3::new(-15.0, 80.75, 16.2), DVec3::new(-11.0, 80.75, 16.2), [z, y], [-y, -z], [4.5, 2.4]),
            ("step top (y 84)", DVec3::new(2.0, 84.2, 13.5), DVec3::new(14.0, 84.2, 13.5), [z, y], [-y, -z], [9.0, 1.8]),
            ("muzzle side / top", DVec3::new(38.0, 100.0, 13.0), DVec3::new(52.0, 100.0, 13.0), [z, y], [-y, -z], [25.0, 12.0]),
        ];
        for (name, a, b, n, d, reach) in cases {
            let t = (b - a).normalize();
            let s: Vec<EdgeSample> = (0..10).map(|i| EdgeSample { p: a.lerp(b, i as f64 / 9.0), t, n, d, reach }).collect();
            match rectify_core::fillet::read(&r.mesh, &r.topo, &r.bvh, &s, r.edge) {
                Ok(f) => println!("{name}: r {:.3} ({:.3} to {:.3}), {} of {} slices, convex {}, off {:.2?} turn {:.1?} rms {:.3}", f.r, f.lo, f.hi, f.slices.len(), f.tried, f.convex, f.slices[0].off, f.slices[0].turn, f.slices[0].rms),
                Err(e) => println!("{name}: {e}"),
            }
        }
    }

}

/// The scan's flat face found last under the pointer: a plane through `o`
/// facing `n` (towards the eye), the reference it was found on.
#[derive(Clone, Copy, Debug)]
pub struct ScanPlaneMemo {
    pub o: DVec3,
    pub n: DVec3,
    pub tol: f64,
    pub gen: u64,
}

/// While the pointer only hovers, the scan's flat faces are searched at
/// most this often (s): a search takes 15 to 55 ms on the pistol; in
/// between, its tangent plane.
const HOVER_SEARCH: f64 = 0.08;

impl AppState {
    /// The scan's plane under a point of the screen (retopology, step 4):
    /// its flat face there (`plane_under`, within 90 points of the
    /// pointer, as Rectify's Pen), else its tangent plane, facing the eye;
    /// a normal within 5° of a world axis is taken as that axis. A flat face
    /// found stays while the pointer is on its plane (it does not tilt at
    /// its border, where the scan rounds off). `hover`: the pointer only
    /// passes (the search for a flat face is not made at every frame).
    /// None when the reference is hidden, not under the pointer, or the
    /// part is in front of it (then the part's face or the ground, as
    /// before).
    pub fn scan_plane_at(&self, p: [f32; 2], hover: bool) -> Option<(BasePlane, DVec3)> {
        let r = self.reference.as_ref().filter(|_| self.ref_shown)?;
        let (ro, rd) = self.ray(p);
        let (t, x, s) = r.hit_tri(ro, rd)?;
        let px = self.mm_per_point(x);
        if let Some(h) = self.pick_at(p) {
            if (h.t as f64) <= s + px * 4.0 {
                return None;
            }
        }
        let reach = (px * 90.0).clamp(1.5, 25.0);
        let memo = self.scan_plane_memo.get().filter(|m| m.gen == self.ref_gen && m.n.dot(rd) < 0.0 && m.n.dot(x - m.o).abs() <= m.tol + px);
        let recent = self.scan_plane_search.get().map(|t| t.elapsed().as_secs_f64() < HOVER_SEARCH).unwrap_or(false);
        let (o, n, flat) = match memo {
            Some(m) => (m.o, m.n, true),
            None if hover && recent => (x, tangent_under(&r.mesh, &r.topo, t, x, reach, r.edge), false),
            None => {
                let t0 = std::time::Instant::now();
                let (pl, flat) = plane_under(&r.mesh, &r.topo, t, x, reach, r.edge);
                self.scan_plane_search.set(Some(std::time::Instant::now()));
                let o = pl.onto(x);
                let n = pl.n;
                self.scan_plane_memo.set(None);
                if flat {
                    let mut f = n.normalize_or_zero();
                    if f.dot(rd) > 0.0 {
                        f = -f;
                    }
                    self.scan_plane_memo.set(Some(ScanPlaneMemo { o, n: snap_axis(f), tol: (r.edge * 1.5).max(0.08) + pl.rms * 3.0, gen: self.ref_gen }));
                }
                if std::env::var_os("FORME_SCAN_DEBUG").is_some() {
                    eprintln!("scan plane at {x:.2?}: flat {flat}, n {n:.3?}, rms {:.3}, area {:.1}, reach {reach:.1}, {:.1} ms", pl.rms, pl.area, t0.elapsed().as_secs_f64() * 1e3);
                }
                (o, n, flat)
            }
        };
        let mut n = n.normalize_or_zero();
        if n == DVec3::ZERO {
            return None;
        }
        if n.dot(rd) > 0.0 {
            n = -n;
        }
        // its origin where it passes nearest the world origin: the grid
        // lines up with the world's (on a face square to an axis, the
        // world grid itself)
        let n = snap_axis(n);
        let mut plane = BasePlane::new(n * n.dot(o), n, None);
        plane.scan = Some(flat);
        plane.hit(ro, rd).map(|q| (plane, q))
    }
}
