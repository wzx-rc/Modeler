//! Smoothing that keeps the edges: bilateral filtering of the face normals
//! (Zheng et al. 2011) then the vertices moved to agree with the filtered
//! normals (Sun et al. 2007). Flat areas flatten, edges get tighter instead
//! of rounder.

use crate::mesh::{Grid, Mesh, Topo};
use glam::DVec3;
use rayon::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmoothParams {
    /// Neighbourhood radius (mm).
    pub radius: f64,
    /// How different two normals may be and still be averaged (degrees).
    pub angle: f64,
    pub normal_iters: usize,
    pub vertex_iters: usize,
}

impl Default for SmoothParams {
    fn default() -> Self {
        SmoothParams { radius: 0.6, angle: 20.0, normal_iters: 4, vertex_iters: 12 }
    }
}

/// Filters the face normals; returns them (also used by the segmentation).
pub fn filtered_normals(m: &Mesh, p: &SmoothParams) -> Vec<DVec3> {
    let centres = m.centres();
    let areas = m.areas();
    let mut normals = m.normals();
    let grid = Grid::new(&centres, p.radius);
    let ss = p.radius * 0.5;
    let sr = 2.0 * (p.angle.to_radians() * 0.5).sin();
    let (ks, kr) = (-0.5 / (ss * ss), -0.5 / (sr * sr));
    let r2 = p.radius * p.radius;
    for _ in 0..p.normal_iters {
        let cur = &normals;
        normals = (0..centres.len())
            .into_par_iter()
            .map(|i| {
                let (c, n) = (centres[i], cur[i]);
                let mut sum = DVec3::ZERO;
                grid.near(c, p.radius, |j| {
                    let j = j as usize;
                    let d2 = (centres[j] - c).length_squared();
                    if d2 > r2 {
                        return;
                    }
                    let nj = cur[j];
                    let dn = (nj - n).length_squared();
                    sum += nj * (areas[j] * (ks * d2 + kr * dn).exp());
                });
                sum.normalize_or(n)
            })
            .collect();
    }
    normals
}

/// Moves the vertices so that the faces take the filtered normals.
pub fn fit_vertices(m: &mut Mesh, topo: &Topo, normals: &[DVec3], iters: usize) {
    for _ in 0..iters {
        let centres = m.centres();
        let pos = &m.pos;
        let new: Vec<DVec3> = (0..pos.len())
            .into_par_iter()
            .map(|v| {
                let x = pos[v];
                let ts = topo.tris_of(v);
                if ts.is_empty() {
                    return x;
                }
                let mut d = DVec3::ZERO;
                for &t in ts {
                    let n = normals[t as usize];
                    d += n * n.dot(centres[t as usize] - x);
                }
                x + d / ts.len() as f64
            })
            .collect();
        m.pos = new;
    }
}

pub fn smooth(m: &mut Mesh, topo: &Topo, p: &SmoothParams) -> Vec<DVec3> {
    let n = filtered_normals(m, p);
    fit_vertices(m, topo, &n, p.vertex_iters);
    n
}
