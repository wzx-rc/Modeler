//! Stage 2, the clean-up: the result is compared with the mesh it came from
//! and nowhere may it be rougher ("never rougher than the AI mesh"). Where
//! the snap left a crease, a fold or a wrinkle the AI mesh did not have,
//! the move there is smoothed with its neighbours' moves; what is still
//! rougher after that goes back towards the AI mesh, softly.

use crate::mesh::{Mesh, Topo};
use crate::NONE;
use glam::DVec3;
use rayon::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TidyParams {
    pub on: bool,
    /// How much sharper than the AI mesh a vertex may turn (degrees), away
    /// from the edges the snap built.
    pub margin: f64,
    /// Same on the edges the snap built (they are sharper by design).
    pub edge_margin: f64,
    /// How much more a vertex's normal may stray from its neighbours' than
    /// on the AI mesh (degrees): the small wrinkles the bend lets through.
    pub noise: f64,
    /// Same on the edges the snap built.
    pub edge_noise: f64,
    /// Smoothing passes where it is rough.
    pub iters: usize,
    /// What is smoothed where it is rough: the result itself (true: its
    /// wrinkles go, its lines mostly stay) or the move (false).
    pub calm: bool,
    /// Rings around a rough vertex that are smoothed too.
    pub rings: usize,
    /// What is still rough after smoothing goes back to the AI mesh.
    pub revert: bool,
    /// The share of the move taken back each round where it is still rough
    /// (1: all at once).
    pub revert_step: f64,
    /// Over how far (mm, along the mesh) a part taken back fades into the
    /// rest: a short fade makes a crease of its own. Scaled by the move
    /// there, from 0.3 to 1.6 times this.
    pub fade: f64,
}

impl Default for TidyParams {
    fn default() -> Self {
        TidyParams { on: true, margin: 14.0, edge_margin: 40.0, noise: 1.5, edge_noise: 4.0, iters: 30, calm: true, rings: 2, revert: true, revert_step: 0.34, fade: 1.5 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TidyReport {
    /// Vertices rougher than the AI mesh before the clean-up, and after.
    pub rough_before: usize,
    pub rough_after: usize,
    /// Still rough after the smoothing, before taking back.
    pub rough_mid: usize,
    /// Vertices whose move was smoothed, and taken back (fully or partly).
    pub smoothed: usize,
    pub reverted: usize,
    /// Vertices kept as the AI mesh by a "keep" zone.
    pub kept: usize,
}

/// What a mesh changed against the AI mesh, per vertex (the diff map).
#[derive(Clone, Debug, Default)]
pub struct Diff {
    /// How far the vertex moved (mm).
    pub moved: Vec<f32>,
    /// How much sharper than the AI mesh it turned, beyond what is allowed
    /// (degrees; 0 = fine).
    pub worse: Vec<f32>,
}

impl Diff {
    pub fn rough_count(&self) -> usize {
        self.worse.iter().filter(|w| **w > 0.0).count()
    }
}

/// Per vertex, the largest bend between two faces that meet at it
/// (degrees).
pub fn bend(m: &Mesh, topo: &Topo) -> Vec<f32> {
    let n = m.normals();
    let per_tri: Vec<[f32; 3]> = (0..m.tris.len())
        .into_par_iter()
        .map(|t| {
            let mut out = [0.0f32; 3];
            for k in 0..3 {
                let o = topo.adj[t][k];
                if o == NONE {
                    continue;
                }
                let c = n[t].dot(n[o as usize]).clamp(-1.0, 1.0);
                out[k] = c.acos().to_degrees() as f32;
            }
            out
        })
        .collect();
    let mut b = vec![0.0f32; m.pos.len()];
    for (t, a) in per_tri.iter().enumerate() {
        let tri = m.tris[t];
        for k in 0..3 {
            let (u, v) = (tri[k] as usize, tri[(k + 1) % 3] as usize);
            b[u] = b[u].max(a[k]);
            b[v] = b[v].max(a[k]);
        }
    }
    b
}

/// Per vertex, how far its normal strays from the mean of its neighbours'
/// (degrees): near 0 on a plane or a steady curve, larger on wrinkles.
pub fn stray(m: &Mesh, topo: &Topo) -> Vec<f32> {
    let vn = m.vertex_normals(topo);
    (0..m.pos.len())
        .into_par_iter()
        .map(|v| {
            let a = topo.verts_of(v).iter().map(|&u| vn[u as usize]).sum::<DVec3>().normalize_or_zero();
            if a == DVec3::ZERO {
                return 0.0;
            }
            vn[v].dot(a).clamp(-1.0, 1.0).acos().to_degrees() as f32
        })
        .collect()
}

/// How the AI mesh looks, per vertex: what the result is measured against.
#[derive(Clone, Debug, Default)]
pub struct Look {
    pub bend: Vec<f32>,
    pub stray: Vec<f32>,
}

impl Look {
    pub fn of(m: &Mesh, topo: &Topo) -> Look {
        Look { bend: bend(m, topo), stray: stray(m, topo) }
    }
}

/// Faces turned over by more than 75° against the AI mesh: their corners.
fn folds(base: &Mesh, m: &Mesh) -> Vec<bool> {
    let nb = base.normals();
    let nm = m.normals();
    let mut out = vec![false; m.pos.len()];
    for t in 0..m.tris.len() {
        if nb[t].dot(nm[t]) < 0.26 && m.tri_area(t) > 1e-12 {
            for &v in &m.tris[t] {
                out[v as usize] = true;
            }
        }
    }
    out
}

/// The diff map of `m` against the AI mesh `base`; `kinds` from the snap
/// (vertices on an edge it built may be sharper).
pub fn diff(base: &Mesh, look: &Look, m: &Mesh, topo: &Topo, kinds: &[u8], p: &TidyParams) -> Diff {
    let b = bend(m, topo);
    let st = stray(m, topo);
    let fold = folds(base, m);
    let worse = (0..m.pos.len())
        .map(|v| {
            let on_edge = matches!(kinds.get(v), Some(2) | Some(4));
            let (margin, noise) = if on_edge { (p.edge_margin, p.edge_noise) } else { (p.margin, p.noise) };
            let w = (b[v] - look.bend[v] - margin as f32).max(st[v] - look.stray[v] - noise as f32);
            if fold[v] {
                w.max(90.0)
            } else {
                w.max(0.0)
            }
        })
        .collect();
    let moved = (0..m.pos.len()).map(|v| (m.pos[v] - base.pos[v]).length() as f32).collect();
    Diff { moved, worse }
}

/// How much of its move each vertex gives back: `level[v]` on a vertex
/// taken back, fading to nothing at its radius `radius[v]` along the mesh
/// (the largest share where several overlap).
fn back_from(m: &Mesh, topo: &Topo, level: &[f64], radius: &[f64]) -> Vec<f64> {
    use std::cmp::Ordering;
    use std::collections::BinaryHeap;
    // value, vertex, the source's level, its radius, how far in (0-1)
    #[derive(PartialEq)]
    struct It(f64, u32, f64, f64, f64);
    impl Eq for It {}
    impl Ord for It {
        fn cmp(&self, o: &Self) -> Ordering {
            self.0.partial_cmp(&o.0).unwrap_or(Ordering::Equal)
        }
    }
    impl PartialOrd for It {
        fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
            Some(self.cmp(o))
        }
    }
    let mut back = vec![0.0f64; m.pos.len()];
    let mut heap = BinaryHeap::new();
    for (v, &l) in level.iter().enumerate() {
        if l > 0.0 {
            back[v] = l;
            heap.push(It(l, v as u32, l, radius[v].max(1e-6), 0.0));
        }
    }
    while let Some(It(val, v, l, r, t)) = heap.pop() {
        if val < back[v as usize] {
            continue;
        }
        for &u in topo.verts_of(v as usize) {
            let nt = t + (m.pos[u as usize] - m.pos[v as usize]).length() / r;
            if nt >= 1.0 {
                continue;
            }
            let nv = l * (1.0 - smooth01(nt));
            if nv > back[u as usize] + 1e-9 {
                back[u as usize] = nv;
                heap.push(It(nv, u, l, r, nt));
            }
        }
    }
    back
}

fn smooth01(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The vertices within `rings` of a marked one, with their ring (0 = marked).
fn rings_of(topo: &Topo, marked: &[bool], rings: usize) -> Vec<u8> {
    let mut ring = vec![u8::MAX; marked.len()];
    let mut front: Vec<usize> = (0..marked.len()).filter(|&v| marked[v]).collect();
    for &v in &front {
        ring[v] = 0;
    }
    for r in 1..=rings {
        let mut next = vec![];
        for &v in &front {
            for &u in topo.verts_of(v) {
                let u = u as usize;
                if ring[u] == u8::MAX {
                    ring[u] = r as u8;
                    next.push(u);
                }
            }
        }
        front = next;
    }
    ring
}

/// Cleans `m` (the result) against `base` (the AI mesh it came from).
/// `keep`: zones (centre, radius) left as the AI mesh.
pub fn tidy(base: &Mesh, m: &mut Mesh, topo: &Topo, kinds: &[u8], keep: &[(DVec3, f64)], p: &TidyParams) -> (TidyReport, Diff, Diff) {
    let nv = m.pos.len();
    let look = Look::of(base, topo);
    let before = diff(base, &look, m, topo, kinds, p);
    let mut rep = TidyReport { rough_before: before.rough_count(), ..Default::default() };
    let mut d: Vec<DVec3> = (0..nv).map(|v| m.pos[v] - base.pos[v]).collect();

    // keep zones: the move fades to nothing inside them
    if !keep.is_empty() {
        for v in 0..nv {
            let mut f = 1.0f64;
            for &(c, r) in keep {
                let x = (base.pos[v] - c).length() / r.max(1e-6);
                if x < 1.0 {
                    rep.kept += 1;
                }
                // 0 inside 80 % of the radius, back to 1 at the radius
                f = f.min(((x - 0.8) / 0.2).clamp(0.0, 1.0));
            }
            d[v] *= f;
        }
    }

    if p.on {
        // 1. smoothed where the result is rough
        let rough: Vec<bool> = before.worse.iter().map(|w| *w > 0.0).collect();
        let ring = rings_of(topo, &rough, p.rings);
        let free: Vec<usize> = (0..nv).filter(|&v| ring[v] != u8::MAX).collect();
        rep.smoothed = free.len();
        // how much of its own move a vertex keeps each pass (rough ones
        // none, the rings around them a little)
        let keep_own = |v: usize| -> f64 {
            match ring[v] {
                0 => 0.0,
                u8::MAX => 1.0,
                r => r as f64 / (p.rings as f64 + 1.0),
            }
        };
        if p.calm {
            // Taubin's two steps (in, then a little more out): the result
            // loses its wrinkles without shrinking
            let weight = |v: usize| 1.0 - keep_own(v);
            for it in 0..p.iters {
                let lam = if it % 2 == 0 { 0.6 } else { -0.63 };
                let cur = &m.pos;
                let upd: Vec<(usize, DVec3)> = free
                    .par_iter()
                    .map(|&v| {
                        let ns = topo.verts_of(v);
                        if ns.is_empty() {
                            return (v, cur[v]);
                        }
                        let avg = ns.iter().map(|&u| cur[u as usize]).sum::<DVec3>() / ns.len() as f64;
                        (v, cur[v] + (avg - cur[v]) * (lam * weight(v)))
                    })
                    .collect();
                for (v, x) in upd {
                    m.pos[v] = x;
                }
            }
            for v in 0..nv {
                d[v] = m.pos[v] - base.pos[v];
            }
        } else {
            for _ in 0..p.iters {
                let cur = &d;
                let upd: Vec<(usize, DVec3)> = free
                    .par_iter()
                    .map(|&v| {
                        let ns = topo.verts_of(v);
                        if ns.is_empty() {
                            return (v, cur[v]);
                        }
                        let avg = ns.iter().map(|&u| cur[u as usize]).sum::<DVec3>() / ns.len() as f64;
                        let k = keep_own(v);
                        (v, cur[v] * k + avg * (1.0 - k))
                    })
                    .collect();
                for (v, x) in upd {
                    d[v] = x;
                }
            }
            for v in 0..nv {
                m.pos[v] = base.pos[v] + d[v];
            }
        }

        // 2. still rough: back to the AI mesh, fully there and fading over
        // `fade` mm around; a few rounds, as taking back can leave a crease
        // at the edge of what was taken back
        if p.revert {
            let smoothed = d.clone();
            let mut back = vec![0.0f64; nv];
            let mut level = vec![0.0f64; nv];
            // a larger move needs a longer fade not to crease (the fade
            // bends the surface by about 6 move / radius² per mm)
            let radius: Vec<f64> = (0..nv)
                .map(|v| {
                    let mv = topo.verts_of(v).iter().map(|&u| smoothed[u as usize].length()).fold(smoothed[v].length(), f64::max);
                    (20.0 * mv).sqrt().clamp(p.fade * 0.3, p.fade * 1.6)
                })
                .collect();
            // a share at a time: often part of the move is enough to take back
            let step = p.revert_step.clamp(0.05, 1.0);
            let rounds = (1.0 / step).ceil() as usize + 3;
            for _ in 0..rounds {
                let mid = diff(base, &look, m, topo, kinds, p);
                if rep.rough_mid == 0 {
                    rep.rough_mid = mid.rough_count();
                }
                let mut new = false;
                for v in 0..nv {
                    if mid.worse[v] > 0.0 && level[v] < 1.0 {
                        level[v] = (back[v].max(level[v]) + step).min(1.0);
                        new = true;
                    }
                }
                if !new {
                    break;
                }
                back = back_from(base, topo, &level, &radius);
                for v in 0..nv {
                    d[v] = smoothed[v] * (1.0 - smooth01(back[v]));
                    m.pos[v] = base.pos[v] + d[v];
                }
            }
            rep.reverted = back.iter().filter(|b| **b > 0.0).count();
        }
    } else {
        for v in 0..nv {
            m.pos[v] = base.pos[v] + d[v];
        }
    }
    let after = diff(base, &look, m, topo, kinds, p);
    rep.rough_after = after.rough_count();
    (rep, before, after)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat grid, one vertex pushed up in the result: found rough, and
    /// the clean-up flattens it again.
    #[test]
    fn spike_is_cleaned() {
        let n = 12;
        let mut pos = vec![];
        for j in 0..n {
            for i in 0..n {
                pos.push(DVec3::new(i as f64, 0.0, j as f64));
            }
        }
        let mut tris = vec![];
        for j in 0..n - 1 {
            for i in 0..n - 1 {
                let a = (j * n + i) as u32;
                tris.push([a, a + n as u32, a + 1]);
                tris.push([a + 1, a + n as u32, a + n as u32 + 1]);
            }
        }
        let base = Mesh { pos, tris, uv: vec![], tri_uv: vec![], unit_mm: 1.0 };
        let topo = Topo::new(&base);
        let mut m = base.clone();
        let c = 6 * n + 6;
        m.pos[c].y = 0.6;
        let kinds = vec![0u8; base.pos.len()];
        let (rep, before, after) = tidy(&base, &mut m, &topo, &kinds, &[], &TidyParams::default());
        assert!(before.worse[c] > 0.0);
        assert!(rep.rough_after == 0, "{rep:?}");
        assert!(after.worse.iter().all(|w| *w == 0.0));
        assert!(m.pos[c].y < 0.2, "{}", m.pos[c].y);
    }
}
