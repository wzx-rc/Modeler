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
    /// Smoothing passes of the move where it is rough.
    pub iters: usize,
    /// Rings around a rough vertex that are smoothed too.
    pub rings: usize,
    /// What is still rough after smoothing goes back to the AI mesh.
    pub revert: bool,
}

impl Default for TidyParams {
    fn default() -> Self {
        TidyParams { on: true, margin: 14.0, edge_margin: 40.0, iters: 30, rings: 2, revert: true }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TidyReport {
    /// Vertices rougher than the AI mesh before the clean-up, and after.
    pub rough_before: usize,
    pub rough_after: usize,
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
pub fn diff(base: &Mesh, base_bend: &[f32], m: &Mesh, topo: &Topo, kinds: &[u8], p: &TidyParams) -> Diff {
    let b = bend(m, topo);
    let fold = folds(base, m);
    let worse = (0..m.pos.len())
        .map(|v| {
            let on_edge = matches!(kinds.get(v), Some(2) | Some(4));
            let margin = if on_edge { p.edge_margin } else { p.margin } as f32;
            let w = b[v] - base_bend[v] - margin;
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
    let base_bend = bend(base, topo);
    let before = diff(base, &base_bend, m, topo, kinds, p);
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
        // 1. the move smoothed where the result is rough
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

        // 2. still rough: back to the AI mesh, fully there and fading over
        // a few rings around; a few rounds, as taking back can leave a
        // crease at the edge of what was taken back
        if p.revert {
            let smoothed = d.clone();
            let mut back = vec![0.0f64; nv];
            for _ in 0..4 {
                let mid = diff(base, &base_bend, m, topo, kinds, p);
                let rough: Vec<bool> = mid.worse.iter().map(|w| *w > 0.0).collect();
                if !rough.iter().any(|r| *r) {
                    break;
                }
                let k = 4usize;
                let ring = rings_of(topo, &rough, k);
                for v in 0..nv {
                    if ring[v] != u8::MAX {
                        let w = 1.0 - ring[v] as f64 / (k as f64 + 1.0);
                        back[v] = back[v].max(w);
                    }
                }
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
    let after = diff(base, &base_bend, m, topo, kinds, p);
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
