//! The Edges tool (Romain, 18 nov. 2026: "globalement toutes les arêtes plus
//! sharp, donc un peu d'auto pour les arêtes"): the straight edges between
//! flat faces found on the whole mesh, each then fixed like a Line.
//!
//! The flat faces: triangles grown into regions while their (lightly
//! smoothed) normals stay within a few degrees of the region's and their
//! centres close to its plane, the large ones kept. The rounded strip between
//! two of them is shared out to the nearest (a few rings at most); where two
//! faces meet in it and turn enough, the line where their planes meet is the
//! edge, cut into runs where they really meet along it.

use crate::line::{self, meet, LineFit, LineParams};
use crate::mesh::{Grid, Mesh, Topo, NONE};
use glam::DVec3;
use rayon::prelude::*;
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeParams {
    /// Two faces meeting at less than this do not make an edge (degrees).
    pub min_angle: f64,
    /// Shorter runs are left alone (mm).
    pub min_len: f64,
    /// A flat face is at least this large (mm²).
    pub min_area: f64,
    /// How wide the rounded strip between two faces may be (mm).
    pub reach: f64,
}

impl Default for EdgeParams {
    fn default() -> Self {
        EdgeParams { min_angle: 25.0, min_len: 3.0, min_area: 4.0, reach: 2.5 }
    }
}

/// A straight edge found between two flat faces.
#[derive(Clone, Debug)]
pub struct Found {
    pub a: DVec3,
    pub b: DVec3,
    /// The angle between the faces' normals (degrees).
    pub angle: f64,
    /// The two faces (region ids).
    pub faces: (u32, u32),
}

fn debug() -> bool {
    std::env::var("RECTIFY_DEBUG").is_ok()
}

/// The flat faces: a region id per triangle (NONE: on none), and each
/// region's plane (point, unit normal) and area.
pub fn flat_faces(m: &Mesh, topo: &Topo, p: &EdgeParams) -> (Vec<u32>, Vec<(DVec3, DVec3, f64)>) {
    let nt = m.tris.len();
    let h = m.edge_median();
    let area: Vec<f64> = (0..nt).into_par_iter().map(|t| m.tri_area(t)).collect();
    let centre: Vec<DVec3> = (0..nt).into_par_iter().map(|t| m.tri_centre(t)).collect();
    let mut ns: Vec<DVec3> = (0..nt).into_par_iter().map(|t| m.tri_normal(t)).collect();
    // smoothed a little: the AI mesh's noise is finer than its faces
    for _ in 0..3 {
        ns = (0..nt)
            .into_par_iter()
            .map(|t| {
                let mut s = ns[t] * area[t];
                for &u in &topo.adj[t] {
                    if u != NONE {
                        s += ns[u as usize] * area[u as usize];
                    }
                }
                s.normalize_or_zero()
            })
            .collect();
    }
    // how flat around each triangle (the largest turn to a neighbour)
    let flat: Vec<f64> = (0..nt)
        .into_par_iter()
        .map(|t| topo.adj[t].iter().filter(|&&u| u != NONE).map(|&u| 1.0 - ns[t].dot(ns[u as usize])).fold(0.0, f64::max))
        .collect();
    let mut order: Vec<u32> = (0..nt as u32).filter(|&t| flat[t as usize] < 1.0 - 2f64.to_radians().cos()).collect();
    order.par_sort_unstable_by(|a, b| flat[*a as usize].partial_cmp(&flat[*b as usize]).unwrap());
    let cos_tol = 6f64.to_radians().cos();
    let dist_tol = (h * 0.5).max(0.06);
    let mut label = vec![NONE; nt];
    let mut planes: Vec<(DVec3, DVec3, f64)> = vec![];
    let mut q: VecDeque<u32> = VecDeque::new();
    for &s in &order {
        if label[s as usize] != NONE {
            continue;
        }
        let id = planes.len() as u32;
        let mut members: Vec<u32> = vec![s];
        label[s as usize] = id;
        let (mut sn, mut sc, mut sa) = (ns[s as usize] * area[s as usize], centre[s as usize] * area[s as usize], area[s as usize]);
        let (mut n, mut c) = (ns[s as usize], centre[s as usize]);
        q.clear();
        q.push_back(s);
        let mut next_fit = 8usize;
        while let Some(t) = q.pop_front() {
            for &u in &topo.adj[t as usize] {
                if u == NONE || label[u as usize] != NONE {
                    continue;
                }
                let ui = u as usize;
                if ns[ui].dot(n) < cos_tol || n.dot(centre[ui] - c).abs() > dist_tol {
                    continue;
                }
                label[ui] = id;
                members.push(u);
                sn += ns[ui] * area[ui];
                sc += centre[ui] * area[ui];
                sa += area[ui];
                if members.len() >= next_fit {
                    n = sn.normalize_or_zero();
                    c = sc / sa;
                    next_fit = members.len() * 2;
                }
                q.push_back(u);
            }
        }
        n = sn.normalize_or_zero();
        c = sc / sa;
        if sa < p.min_area {
            // too small to be a face: given back (marked so that it is not
            // seeded again; its id goes to the next one)
            for &t in &members {
                label[t as usize] = NONE - 1;
            }
            continue;
        }
        planes.push((c, n, sa));
    }
    for l in label.iter_mut() {
        if *l == NONE - 1 {
            *l = NONE;
        }
    }
    (label, planes)
}

/// The straight edges between flat faces.
pub fn find(m: &Mesh, topo: &Topo, p: &EdgeParams) -> Vec<Found> {
    let t0 = std::time::Instant::now();
    let (label, planes) = flat_faces(m, topo, p);
    let nt = m.tris.len();
    let h = m.edge_median();
    if debug() {
        let covered: f64 = planes.iter().map(|x| x.2).sum();
        eprintln!("edges: {} flat faces, {:.0} mm² ({:.1}s)", planes.len(), covered, t0.elapsed().as_secs_f64());
    }
    // the strip between faces shared out to the nearest, a few rings at most
    let rings = ((p.reach / h).ceil() as u32).max(2);
    let mut owner = label.clone();
    let mut ring = vec![u32::MAX; nt];
    let mut q: VecDeque<u32> = VecDeque::new();
    for t in 0..nt {
        if label[t] != NONE {
            ring[t] = 0;
            q.push_back(t as u32);
        }
    }
    while let Some(t) = q.pop_front() {
        let r = ring[t as usize];
        if r >= rings {
            continue;
        }
        for &u in &topo.adj[t as usize] {
            if u != NONE && owner[u as usize] == NONE {
                owner[u as usize] = owner[t as usize];
                ring[u as usize] = r + 1;
                q.push_back(u);
            }
        }
    }
    // where two faces meet: the points along their border, by pair
    let mut meets: HashMap<(u32, u32), Vec<DVec3>> = HashMap::new();
    for t in 0..nt {
        let a = owner[t];
        if a == NONE {
            continue;
        }
        for &u in &topo.adj[t] {
            if u == NONE || (u as usize) < t {
                continue;
            }
            let b = owner[u as usize];
            if b == NONE || b == a {
                continue;
            }
            let key = (a.min(b), a.max(b));
            meets.entry(key).or_default().push((m.tri_centre(t) + m.tri_centre(u as usize)) * 0.5);
        }
    }
    let cos_min = p.min_angle.to_radians().cos();
    let cos_max = (180.0 - p.min_angle).to_radians().cos();
    let mut out = vec![];
    for ((a, b), pts) in meets {
        let (pa, pb) = (planes[a as usize], planes[b as usize]);
        let d = pa.1.dot(pb.1);
        if d > cos_min || d < cos_max || pts.len() < 4 {
            continue;
        }
        let mid = pts.iter().copied().sum::<DVec3>() / pts.len() as f64;
        let Some((o, e)) = meet((pa.0, pa.1), (pb.0, pb.1), mid) else { continue };
        // the points near the line, along it; runs split at gaps
        let mut ts: Vec<f64> = pts
            .iter()
            .filter(|x| {
                let r = **x - o;
                (r - e * r.dot(e)).length() < p.reach
            })
            .map(|x| (*x - o).dot(e))
            .collect();
        ts.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let gap = (h * 8.0).max(1.0);
        let mut i = 0;
        while i < ts.len() {
            let mut j = i;
            while j + 1 < ts.len() && ts[j + 1] - ts[j] < gap {
                j += 1;
            }
            // enough points along the run (not a few stray contacts)
            if ts[j] - ts[i] >= p.min_len && (j - i + 1) as f64 >= (ts[j] - ts[i]) / (h * 3.0) {
                out.push(Found { a: o + e * ts[i], b: o + e * ts[j], angle: d.clamp(-1.0, 1.0).acos().to_degrees(), faces: (a, b) });
            }
            i = j + 1;
        }
    }
    out.sort_by(|x, y| (y.b - y.a).length().partial_cmp(&(x.b - x.a).length()).unwrap());
    if debug() {
        eprintln!("edges: {} edges found ({:.1}s)", out.len(), t0.elapsed().as_secs_f64());
    }
    out
}

/// Whether a Line fix of an edge found is safe: the sides it fitted turn as
/// the faces found do, it moves no vertex far, and it folds no triangle.
fn check(m: &Mesh, topo: &Topo, f: &Found, fit: &LineFit, max_move: f64) -> Result<(), String> {
    if (fit.angle - f.angle).abs() > 15.0 {
        return Err(format!("its sides turn {:.0}°, the faces {:.0}°", fit.angle, f.angle));
    }
    let mx = fit.moves.max_move();
    if mx > max_move {
        return Err(format!("it would move {mx:.2} mm"));
    }
    let mut moved: HashMap<u32, DVec3> = HashMap::with_capacity(fit.moves.idx.len());
    for k in 0..fit.moves.idx.len() {
        moved.insert(fit.moves.idx[k], fit.moves.to[k]);
    }
    let mut tris: Vec<u32> = fit.moves.idx.iter().flat_map(|&v| topo.tris_of(v as usize).iter().copied()).collect();
    tris.sort_unstable();
    tris.dedup();
    let pos = |v: u32| moved.get(&v).copied().unwrap_or(m.pos[v as usize]);
    let folded = tris
        .iter()
        .filter(|&&t| {
            let [a, b, c] = m.tris[t as usize];
            let n0 = m.tri_cross(t as usize).normalize_or_zero();
            let n1 = (pos(b) - pos(a)).cross(pos(c) - pos(a)).normalize_or_zero();
            n0.dot(n1) < 75f64.to_radians().cos()
        })
        .count();
    if folded as f64 > tris.len() as f64 * 0.003 + 2.0 {
        return Err(format!("it would fold {folded} triangles"));
    }
    Ok(())
}

/// The edges switched on fixed like Lines, one after the other on the mesh
/// as the ones before left it (longest first; where two meet at a corner the
/// second sees the first's sides already flat). Each fix is checked; one that
/// fails is left out. Returns each edge's fix (None: off or left out, with
/// why) and all the moves together.
pub fn fix_all(m: &Mesh, topo: &Topo, found: &[Found], on: &[bool], lp: &LineParams) -> (Vec<Result<LineFit, String>>, crate::moves::Moves) {
    let h = m.edge_median();
    let grid = Grid::new(&m.pos, h * 4.0);
    let mut work = m.clone();
    let max_move = (lp.width * 0.6).max(0.6);
    let mut out = Vec::with_capacity(found.len());
    for (i, f) in found.iter().enumerate() {
        if !on.get(i).copied().unwrap_or(true) {
            out.push(Err("off".to_string()));
            continue;
        }
        let seeds: Vec<u32> = [f.a, f.b, (f.a + f.b) * 0.5].iter().filter_map(|&x| grid.nearest(&work.pos, x, lp.width * 3.0)).collect();
        if seeds.len() < 2 {
            out.push(Err("off the mesh".into()));
            continue;
        }
        let r = line::fix(&work, topo, &seeds, f.a, f.b, h, lp).and_then(|fit| check(&work, topo, f, &fit, max_move).map(|_| fit));
        if let Ok(fit) = &r {
            fit.moves.apply(&mut work);
        }
        out.push(r);
    }
    let moved: Vec<(u32, DVec3)> = (0..m.pos.len() as u32).into_par_iter().filter(|&v| work.pos[v as usize] != m.pos[v as usize]).map(|v| (v, work.pos[v as usize])).collect();
    (out, crate::moves::Moves::new(m, moved))
}
