//! The mirror plane of the object: found from the three main axes of its
//! points, then refined like ICP (each point against the nearest point of the
//! mirrored object). Symmetrizing then averages each side with the mirror of
//! the other: half of the noise goes away.

use crate::mesh::{closest_on_tri, eigen_sym, add_outer, Grid, Mesh, Topo};
use glam::DVec3;
use rayon::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mirror {
    /// Unit normal and offset: the plane is n · x = d.
    pub n: DVec3,
    pub d: f64,
}

impl Mirror {
    pub fn apply(&self, p: DVec3) -> DVec3 {
        p - self.n * (2.0 * (self.n.dot(p) - self.d))
    }
    pub fn apply_dir(&self, v: DVec3) -> DVec3 {
        v - self.n * (2.0 * self.n.dot(v))
    }
    pub fn dist(&self, p: DVec3) -> f64 {
        self.n.dot(p) - self.d
    }
}

#[derive(Clone, Debug)]
pub struct SymReport {
    pub mirror: Mirror,
    /// Median distance between a point and the mirrored object (mm).
    pub median: f64,
    /// Share of the points that have a mirror partner within the tolerance.
    pub matched: f64,
}

fn samples(n: usize, want: usize) -> Vec<usize> {
    let step = (n / want.max(1)).max(1);
    (0..n).step_by(step).collect()
}

/// Distance from each sampled point's mirror image to the nearest vertex.
fn mirror_dists(m: &Mesh, grid: &Grid, mir: &Mirror, idx: &[usize], r: f64) -> Vec<(usize, f64, Option<u32>)> {
    idx.par_iter()
        .map(|&i| {
            let q = mir.apply(m.pos[i]);
            match grid.nearest(&m.pos, q, r) {
                Some(j) => (i, (m.pos[j as usize] - q).length(), Some(j)),
                None => (i, r, None),
            }
        })
        .collect()
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Finds the mirror plane. `tol` is the distance under which a point counts
/// as matched (mm).
pub fn find(m: &Mesh, tol: f64) -> SymReport {
    let (lo, hi) = m.bounds();
    let diag = (hi - lo).length();
    let c: DVec3 = m.pos.iter().copied().sum::<DVec3>() / m.pos.len() as f64;
    let mut cov = [[0.0; 3]; 3];
    for p in m.pos.iter().step_by((m.pos.len() / 200000).max(1)) {
        add_outer(&mut cov, *p - c, 1.0);
    }
    let (_, axes) = eigen_sym(cov);
    let edge = m.edge_median();
    let grid = Grid::new(&m.pos, (edge * 4.0).max(diag / 400.0));
    let idx = samples(m.pos.len(), 20000);
    let search = diag * 0.05;
    // the best of the three axes through the centre
    let mut best: Option<(f64, Mirror)> = None;
    for a in axes {
        let mir = Mirror { n: a, d: a.dot(c) };
        let med = median(mirror_dists(m, &grid, &mir, &idx, search).into_iter().map(|x| x.1).collect());
        if best.map(|b| med < b.0).unwrap_or(true) {
            best = Some((med, mir));
        }
    }
    let mut mir = best.unwrap().1;
    // ICP: each point and the nearest vertex to its image are mirror
    // partners; the plane between them is refitted
    let mut r = search;
    for _ in 0..25 {
        let pairs = mirror_dists(m, &grid, &mir, &idx, r);
        let med = median(pairs.iter().map(|x| x.1).collect());
        let keep = (med * 2.5).max(edge);
        let mut nsum = DVec3::ZERO;
        let mut dsum = 0.0;
        let mut w = 0.0;
        for (i, d, j) in &pairs {
            let Some(j) = j else { continue };
            if *d > keep {
                continue;
            }
            let p = m.pos[*i];
            let q = m.pos[*j as usize];
            let v = p - q;
            // the partner of a point on the plane is itself: no direction
            if v.length() > edge * 2.0 {
                nsum += if v.dot(mir.n) >= 0.0 { v } else { -v };
            }
            dsum += mir.n.dot((p + q) * 0.5);
            w += 1.0;
        }
        if w < 10.0 {
            break;
        }
        let n = nsum.normalize_or(mir.n);
        let d = dsum / w;
        // the offset was measured along the old normal: close enough as it
        // converges
        let moved = (n - mir.n).length() + (d - mir.d).abs() / diag;
        mir = Mirror { n, d };
        r = (med * 6.0).max(edge * 4.0).min(search);
        if moved < 1e-7 {
            break;
        }
    }
    let pairs = mirror_dists(m, &grid, &mir, &idx, search);
    let med = median(pairs.iter().map(|x| x.1).collect());
    let matched = pairs.iter().filter(|x| x.1 < tol).count() as f64 / pairs.len().max(1) as f64;
    SymReport { mirror: mir, median: med, matched }
}

/// Moves every vertex half-way to the mirror image of the closest point of
/// the other side (when it is within `tol`); vertices within `plane_tol` of
/// the plane go onto it. Returns how many vertices moved.
pub fn symmetrize(m: &mut Mesh, topo: &Topo, mir: &Mirror, tol: f64, plane_tol: f64) -> usize {
    let edge = m.edge_median();
    let grid = Grid::new(&m.pos, (edge * 3.0).max(tol));
    let old = &m.pos;
    let new: Vec<Option<DVec3>> = (0..old.len())
        .into_par_iter()
        .map(|i| {
            let p = old[i];
            if mir.dist(p).abs() < plane_tol {
                return Some(p - mir.n * mir.dist(p));
            }
            let q = mir.apply(p);
            let j = grid.nearest(old, q, tol.max(edge * 3.0))?;
            // closest point on the triangles around the nearest vertex
            let mut best = (f64::MAX, q);
            for &t in topo.tris_of(j as usize) {
                let [a, b, c] = m.tri_pts(t as usize);
                let x = closest_on_tri(q, a, b, c);
                let d = (x - q).length_squared();
                if d < best.0 {
                    best = (d, x);
                }
            }
            if best.0.sqrt() > tol {
                return None;
            }
            Some((p + mir.apply(best.1)) * 0.5)
        })
        .collect();
    let mut moved = 0;
    for (i, n) in new.into_iter().enumerate() {
        if let Some(n) = n {
            m.pos[i] = n;
            moved += 1;
        }
    }
    moved
}
