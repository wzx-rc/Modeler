//! Re-symmetry: the mirror plane square to an axis (through the middle of the
//! mesh, then refined on the mesh itself), then both sides made mirror
//! images: averaged, or one side copied onto the other.

use crate::mesh::{Grid, Mesh, Surface, Topo};
use crate::moves::Moves;
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
    pub fn dist(&self, p: DVec3) -> f64 {
        self.n.dot(p) - self.d
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];
    pub fn dir(self) -> DVec3 {
        match self {
            Axis::X => DVec3::X,
            Axis::Y => DVec3::Y,
            Axis::Z => DVec3::Z,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Axis::X => "X",
            Axis::Y => "Y",
            Axis::Z => "Z",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymMode {
    /// Each side averaged with the mirror of the other.
    Average,
    /// The + side copied onto the − side.
    PlusToMinus,
    /// The − side copied onto the + side.
    MinusToPlus,
}

/// The plane found, and how well the mesh matches its mirror image.
#[derive(Clone, Copy, Debug)]
pub struct Found {
    pub mirror: Mirror,
    /// Median distance from a vertex's image to the nearest vertex (mm).
    pub gap: f64,
    /// Share of the vertices whose image lands on the mesh (within reach).
    pub matched: f64,
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::INFINITY;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// The mirror plane square to `axis`: its offset searched across the
/// middle of the mesh (the median distance from a vertex's image to the
/// nearest vertex, on a sample), then refined, then a small tilt (under 2°)
/// tried around it.
pub fn find(m: &Mesh, axis: Axis, reach: f64) -> Found {
    let (lo, hi) = m.bounds();
    let diag = (hi - lo).length();
    let edge = m.edge_median();
    let grid = Grid::new(&m.pos, (edge * 4.0).max(diag / 400.0));
    let pick = |want: usize| -> Vec<usize> {
        let step = (m.pos.len() / want).max(1);
        // a hash, not a stride: the file's order follows the shape
        (0..m.pos.len()).filter(|i| (i.wrapping_mul(2654435761) >> 7) % step == 0).collect()
    };
    let idx = pick(6000);
    let cap = diag * 0.05;
    let score = |mir: &Mirror| -> f64 {
        median(
            idx.par_iter()
                .map(|&i| {
                    let q = mir.apply(m.pos[i]);
                    grid.nearest(&m.pos, q, cap).map(|j| (m.pos[j as usize] - q).length()).unwrap_or(cap)
                })
                .collect(),
        )
    };
    let n0 = axis.dir();
    let (a, b) = (n0.dot(lo), n0.dot(hi));
    let ext = b - a;
    // the offset: a scan over the middle half, then finer around the best
    let mut best = (f64::INFINITY, Mirror { n: n0, d: (a + b) * 0.5 });
    let mut step = ext / 100.0;
    let (mut from, mut to) = (a + ext * 0.25, b - ext * 0.25);
    for _ in 0..3 {
        let mut d = from;
        while d <= to + 1e-9 {
            let mir = Mirror { n: n0, d };
            let s = score(&mir);
            if s < best.0 {
                best = (s, mir);
            }
            d += step;
        }
        from = best.1.d - step;
        to = best.1.d + step;
        step /= 8.0;
    }
    // a small tilt
    let (u, v) = (n0.any_orthonormal_vector(), n0.cross(n0.any_orthonormal_vector()));
    let mut ang = 0.5f64.to_radians();
    for _ in 0..4 {
        let mut improved = true;
        while improved {
            improved = false;
            for (du, dv) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                let n = (best.1.n + u * (du * ang) + v * (dv * ang)).normalize();
                if n.dot(n0) < 2f64.to_radians().cos() {
                    continue;
                }
                // through the same middle point
                let c = n0 * best.1.d + u * u.dot((lo + hi) * 0.5) + v * v.dot((lo + hi) * 0.5);
                let mir = Mirror { n, d: n.dot(c) };
                let s = score(&mir);
                if s < best.0 * 0.995 {
                    best = (s, mir);
                    improved = true;
                }
            }
        }
        ang *= 0.5;
    }
    let mir = best.1;
    let all = pick(20000);
    let gaps: Vec<f64> = all
        .par_iter()
        .map(|&i| {
            let q = mir.apply(m.pos[i]);
            grid.nearest(&m.pos, q, reach * 4.0).map(|j| (m.pos[j as usize] - q).length()).unwrap_or(reach * 4.0)
        })
        .collect();
    let matched = gaps.iter().filter(|g| **g <= reach).count() as f64 / gaps.len().max(1) as f64;
    Found { mirror: mir, gap: median(gaps), matched }
}

/// Both sides made mirror images. `reach`: how far a vertex's image may be
/// from the surface (farther, the vertex stays: what has no partner). The
/// moves are then evened out between neighbours, so that two vertices side
/// by side that found their partners on different faces do not tear apart.
pub fn apply(m: &Mesh, topo: &Topo, surf: &Surface, mir: &Mirror, mode: SymMode, reach: f64) -> (Moves, f64) {
    let edge = m.edge_median();
    let keep_plus = mode == SymMode::PlusToMinus;
    // per vertex: the move wanted, how sure it is, and whether it is held
    // (the side copied)
    let first: Vec<(DVec3, f64, bool)> = (0..m.pos.len())
        .into_par_iter()
        .map(|v| {
            let p = m.pos[v];
            let side = mir.dist(p);
            let q = mir.apply(p);
            let sure = |c: DVec3| smooth01((reach - (c - q).length()) / (reach * 0.5));
            match mode {
                SymMode::Average => match surf.closest(m, q, reach) {
                    Some((_, c)) => ((mir.apply(c) - p) * 0.5, sure(c), false),
                    None => (DVec3::ZERO, 0.0, false),
                },
                SymMode::PlusToMinus | SymMode::MinusToPlus => {
                    if (side >= 0.0) == keep_plus {
                        return (DVec3::ZERO, 1.0, true);
                    }
                    let near = surf.closest_where(m, q, reach, |t| {
                        let s = mir.dist(m.tri_centre(t as usize));
                        if keep_plus {
                            s >= -edge
                        } else {
                            s <= edge
                        }
                    });
                    match near {
                        Some((_, c)) => (mir.apply(c) - p, sure(c), false),
                        None => (DVec3::ZERO, 0.0, false),
                    }
                }
            }
        })
        .collect();
    // along the vertex's normal only: sliding along the surface towards a
    // partner on another facet wrinkles it
    let normals = m.vertex_normals(topo);
    let first: Vec<(DVec3, f64, bool)> = first.into_iter().enumerate().map(|(v, (d, c, held))| (normals[v] * d.dot(normals[v]), c, held)).collect();
    let found = first.iter().filter(|x| !x.2 && x.1 > 0.0).count();
    let considered = first.iter().filter(|x| !x.2).count();
    let mut disp: Vec<DVec3> = first.iter().map(|x| x.0).collect();
    let mut conf: Vec<f64> = first.iter().map(|x| x.1).collect();
    for _ in 0..6 {
        let next: Vec<(DVec3, f64)> = (0..m.pos.len())
            .into_par_iter()
            .map(|v| {
                if first[v].2 {
                    return (DVec3::ZERO, 1.0);
                }
                let mut sd = disp[v] * conf[v];
                let mut sc = conf[v];
                let mut cn = conf[v];
                let nb = topo.verts_of(v);
                for &u in nb {
                    sd += disp[u as usize] * conf[u as usize];
                    sc += conf[u as usize];
                    cn += conf[u as usize];
                }
                let d = if sc > 1e-12 { sd / sc } else { DVec3::ZERO };
                (d, cn / (nb.len() + 1) as f64)
            })
            .collect();
        for (v, (d, c)) in next.into_iter().enumerate() {
            disp[v] = d;
            conf[v] = c;
        }
    }
    let moves = Moves::new(m, (0..m.pos.len()).filter(|&v| !first[v].2).map(|v| (v as u32, m.pos[v] + disp[v] * conf[v])));
    (moves, found as f64 / considered.max(1) as f64)
}

fn smooth01(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}
