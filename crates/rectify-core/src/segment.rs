//! Cutting the mesh into regions, each one a plane, a cylinder or a sphere
//! (region growing driven by the shape: from a seed face, neighbours are
//! taken while they lie on the shape, which is refitted as it grows). From
//! each seed the shapes are tried and the one that covers the most wins, so a
//! barrel is one cylinder and not strips of planes.

use crate::mesh::{Mesh, Topo, NONE};
use crate::prim::{axis_of, fit_cylinder, fit_plane, fit_sphere, total_weight, Kind, Prim, Sample};
use glam::DVec3;
use rayon::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegParams {
    /// Largest distance from a face to its shape (mm).
    pub tol: f64,
    /// Largest angle between a face's normal and its shape's (degrees).
    pub angle: f64,
    /// Smallest region kept (mm²).
    pub min_area: f64,
    pub cylinders: bool,
    pub spheres: bool,
    /// Largest cylinder / sphere radius (mm): beyond, it is a plane.
    pub max_radius: f64,
}

impl Default for SegParams {
    fn default() -> Self {
        SegParams { tol: 0.2, angle: 18.0, min_area: 5.0, cylinders: true, spheres: true, max_radius: 60.0 }
    }
}

#[derive(Clone, Debug)]
pub struct Region {
    pub prim: Prim,
    pub faces: Vec<u32>,
    pub area: f64,
    /// Distance of the faces to the shape before snapping (mm, RMS).
    pub rms: f64,
    /// Set by hand: never changed by the regularization.
    pub locked: bool,
}

pub struct Segmentation {
    /// Region of each triangle, NONE when free.
    pub label: Vec<u32>,
    pub regions: Vec<Region>,
}

impl Segmentation {
    pub fn count(&self, k: Kind) -> usize {
        self.regions.iter().filter(|r| r.prim.kind() == k).count()
    }
    pub fn covered(&self, areas: &[f64]) -> f64 {
        let total: f64 = areas.iter().sum();
        let on: f64 = self.label.iter().zip(areas).filter(|(l, _)| **l != NONE).map(|(_, a)| a).sum();
        on / total.max(1e-300)
    }
}

pub struct Ctx<'a> {
    pub topo: &'a Topo,
    pub samples: Vec<Sample>,
}

impl<'a> Ctx<'a> {
    pub fn new(m: &Mesh, topo: &'a Topo) -> Ctx<'a> {
        let samples = (0..m.tris.len())
            .into_par_iter()
            .map(|t| {
                let c = m.tri_cross(t);
                let l = c.length();
                Sample { p: m.tri_centre(t), n: if l > 0.0 { c / l } else { DVec3::Y }, w: l * 0.5 }
            })
            .collect();
        Ctx { topo, samples }
    }

    pub fn region_samples(&self, faces: &[u32]) -> Vec<Sample> {
        faces.iter().map(|&f| self.samples[f as usize]).collect()
    }
}

/// Grows a region from `seed` on the shape, among the faces not labelled yet
/// (`mark` is scratch space: faces already reached this attempt hold `gen`).
fn grow(ctx: &Ctx, label: &[u32], seed: u32, mut prim: Prim, p: &SegParams, mark: &mut [u32], gen: &mut u32) -> Option<(Vec<u32>, Prim)> {
    let cos = p.angle.to_radians().cos();
    let mut faces = vec![];
    // two passes: the second regrows with the shape fitted on the first
    for pass in 0..2 {
        *gen += 1;
        let g = *gen;
        faces.clear();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(seed);
        mark[seed as usize] = g;
        let mut next_fit = 24usize;
        while let Some(f) = queue.pop_front() {
            let s = &ctx.samples[f as usize];
            if f != seed && !prim.fits(s, p.tol, cos) {
                continue;
            }
            faces.push(f);
            for &o in &ctx.topo.adj[f as usize] {
                if o != NONE && mark[o as usize] != g && label[o as usize] == NONE {
                    mark[o as usize] = g;
                    queue.push_back(o);
                }
            }
            if pass == 0 && faces.len() >= next_fit {
                next_fit = faces.len() * 2;
                if let Some(q) = refit(ctx, &faces, &prim, p) {
                    prim = q;
                }
            }
        }
        if let Some(q) = refit(ctx, &faces, &prim, p) {
            prim = q;
        }
        if faces.len() < 3 {
            return None;
        }
    }
    Some((faces, prim))
}

fn refit(ctx: &Ctx, faces: &[u32], prim: &Prim, p: &SegParams) -> Option<Prim> {
    let s = ctx.region_samples(faces);
    let q = match prim.kind() {
        Kind::Plane => fit_plane(&s),
        Kind::Cylinder => {
            // the axis from the normals needs enough turn; else keep it
            let (_, l) = axis_of(&s);
            let axis = if l[1] > 0.004 { None } else if let Prim::Cylinder { a, .. } = prim { Some(*a) } else { None };
            fit_cylinder(&s, axis, None)
        }
        Kind::Sphere => fit_sphere(&s, None),
    }?;
    valid(&q, p).then_some(q)
}

fn valid(q: &Prim, p: &SegParams) -> bool {
    match *q {
        Prim::Plane { n, d } => n.is_finite() && d.is_finite(),
        Prim::Cylinder { r, p: c, a } => r.is_finite() && c.is_finite() && a.is_finite() && r <= p.max_radius && r >= p.tol * 2.0,
        Prim::Sphere { r, c } => r.is_finite() && c.is_finite() && r <= p.max_radius && r >= p.tol * 2.0,
    }
}

/// Faces within a few rings of `seed`, not labelled.
fn patch(ctx: &Ctx, label: &[u32], seed: u32, want: usize, mark: &mut [u32], gen: &mut u32) -> Vec<u32> {
    *gen += 1;
    let g = *gen;
    let mut out = vec![seed];
    mark[seed as usize] = g;
    let mut i = 0;
    while i < out.len() && out.len() < want {
        let f = out[i];
        i += 1;
        for &o in &ctx.topo.adj[f as usize] {
            if o != NONE && mark[o as usize] != g && label[o as usize] == NONE {
                mark[o as usize] = g;
                out.push(o);
            }
        }
    }
    out
}

pub fn segment(m: &Mesh, topo: &Topo, p: &SegParams, progress: &(dyn Fn(f32) + Sync)) -> Segmentation {
    let ctx = Ctx::new(m, topo);
    let nt = m.tris.len();
    let mut label = vec![NONE; nt];
    let mut tried = vec![false; nt];
    let mut mark = vec![0u32; nt];
    let mut gen = 0u32;
    // seeds: the flattest faces first (normals of the ring around agree)
    let flat: Vec<f64> = (0..nt)
        .into_par_iter()
        .map(|t| {
            let n = ctx.samples[t].n;
            let mut s = 0.0;
            for &o in &topo.adj[t] {
                if o != NONE {
                    s += 1.0 - n.dot(ctx.samples[o as usize].n);
                }
            }
            s
        })
        .collect();
    let mut order: Vec<u32> = (0..nt as u32).collect();
    order.par_sort_by(|&a, &b| flat[a as usize].partial_cmp(&flat[b as usize]).unwrap());
    let mut regions: Vec<Region> = vec![];
    // the seed patch for curved shapes: about 1.5 mm across
    let edge = m.edge_median();
    let patch_n = ((1.5 / edge.max(1e-6)).powi(2) as usize).clamp(30, 600);
    for (k, &seed) in order.iter().enumerate() {
        if k % 20000 == 0 {
            progress(k as f32 / nt as f32);
        }
        if label[seed as usize] != NONE || tried[seed as usize] {
            continue;
        }
        let s = ctx.samples[seed as usize];
        let mut best: Option<(f64, Vec<u32>, Prim)> = None;
        let plane = Prim::Plane { n: s.n, d: s.n.dot(s.p) };
        if let Some((f, q)) = grow(&ctx, &label, seed, plane, p, &mut mark, &mut gen) {
            let a: f64 = f.iter().map(|&i| ctx.samples[i as usize].w).sum();
            best = Some((a, f, q));
        }
        // curved shapes from the patch around the seed
        if p.cylinders || p.spheres {
            let pf = patch(&ctx, &label, seed, patch_n, &mut mark, &mut gen);
            let ps = ctx.region_samples(&pf);
            let (_, l) = axis_of(&ps);
            let mut tries = vec![];
            if p.cylinders && l[1] > 0.004 {
                if let Some(c) = fit_cylinder(&ps, None, None) {
                    tries.push(c);
                }
            }
            if p.spheres && l[0] > 0.02 {
                if let Some(c) = fit_sphere(&ps, None) {
                    tries.push(c);
                }
            }
            for c in tries {
                if !valid(&c, p) || c.rms(&ps) > p.tol {
                    continue;
                }
                if let Some((f, q)) = grow(&ctx, &label, seed, c, p, &mut mark, &mut gen) {
                    let a: f64 = f.iter().map(|&i| ctx.samples[i as usize].w).sum();
                    // a curved shape must clearly beat the plane
                    let beat = match &best {
                        Some((ba, _, bq)) => a > ba * if bq.kind() == Kind::Plane { 1.3 } else { 1.0 },
                        None => true,
                    };
                    if beat {
                        best = Some((a, f, q));
                    }
                }
            }
        }
        match best {
            Some((a, f, q)) if a >= p.min_area => {
                let id = regions.len() as u32;
                for &i in &f {
                    label[i as usize] = id;
                }
                let s = ctx.region_samples(&f);
                regions.push(Region { rms: q.rms(&s), prim: q, area: a, faces: f, locked: false });
            }
            Some((_, f, _)) => {
                for &i in &f {
                    tried[i as usize] = true;
                }
                tried[seed as usize] = true;
            }
            None => tried[seed as usize] = true,
        }
    }
    let _ = total_weight;
    progress(1.0);
    Segmentation { label, regions }
}

/// Refits each region's shape on its faces (after the labels changed).
pub fn refit_all(m: &Mesh, topo: &Topo, seg: &mut Segmentation) {
    let ctx = Ctx::new(m, topo);
    seg.regions.par_iter_mut().for_each(|r| {
        let s = ctx.region_samples(&r.faces);
        r.area = total_weight(&s);
        let q = match r.prim.kind() {
            Kind::Plane => fit_plane(&s),
            Kind::Cylinder => fit_cylinder(&s, None, None),
            Kind::Sphere => fit_sphere(&s, None),
        };
        if let Some(q) = q {
            if !r.locked {
                r.prim = q;
            }
        }
        r.rms = r.prim.rms(&s);
    });
}
