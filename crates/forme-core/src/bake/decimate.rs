//! Optional second pass on a baked mesh (R&D, off by default): dissolve the
//! closed edge loops that add no volume.
//!
//! A loop may go when, at each of its vertices, the path across it (the
//! neighbour on one side, the vertex, the neighbour on the other side) leaves
//! the surface by less than the threshold. Only the slope out of the surface
//! counts, not the bends inside a flat face. The two quads on both sides of
//! each loop edge then merge into one and the loop's vertices disappear.
//! Loops that run into a pole, a triangle, an n-gon or an open edge stay
//! whole: a loop is removed entirely or not at all.

use super::field::Field;
use super::{finish, BFace, BakeSettings, BakeStats, Baked, Progress};
use crate::program::compile;
use crate::scene::Scene;
use glam::DVec3;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// Default threshold (degrees). Between 1 and 7 degrees the test part gives
/// the same result: the loops removed are exactly flat, the next ones follow
/// a 7.5 degree cylinder.
pub const FLAT_LOOPS_DEG: f64 = 5.0;

/// `b` with its flat loops dissolved, stats recomputed. `stats.flat_loops`
/// counts the loops removed and `stats.faces_before_flat` the faces before.
pub fn dissolve_flat_loops(scene: &Scene, b: &Baked, max_deg: f64, threads: usize) -> Baked {
    let t0 = Instant::now();
    let field = Field::new(&compile(scene));
    let (mut out, loops) = dissolve(&field, b, max_deg);
    let s = &b.stats;
    out.stats = BakeStats { patches: s.patches, seconds: s.seconds, quad_size: s.quad_size, ..Default::default() };
    let bs = BakeSettings { tol: 0.01, threads, divisions: 0.0, bevel: 0.0 };
    finish(&field, &mut out, &bs, &Progress::default());
    out.stats.flat_loops = loops.len();
    out.stats.faces_before_flat = b.faces.len();
    out.stats.flat_seconds = t0.elapsed().as_secs_f64();
    out
}

struct Candidate {
    worst: f64,
    faces: Vec<usize>,
    merged: Vec<([u32; 4], usize)>,
    ring: Vec<u32>,
}

/// The pass on a field already built: the mesh (stats left empty) and the
/// removed loops as positions, in the order they went.
pub fn dissolve(field: &Field, b: &Baked, max_deg: f64) -> (Baked, Vec<Vec<DVec3>>) {
    let pos = &b.positions;
    let nrm: Vec<DVec3> = pos.iter().map(|p| field.grad(*p).normalize_or_zero()).collect();
    let mut faces: Vec<Option<BFace>> = b.faces.iter().cloned().map(Some).collect();
    let mut removed = vec![];
    loop {
        // adjacency: oriented quad edges, and every neighbour of each vertex
        let mut dir: HashMap<(u32, u32), usize> = HashMap::new();
        let mut nb: Vec<Vec<u32>> = vec![vec![]; pos.len()];
        for (fi, f) in faces.iter().enumerate() {
            let Some(f) = f else { continue };
            let (v, m) = (&f.verts, f.verts.len());
            for k in 0..m {
                let (a, c) = (v[k], v[(k + 1) % m]);
                if m == 4 {
                    dir.insert((a, c), fi);
                }
                for (x, y) in [(a, c), (c, a)] {
                    if !nb[x as usize].contains(&y) {
                        nb[x as usize].push(y);
                    }
                }
            }
        }
        let quad = |fi: usize| -> [u32; 4] {
            let v = &faces[fi].as_ref().unwrap().verts;
            [v[0], v[1], v[2], v[3]]
        };
        // the edge after (x, y), straight across y (four quads around it)
        let opposite = |x: u32, y: u32| -> Option<u32> {
            if nb[y as usize].len() != 4 {
                return None;
            }
            let (f1, f2) = (quad(*dir.get(&(x, y))?), quad(*dir.get(&(y, x))?));
            let k1 = f1.iter().position(|&v| v == y)?;
            let k2 = f2.iter().position(|&v| v == y)?;
            let (c1, c2) = (f1[(k1 + 1) % 4], f2[(k2 + 3) % 4]);
            let rest: Vec<u32> = nb[y as usize].iter().copied().filter(|&v| v != x && v != c1 && v != c2).collect();
            (rest.len() == 1).then(|| rest[0])
        };
        let mut keys: Vec<(u32, u32)> = dir.keys().copied().collect();
        keys.sort_unstable();
        let mut seen: HashSet<(u32, u32)> = HashSet::new();
        let mut cands: Vec<Candidate> = vec![];
        for (a, c) in keys {
            if seen.contains(&(a.min(c), a.max(c))) {
                continue;
            }
            let mut ring = vec![a, c];
            let mut inside: HashSet<u32> = [a, c].into_iter().collect();
            let mut closed = false;
            loop {
                let (x, y) = (ring[ring.len() - 2], ring[ring.len() - 1]);
                seen.insert((x.min(y), x.max(y)));
                let Some(z) = opposite(x, y) else { break };
                if z == a {
                    seen.insert((y.min(a), y.max(a)));
                    closed = opposite(y, a) == Some(c);
                    break;
                }
                if !inside.insert(z) {
                    break;
                }
                ring.push(z);
            }
            if !closed {
                continue;
            }
            let n = ring.len();
            // how far the path across the loop leaves the surface
            let mut worst = 0.0f64;
            for i in 0..n {
                let (p, v, q) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
                let cross: Vec<u32> = nb[v as usize].iter().copied().filter(|&w| w != p && w != q).collect();
                let (pv, nv) = (pos[v as usize], nrm[v as usize]);
                let e1 = (pv - pos[cross[0] as usize]).normalize_or_zero();
                let e2 = (pos[cross[1] as usize] - pv).normalize_or_zero();
                let bend = (e1.dot(nv).clamp(-1.0, 1.0).asin() - e2.dot(nv).clamp(-1.0, 1.0).asin()).abs().to_degrees();
                worst = worst.max(bend);
                if worst >= max_deg {
                    break;
                }
            }
            if worst >= max_deg {
                continue;
            }
            // both quads of each loop edge become one
            let mut used = vec![];
            let mut merged = vec![];
            let mut ok = true;
            for i in 0..n {
                let (u, w) = (ring[i], ring[(i + 1) % n]);
                let (Some(&f1), Some(&f2)) = (dir.get(&(u, w)), dir.get(&(w, u))) else {
                    ok = false;
                    break;
                };
                if used.contains(&f1) || used.contains(&f2) {
                    ok = false;
                    break;
                }
                used.push(f1);
                used.push(f2);
                let rot = |f: [u32; 4], s: u32| {
                    let k = f.iter().position(|&v| v == s).unwrap();
                    [f[k], f[(k + 1) % 4], f[(k + 2) % 4], f[(k + 3) % 4]]
                };
                let (a1, a2) = (rot(quad(f1), u), rot(quad(f2), w));
                let q = [a1[2], a1[3], a2[2], a2[3]];
                let distinct = (0..4).all(|i| (i + 1..4).all(|j| q[i] != q[j]));
                // no edge that exists already elsewhere, convex in its plane
                let fresh = !nb[a1[3] as usize].contains(&a2[2]) && !nb[a2[3] as usize].contains(&a1[2]);
                let pts: Vec<DVec3> = q.iter().map(|v| pos[*v as usize]).collect();
                let cn = field.grad((pts[0] + pts[1] + pts[2] + pts[3]) * 0.25);
                let turns: Vec<f64> = (0..4).map(|k| (pts[(k + 1) % 4] - pts[k]).cross(pts[(k + 2) % 4] - pts[(k + 1) % 4]).dot(cn)).collect();
                let convex = turns.iter().all(|t| *t > 0.0) || turns.iter().all(|t| *t < 0.0);
                if !distinct || !fresh || !convex {
                    ok = false;
                    break;
                }
                merged.push((q, f1));
            }
            if ok {
                cands.push(Candidate { worst, faces: used, merged, ring });
            }
        }
        if cands.is_empty() {
            break;
        }
        // the flattest first; a loop sharing a face with one already removed
        // this round waits for the next round (its neighbours changed)
        cands.sort_by(|x, y| x.worst.total_cmp(&y.worst).then(x.ring[0].cmp(&y.ring[0])));
        let mut touched: HashSet<usize> = HashSet::new();
        for c in cands {
            if c.faces.iter().any(|f| touched.contains(f)) {
                continue;
            }
            touched.extend(c.faces.iter().copied());
            removed.push(c.ring.iter().map(|v| pos[*v as usize]).collect());
            let new: Vec<BFace> = c
                .merged
                .iter()
                .map(|(q, src)| {
                    let f = faces[*src].as_ref().unwrap();
                    BFace { verts: q.to_vec(), normals: vec![], mat: f.mat, patch: f.patch }
                })
                .collect();
            for fi in &c.faces {
                faces[*fi] = None;
            }
            faces.extend(new.into_iter().map(Some));
        }
    }
    // drop the vertices nobody uses any more
    let mut out = Baked::default();
    let mut remap = vec![u32::MAX; pos.len()];
    for mut f in faces.into_iter().flatten() {
        for v in f.verts.iter_mut() {
            if remap[*v as usize] == u32::MAX {
                remap[*v as usize] = out.positions.len() as u32;
                out.positions.push(pos[*v as usize]);
            }
            *v = remap[*v as usize];
        }
        f.normals = f.verts.iter().map(|v| nrm[*v as usize]).collect();
        out.faces.push(f);
    }
    (out, removed)
}
