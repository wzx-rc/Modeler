//! Re-symmetry: the mirror plane square to an axis (through the middle of the
//! mesh, then refined on the mesh itself), then both sides made mirror
//! images: averaged, or one side copied onto the other.

use crate::mesh::{Grid, Mesh, Surface};
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

/// The mirror plane square to `axis`: first through the middle of the box,
/// then like ICP (each vertex's image against the nearest vertex) for its
/// offset and a small tilt.
pub fn find(m: &Mesh, axis: Axis, reach: f64) -> Found {
    let (lo, hi) = m.bounds();
    let diag = (hi - lo).length();
    let edge = m.edge_median();
    let grid = Grid::new(&m.pos, (edge * 4.0).max(diag / 400.0));
    let step = (m.pos.len() / 20000).max(1);
    let idx: Vec<usize> = (0..m.pos.len()).step_by(step).collect();
    let n0 = axis.dir();
    let mut mir = Mirror { n: n0, d: n0.dot((lo + hi) * 0.5) };
    let mut search = diag * 0.05;
    for _ in 0..10 {
        let pairs: Vec<(DVec3, DVec3)> = idx
            .par_iter()
            .filter_map(|&i| {
                let p = m.pos[i];
                let q = mir.apply(p);
                grid.nearest(&m.pos, q, search).map(|j| (p, m.pos[j as usize]))
            })
            .collect();
        if pairs.len() < 50 {
            break;
        }
        // each pair's bisector: the normal from p to its partner's image
        let mut nsum = DVec3::ZERO;
        for (p, q) in &pairs {
            let v = *p - *q;
            if v.length() > edge * 0.5 {
                nsum += if v.dot(mir.n) >= 0.0 { v } else { -v };
            }
        }
        let mut n = if nsum.length() > 1e-9 { nsum.normalize() } else { mir.n };
        // not more than 10° from the axis asked
        if n.dot(n0) < 10f64.to_radians().cos() {
            n = mir.n;
        }
        if n.dot(n0) < 0.0 {
            n = -n;
        }
        let d = median(pairs.iter().map(|(p, q)| n.dot((*p + *q) * 0.5)).collect());
        mir = Mirror { n, d };
        search = (search * 0.6).max(reach);
    }
    let gaps: Vec<f64> = idx
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
/// from the surface (farther, the vertex stays: what has no partner).
pub fn apply(m: &Mesh, surf: &Surface, mir: &Mirror, mode: SymMode, reach: f64) -> (Moves, f64) {
    let edge = m.edge_median();
    let targets: Vec<Option<(u32, DVec3)>> = (0..m.pos.len())
        .into_par_iter()
        .map(|v| {
            let p = m.pos[v];
            let side = mir.dist(p);
            let q = mir.apply(p);
            match mode {
                SymMode::Average => {
                    let (_, c) = surf.closest(m, q, reach)?;
                    Some((v as u32, (p + mir.apply(c)) * 0.5))
                }
                SymMode::PlusToMinus | SymMode::MinusToPlus => {
                    let keep_plus = mode == SymMode::PlusToMinus;
                    // this side is the one copied: it stays
                    if (side >= 0.0) == keep_plus {
                        return None;
                    }
                    // the closest point on the side copied (never across the plane)
                    let (_, c) = surf.closest_where(m, q, reach, |t| {
                        let s = mir.dist(m.tri_centre(t as usize));
                        if keep_plus {
                            s >= -edge
                        } else {
                            s <= edge
                        }
                    })?;
                    Some((v as u32, mir.apply(c)))
                }
            }
        })
        .collect();
    let found = targets.iter().filter(|t| t.is_some()).count();
    let considered = match mode {
        SymMode::Average => m.pos.len(),
        SymMode::PlusToMinus => m.pos.iter().filter(|p| mir.dist(**p) < 0.0).count(),
        SymMode::MinusToPlus => m.pos.iter().filter(|p| mir.dist(**p) >= 0.0).count(),
    };
    let moves = Moves::new(m, targets.into_iter().flatten());
    (moves, found as f64 / considered.max(1) as f64)
}
