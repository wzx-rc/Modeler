//! Tidying the regions: neighbours that are really one shape are merged,
//! borders follow the line where the two shapes meet (each face goes to the
//! shape it is closest to, then a vote smooths the zigzags), and stray bits
//! of a region far from its main piece are let go.

use crate::mesh::{Mesh, Topo, NONE};
use crate::prim::{fit_cylinder, fit_plane, fit_sphere, Kind, Prim, Sample};
use crate::segment::{Ctx, Region, SegParams, Segmentation};
use rayon::prelude::*;
use std::collections::HashMap;

/// Rebuilds each region's face list and area from the labels (regions keep
/// their index).
pub fn rebuild(ctx: &Ctx, seg: &mut Segmentation) {
    for r in seg.regions.iter_mut() {
        r.faces.clear();
        r.area = 0.0;
    }
    for (t, &l) in seg.label.iter().enumerate() {
        if l != NONE {
            let r = &mut seg.regions[l as usize];
            r.faces.push(t as u32);
            r.area += ctx.samples[t].w;
        }
    }
}

/// Drops the regions left without faces (indices change).
pub fn compact(seg: &mut Segmentation) {
    let mut map = vec![NONE; seg.regions.len()];
    let mut out = vec![];
    for (i, r) in seg.regions.drain(..).enumerate() {
        if !r.faces.is_empty() {
            map[i] = out.len() as u32;
            out.push(r);
        }
    }
    for l in seg.label.iter_mut() {
        if *l != NONE {
            *l = map[*l as usize];
        }
    }
    seg.regions = out;
}

fn fit_kind(k: Kind, s: &[Sample]) -> Option<Prim> {
    match k {
        Kind::Plane => fit_plane(s),
        Kind::Cylinder => fit_cylinder(s, None, None),
        Kind::Sphere => fit_sphere(s, None),
    }
}

/// Share of the area that does not fit the shape.
fn misfit(q: &Prim, s: &[Sample], tol: f64, cos: f64) -> f64 {
    let (mut bad, mut all) = (0.0, 0.0);
    for x in s {
        all += x.w;
        if !q.fits(x, tol, cos) {
            bad += x.w;
        }
    }
    bad / all.max(1e-300)
}

/// Merges neighbouring regions whose faces together still fit one shape.
pub fn merge(m: &Mesh, topo: &Topo, seg: &mut Segmentation, p: &SegParams) -> usize {
    let ctx = Ctx::new(m, topo);
    let cos = p.angle.to_radians().cos();
    let mut merged = 0;
    for _round in 0..6 {
        // shared border length (in sides) between each pair of regions
        let mut pairs: HashMap<(u32, u32), u32> = HashMap::new();
        for t in 0..seg.label.len() {
            let a = seg.label[t];
            if a == NONE {
                continue;
            }
            for &o in &topo.adj[t] {
                if o == NONE || (o as usize) < t {
                    continue;
                }
                let b = seg.label[o as usize];
                if b != NONE && b != a {
                    *pairs.entry((a.min(b), a.max(b))).or_default() += 1;
                }
            }
        }
        let mut list: Vec<((u32, u32), u32)> = pairs.into_iter().collect();
        list.sort_by(|a, b| b.1.cmp(&a.1));
        // test the pairs in parallel, merge greedily (each region once a round)
        let tests: Vec<Option<Prim>> = list
            .par_iter()
            .map(|&((a, b), _)| {
                let (ra, rb) = (&seg.regions[a as usize], &seg.regions[b as usize]);
                if ra.locked || rb.locked {
                    return None;
                }
                let mut s = ctx.region_samples(&ra.faces);
                s.extend(ctx.region_samples(&rb.faces));
                let mut kinds = vec![ra.prim.kind(), rb.prim.kind()];
                kinds.dedup();
                let mut best: Option<(f64, Prim)> = None;
                for k in kinds {
                    if let Some(q) = fit_kind(k, &s) {
                        let ok = match q {
                            Prim::Cylinder { r, .. } | Prim::Sphere { r, .. } => r <= p.max_radius && r >= p.tol * 2.0,
                            _ => true,
                        };
                        let bad = misfit(&q, &s, p.tol, cos);
                        if ok && bad < 0.04 && best.map(|b| bad < b.0).unwrap_or(true) {
                            best = Some((bad, q));
                        }
                    }
                }
                best.map(|b| b.1)
            })
            .collect();
        let mut used = vec![false; seg.regions.len()];
        let mut n = 0;
        for (((a, b), _), q) in list.iter().zip(tests) {
            let Some(q) = q else { continue };
            let (a, b) = (*a as usize, *b as usize);
            if used[a] || used[b] {
                continue;
            }
            used[a] = true;
            used[b] = true;
            let fb = std::mem::take(&mut seg.regions[b].faces);
            for &f in &fb {
                seg.label[f as usize] = a as u32;
            }
            seg.regions[a].faces.extend(fb);
            seg.regions[a].prim = q;
            n += 1;
        }
        merged += n;
        rebuild(&ctx, seg);
        compact(seg);
        if n == 0 {
            break;
        }
    }
    let ctx = Ctx::new(m, topo);
    for r in seg.regions.iter_mut() {
        let s = ctx.region_samples(&r.faces);
        r.rms = r.prim.rms(&s);
    }
    merged
}

/// Borders: a face next to another region goes to it when it is closer to
/// that region's shape (repeated until it settles), then a vote: a face whose
/// neighbours are mostly of another region joins it.
pub fn borders(m: &Mesh, topo: &Topo, seg: &mut Segmentation, angle: f64) -> usize {
    let ctx = Ctx::new(m, topo);
    let cos = angle.to_radians().cos();
    let mut moved = 0;
    for _ in 0..40 {
        let label = &seg.label;
        let changes: Vec<(usize, u32)> = (0..label.len())
            .into_par_iter()
            .filter_map(|t| {
                let a = label[t];
                if a == NONE {
                    return None;
                }
                let s = &ctx.samples[t];
                let da = seg.regions[a as usize].prim.dist(s.p);
                let mut best = (da, a);
                for &o in &topo.adj[t] {
                    if o == NONE {
                        continue;
                    }
                    let b = label[o as usize];
                    if b == NONE || b == a {
                        continue;
                    }
                    let q = &seg.regions[b as usize].prim;
                    let d = q.dist(s.p);
                    let c = q.normal_at(s.p).dot(s.n);
                    let c = if matches!(q, Prim::Plane { .. }) { c } else { c.abs() };
                    if d < best.0 && c > cos.min(0.5) {
                        best = (d, b);
                    }
                }
                (best.1 != a).then_some((t, best.1))
            })
            .collect();
        if changes.is_empty() {
            break;
        }
        moved += changes.len();
        for (t, b) in changes {
            seg.label[t] = b;
        }
    }
    vote(topo, &mut seg.label, 4);
    rebuild(&ctx, seg);
    moved
}

/// A face whose neighbours (2 of 3) share another region joins it.
pub fn vote(topo: &Topo, label: &mut [u32], passes: usize) {
    for _ in 0..passes {
        let cur = label.to_vec();
        label.par_iter_mut().enumerate().for_each(|(t, l)| {
            let a = topo.adj[t];
            let n: Vec<u32> = a.iter().filter(|&&o| o != NONE).map(|&o| cur[o as usize]).collect();
            if n.len() == 3 {
                for k in 0..3 {
                    let c = n[k];
                    if c != NONE && c != cur[t] && n.iter().filter(|&&x| x == c).count() >= 2 {
                        *l = c;
                        break;
                    }
                }
            }
        });
    }
}

/// Pieces of a region apart from its largest piece, smaller than `min_area`,
/// are freed. Returns how many faces were freed.
pub fn islands(m: &Mesh, topo: &Topo, seg: &mut Segmentation, min_area: f64) -> usize {
    let ctx = Ctx::new(m, topo);
    let n = seg.label.len();
    let mut comp = vec![NONE; n];
    let mut pieces: Vec<(u32, f64, Vec<u32>)> = vec![];
    for t in 0..n {
        if seg.label[t] == NONE || comp[t] != NONE {
            continue;
        }
        let l = seg.label[t];
        let id = pieces.len() as u32;
        let mut faces = vec![t as u32];
        comp[t] = id;
        let mut i = 0;
        let mut area = 0.0;
        while i < faces.len() {
            let f = faces[i] as usize;
            i += 1;
            area += ctx.samples[f].w;
            for &o in &topo.adj[f] {
                if o != NONE && comp[o as usize] == NONE && seg.label[o as usize] == l {
                    comp[o as usize] = id;
                    faces.push(o);
                }
            }
        }
        pieces.push((l, area, faces));
    }
    let mut biggest: HashMap<u32, f64> = HashMap::new();
    for (l, a, _) in &pieces {
        let e = biggest.entry(*l).or_insert(0.0);
        *e = e.max(*a);
    }
    let mut freed = 0;
    for (l, a, faces) in &pieces {
        if *a < biggest[l] && *a < min_area {
            for &f in faces {
                seg.label[f as usize] = NONE;
            }
            freed += faces.len();
        }
    }
    rebuild(&ctx, seg);
    freed
}

/// Regions too small after all this are freed.
pub fn small(seg: &mut Segmentation, min_area: f64) {
    for r in seg.regions.iter_mut() {
        if r.area < min_area && !r.locked {
            for &f in &r.faces {
                seg.label[f as usize] = NONE;
            }
            r.faces.clear();
            r.area = 0.0;
        }
    }
}

pub fn refit(m: &Mesh, topo: &Topo, regions: &mut [Region]) {
    let ctx = Ctx::new(m, topo);
    regions.par_iter_mut().for_each(|r| {
        if r.faces.is_empty() || r.locked {
            return;
        }
        let s = ctx.region_samples(&r.faces);
        if let Some(q) = fit_kind(r.prim.kind(), &s) {
            r.prim = q;
        }
        r.rms = r.prim.rms(&s);
    });
}
