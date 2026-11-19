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
//! Step 4, the plane under the pointer: Cube, Sketch and Path start on the
//! scan's flat face under the pointer (`rectify_core::planes::plane_under`, as Forme Rectify's Pen),
//! else its tangent plane there, when the part is not in front of it.

use crate::render::MeshVertex;
use crate::state::{AppState, BasePlane};
use glam::{DQuat, DVec3};
use nurbs_kernel::{Model, Op, Prim};
use rectify_core::planes::{dominant_normal, plane_under, tangent_under};
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
        let Some(read) = found.as_ref().and_then(round_step) else {
            let why = if found.is_some() { "its depth could not be read" } else { "no wall goes round the circle drawn" };
            self.toast(&format!("Nothing round read there ({why}) · drag across it, rim to rim"), true);
            return;
        };
        let kind = if read.op == Op::Carve { "Hole" } else { "Boss" };
        let rot = DQuat::from_rotation_arc(DVec3::Y, read.axis);
        let mat = self.material;
        let mut id = 0;
        self.edit(&format!("{kind} from the scan"), |d| {
            id = d.add(read.op, Prim::Cylinder { radius: read.radius, height: read.height }, read.origin, rot);
            let name = d.fresh_name(kind);
            if let Some(f) = d.feature_mut(id) {
                f.name = name;
                f.material = mat;
            }
        });
        self.selection = vec![id];
        let fmt = |v: f64| crate::widgets::fmt_mm(v as f32);
        let way = if read.op == Op::Carve { "deep" } else { "high" };
        self.toast(&format!("{kind} read on the scan: Ø {} mm, {} mm {way}", fmt(read.radius * 2.0), fmt(read.depth)), false);
    }

    pub fn scan_round_cancel(&mut self) {
        self.scan_round = None;
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
