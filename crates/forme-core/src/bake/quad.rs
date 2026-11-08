//! Quad Bake: an all-quad, uniform mesh whose edge loops follow the sharp
//! edges and the curvature (rows along fillets, rings around cylinders),
//! every vertex lying on the exact surface.
//!
//! Field-aligned remeshing in the spirit of Instant Meshes (Jakob et al. 2015):
//! a 4-fold orientation field and a lattice position field are smoothed on a
//! fine helper mesh of the exact surface, with sharp edges as hard
//! constraints and the principal curvature as a soft one; the lattice is then
//! extracted at twice the target size and split once into pure quads.

use super::field::Field;
use super::{Baked, BakeStats, BFace, Progress};
use crate::mesh;
use crate::program::compile;
use crate::scene::Scene;
use glam::{DVec3, IVec2, Vec3};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct QuadSettings {
    /// Target quad edge length (mm).
    pub size: f64,
    /// Dihedral angle above which an edge is a sharp edge (degrees).
    pub crease_deg: f64,
    pub threads: usize,
}

// ---------------------------------------------------------------- helpers

fn compat_orient(q0: DVec3, n0: DVec3, q1: DVec3, n1: DVec3) -> (DVec3, DVec3) {
    let a = [q0, n0.cross(q0)];
    let b = [q1, n1.cross(q1)];
    let mut best = (-1.0, 0, 0);
    for (i, ai) in a.iter().enumerate() {
        for (j, bj) in b.iter().enumerate() {
            let s = ai.dot(*bj).abs();
            if s > best.0 + 1e-9 {
                best = (s, i, j);
            }
        }
    }
    let dp = a[best.1].dot(b[best.2]);
    (a[best.1], b[best.2] * if dp >= 0.0 { 1.0 } else { -1.0 })
}

fn middle_point(p0: DVec3, n0: DVec3, p1: DVec3, n1: DVec3) -> DVec3 {
    let (n0p0, n0p1, n1p0, n1p1, n0n1) = (n0.dot(p0), n0.dot(p1), n1.dot(p0), n1.dot(p1), n0.dot(n1));
    let denom = 1.0 / (1.0 - n0n1 * n0n1 + 1e-4);
    let l0 = 2.0 * (n0p1 - n0p0 - n0n1 * (n1p0 - n1p1)) * denom;
    let l1 = 2.0 * (n1p0 - n1p1 - n0n1 * (n0p1 - n0p0)) * denom;
    0.5 * (p0 + p1) - 0.25 * (n0 * l0 + n1 * l1)
}

fn lattice_floor(o: DVec3, q: DVec3, n: DVec3, p: DVec3, s: f64) -> IVec2 {
    let t = n.cross(q);
    let d = p - o;
    IVec2::new((q.dot(d) / s).floor() as i32, (t.dot(d) / s).floor() as i32)
}

fn lattice_point(o: DVec3, q: DVec3, n: DVec3, k: IVec2, s: f64) -> DVec3 {
    o + (q * k.x as f64 + n.cross(q) * k.y as f64) * s
}

fn lattice_round(o: DVec3, q: DVec3, n: DVec3, p: DVec3, s: f64) -> DVec3 {
    let t = n.cross(q);
    let d = p - o;
    o + q * ((q.dot(d) / s).round() * s) + t * ((t.dot(d) / s).round() * s)
}

/// Closest pair of lattice points of i and j near their middle (indices relative to each origin).
#[allow(clippy::too_many_arguments)]
fn compat_pos_index(x0: DVec3, n0: DVec3, q0: DVec3, o0: DVec3, x1: DVec3, n1: DVec3, q1: DVec3, o1: DVec3, s: f64) -> (IVec2, IVec2, DVec3, DVec3) {
    let m = middle_point(x0, n0, x1, n1);
    let b0 = lattice_floor(o0, q0, n0, m, s);
    let b1 = lattice_floor(o1, q1, n1, m, s);
    let mut best = (f64::MAX, IVec2::ZERO, IVec2::ZERO, DVec3::ZERO, DVec3::ZERO);
    for i in 0..4 {
        let k0 = b0 + IVec2::new(i & 1, (i & 2) >> 1);
        let p0 = lattice_point(o0, q0, n0, k0, s);
        for j in 0..4 {
            let k1 = b1 + IVec2::new(j & 1, (j & 2) >> 1);
            let p1 = lattice_point(o1, q1, n1, k1, s);
            let c = p0.distance_squared(p1);
            if c < best.0 {
                best = (c, k0, k1, p0, p1);
            }
        }
    }
    (best.1, best.2, best.3, best.4)
}

#[derive(Clone, Copy)]
enum Cons {
    Free,
    /// A lattice line must run along `t` through `p` (sharp edge, or the
    /// middle of a fillet between two shapes).
    Line(DVec3, DVec3),
    /// A lattice point must sit at `p` (junction of several curves).
    Corner(DVec3),
    /// Only the orientation follows `t` (curves cut in afterwards).
    Dir(DVec3),
}

struct Level {
    x: Vec<DVec3>,
    n: Vec<DVec3>,
    a: Vec<f64>,
    start: Vec<usize>,
    adj: Vec<u32>,
    cons: Vec<Cons>,
    /// Max curvature direction and strength (soft alignment).
    curv: Vec<(DVec3, f64)>,
    /// Parent in the next coarser level.
    parent: Vec<u32>,
}

impl Level {
    fn len(&self) -> usize {
        self.x.len()
    }
    fn nb(&self, i: usize) -> &[u32] {
        &self.adj[self.start[i]..self.start[i + 1]]
    }
}

fn csr(n: usize, edges: &[(u32, u32)]) -> (Vec<usize>, Vec<u32>) {
    let mut lists: Vec<Vec<u32>> = vec![vec![]; n];
    for &(a, b) in edges {
        if a != b {
            lists[a as usize].push(b);
            lists[b as usize].push(a);
        }
    }
    let mut start = vec![0usize; n + 1];
    let mut adj = vec![];
    for (i, l) in lists.iter_mut().enumerate() {
        l.sort_unstable();
        l.dedup();
        adj.extend_from_slice(l);
        start[i + 1] = adj.len();
    }
    (start, adj)
}

fn coarsen(f: &mut Level) -> Option<Level> {
    let n = f.len();
    let mut pairs: Vec<(f64, u32, u32)> = vec![];
    for i in 0..n {
        for &j in f.nb(i) {
            if (j as usize) > i {
                let dp = f.n[i].dot(f.n[j as usize]);
                pairs.push((dp / (f.a[i] + f.a[j as usize]).max(1e-12), i as u32, j));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let mut parent = vec![u32::MAX; n];
    let mut count = 0u32;
    for (_, i, j) in pairs {
        if parent[i as usize] == u32::MAX && parent[j as usize] == u32::MAX {
            parent[i as usize] = count;
            parent[j as usize] = count;
            count += 1;
        }
    }
    for p in parent.iter_mut() {
        if *p == u32::MAX {
            *p = count;
            count += 1;
        }
    }
    let m = count as usize;
    if m as f64 > n as f64 * 0.92 || m < 16 {
        return None;
    }
    let mut x = vec![DVec3::ZERO; m];
    let mut nn = vec![DVec3::ZERO; m];
    let mut a = vec![0.0; m];
    let mut cons = vec![Cons::Free; m];
    let mut curv = vec![(DVec3::ZERO, 0.0f64); m];
    for i in 0..n {
        let p = parent[i] as usize;
        x[p] += f.x[i] * f.a[i];
        nn[p] += f.n[i] * f.a[i];
        a[p] += f.a[i];
        cons[p] = match (cons[p], f.cons[i]) {
            (Cons::Corner(q), _) => Cons::Corner(q),
            (_, Cons::Corner(q)) => Cons::Corner(q),
            (Cons::Line(a, t), _) => Cons::Line(a, t),
            (Cons::Dir(t), Cons::Free) => Cons::Dir(t),
            (_, c) => c,
        };
        if f.curv[i].1 > curv[p].1 {
            curv[p] = f.curv[i];
        }
    }
    for p in 0..m {
        x[p] /= a[p].max(1e-12);
        nn[p] = nn[p].normalize_or(DVec3::Y);
    }
    let mut edges = vec![];
    for i in 0..n {
        for &j in f.nb(i) {
            let (pa, pb) = (parent[i], parent[j as usize]);
            if pa < pb {
                edges.push((pa, pb));
            }
        }
    }
    let (start, adj) = csr(m, &edges);
    f.parent = parent;
    Some(Level { x, n: nn, a, start, adj, cons, curv, parent: vec![] })
}

fn smooth_orient(l: &Level, q: &mut [DVec3], iters: usize) {
    for _ in 0..iters {
        for i in 0..l.len() {
            let ni = l.n[i];
            if let Cons::Line(_, t) | Cons::Dir(t) = l.cons[i] {
                q[i] = (t - ni * ni.dot(t)).normalize_or(q[i]);
                continue;
            }
            let mut sum = q[i];
            let mut wsum = 0.0;
            for &j in l.nb(i) {
                let j = j as usize;
                let w = 1.0;
                let (a, b) = compat_orient(sum, ni, q[j], l.n[j]);
                sum = a * wsum + b * w;
                sum -= ni * ni.dot(sum);
                wsum += w;
                sum = sum.normalize_or(a);
            }
            let (d, k) = l.curv[i];
            if k > 0.05 {
                let w = 3.0 * k;
                let (a, b) = compat_orient(sum, ni, d, ni);
                sum = a * wsum + b * w;
                sum -= ni * ni.dot(sum);
                sum = sum.normalize_or(a);
            }
            q[i] = sum;
        }
    }
}

fn smooth_pos(l: &Level, q: &[DVec3], o: &mut [DVec3], s: f64, iters: usize) {
    for _ in 0..iters {
        for i in 0..l.len() {
            let (xi, ni, qi) = (l.x[i], l.n[i], q[i]);
            if let Cons::Corner(p) = l.cons[i] {
                o[i] = p - ni * ni.dot(p - xi);
                continue;
            }
            let mut sum = o[i];
            let mut wsum = 0.0;
            for &j in l.nb(i) {
                let j = j as usize;
                let w = 1.0;
                let (_, _, p0, p1) = compat_pos_index(xi, ni, qi, sum, l.x[j], l.n[j], q[j], o[j], s);
                // move our lattice so that p0 lands on p1
                let target = sum + (p1 - p0);
                sum = (sum * wsum + target * w) / (wsum + w);
                wsum += w;
                sum -= ni * ni.dot(sum - xi);
            }
            let mut r = lattice_round(sum, qi, ni, xi, s);
            if let Cons::Line(p, t) = l.cons[i] {
                let t = (t - ni * ni.dot(t)).normalize_or(qi);
                let p = p - ni * ni.dot(p - xi);
                r = p + t * t.dot(r - p);
            }
            o[i] = r;
        }
    }
}

struct Uf(Vec<u32>);
impl Uf {
    fn find(&mut self, a: u32) -> u32 {
        let mut r = a;
        while self.0[r as usize] != r {
            r = self.0[r as usize];
        }
        let mut c = a;
        while self.0[c as usize] != r {
            let nx = self.0[c as usize];
            self.0[c as usize] = r;
            c = nx;
        }
        r
    }
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb) as usize] = ra.min(rb);
        }
    }
}

/// Runs the quad bake. None if cancelled.
pub fn quad_bake(scene: &Scene, st: &QuadSettings, progress: &Progress) -> Option<Baked> {
    let t0 = Instant::now();
    let mut stage_t = [0.0f64; 5];
    let set = |stage: u32, total: u32| {
        progress.stage.store(stage, Ordering::Relaxed);
        progress.done.store(0, Ordering::Relaxed);
        progress.total.store(total, Ordering::Relaxed);
    };
    set(0, 1);
    let prog = compile(scene);
    let field = Field::new(&prog);
    if field.is_empty() {
        return Some(Baked::default());
    }
    let s = st.size * 2.0; // extraction scale, split once afterwards
    // 1. fine helper mesh of the exact surface
    let (lo, hi) = scene.bounds();
    let cell = (st.size * 0.55) as f32;
    let dc = mesh::dual_contour(&prog, lo - Vec3::splat(cell * 2.0), hi + Vec3::splat(cell * 2.0), cell);
    let nv = dc.positions.len();
    let mut x: Vec<DVec3> = dc.positions.iter().map(|p| p.as_dvec3()).collect();
    for p in x.iter_mut() {
        if let Some(q) = field.project(*p) {
            if q.distance(*p) < cell as f64 {
                *p = q;
            }
        }
    }
    let tris: Vec<[u32; 3]> = dc.indices.chunks(3).map(|t| [t[0], t[1], t[2]]).filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]).collect();
    stage_t[0] = t0.elapsed().as_secs_f64();
    if progress.cancel.load(Ordering::Relaxed) {
        return None;
    }
    set(1, 3);
    let t1 = Instant::now();
    // face normals, areas, vertex normals
    let fnrm: Vec<DVec3> = tris.iter().map(|t| (x[t[1] as usize] - x[t[0] as usize]).cross(x[t[2] as usize] - x[t[0] as usize])).collect();
    let mut area = vec![0.0f64; nv];
    let mut vn = vec![DVec3::ZERO; nv];
    for (t, fnm) in tris.iter().zip(&fnrm) {
        for &v in t {
            area[v as usize] += fnm.length() / 6.0;
            vn[v as usize] += *fnm;
        }
    }
    // sharp edges from the dihedral angle
    let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (fi, t) in tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            edge_faces.entry((a.min(b), a.max(b))).or_default().push(fi);
        }
    }
    let cos_c = st.crease_deg.to_radians().cos();
    let mut crease_nb: Vec<Vec<u32>> = vec![vec![]; nv];
    for (&(a, b), fs) in &edge_faces {
        let sharp = fs.len() != 2 || fnrm[fs[0]].normalize_or_zero().dot(fnrm[fs[1]].normalize_or_zero()) < cos_c;
        if sharp {
            crease_nb[a as usize].push(b);
            crease_nb[b as usize].push(a);
        }
    }
    let edges: Vec<(u32, u32)> = edge_faces.keys().copied().collect();
    let (start, adj) = csr(nv, &edges);
    let mut n: Vec<DVec3> = vec![DVec3::Y; nv];
    let mut cons = vec![Cons::Free; nv];
    for i in 0..nv {
        let g = field.grad(x[i]).normalize_or_zero();
        let avg = vn[i].normalize_or(g);
        n[i] = if crease_nb[i].is_empty() && g != DVec3::ZERO { g } else { avg };
        cons[i] = match crease_nb[i].len() {
            0 => Cons::Free,
            2 => Cons::Line(x[i], (x[crease_nb[i][1] as usize] - x[crease_nb[i][0] as usize]).normalize_or(DVec3::X)),
            _ => Cons::Corner(x[i]),
        };
    }
    // curves where the surface passes from one shape to another (fillet
    // middles, sharp booleans) and where fillets start: the field runs along
    // them; they are cut in exactly once the quads exist
    let samples: Vec<super::field::Sample> = x.iter().map(|p| field.eval(*p)).collect();
    let side_key = |sm: &super::field::Sample| (sm.owner.key(), sm.blend > 1e-7);
    let mut dirs: Vec<DVec3> = vec![DVec3::ZERO; nv];
    for &(a, b) in &edges {
        let (a, b) = (a as usize, b as usize);
        if side_key(&samples[a]) != side_key(&samples[b]) {
            let d = x[b] - x[a];
            dirs[a] += d;
            dirs[b] -= d;
        }
    }
    for i in 0..nv {
        if dirs[i] != DVec3::ZERO && matches!(cons[i], Cons::Free) {
            cons[i] = Cons::Dir(n[i].cross(dirs[i]).normalize_or(DVec3::X));
        }
    }
    // principal (max) curvature direction from the spread of neighbour normals
    let mut curv = vec![(DVec3::ZERO, 0.0f64); nv];
    for i in 0..nv {
        if !matches!(cons[i], Cons::Free) {
            continue;
        }
        let ni = n[i];
        let u = ni.any_orthonormal_vector();
        let w = ni.cross(u);
        let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
        let mut count = 0.0;
        for &j in &adj[start[i]..start[i + 1]] {
            let d = x[j as usize] - x[i];
            let len2 = d.length_squared().max(1e-18);
            let dn = n[j as usize] - ni;
            // curvature along the edge direction
            let k = dn.dot(d) / len2;
            let dir = (d - ni * ni.dot(d)).normalize_or_zero();
            let (cx, cy) = (dir.dot(u), dir.dot(w));
            sxx += k * cx * cx;
            sxy += k * cx * cy;
            syy += k * cy * cy;
            count += 1.0;
        }
        if count < 3.0 {
            continue;
        }
        let (a, b, c) = (sxx / count, sxy / count, syy / count);
        // eigen decomposition of [[a,b],[b,c]]; the cross field does not care
        // which of the two (orthogonal) principal directions we keep
        let tr = a + c;
        let disc = ((a - c) * (a - c) * 0.25 + b * b).sqrt();
        let (l1, l2) = (tr * 0.5 + disc, tr * 0.5 - disc);
        let ang = 0.5 * (2.0 * b).atan2(a - c);
        let dir = u * ang.cos() + w * ang.sin();
        let (big, small) = if l1.abs() >= l2.abs() { (l1.abs(), l2.abs()) } else { (l2.abs(), l1.abs()) };
        let aniso = ((big - small) / (big + small + 1e-9)) * (big * st.size * 4.0).min(1.0);
        curv[i] = (dir, aniso);
    }
    // 2. hierarchy
    let mut levels = vec![Level { x: x.clone(), n: n.clone(), a: area, start, adj, cons, curv, parent: vec![] }];
    loop {
        let last = levels.last_mut().unwrap();
        match coarsen(last) {
            Some(l) => levels.push(l),
            None => break,
        }
        if levels.len() > 40 {
            break;
        }
    }
    progress.done.store(1, Ordering::Relaxed);
    // 3. orientation field, coarse to fine
    let nl = levels.len();
    let mut q: Vec<DVec3> = levels[nl - 1].n.iter().map(|n| n.any_orthonormal_vector()).collect();
    smooth_orient(&levels[nl - 1], &mut q, 50);
    for li in (0..nl - 1).rev() {
        let l = &levels[li];
        let mut qf: Vec<DVec3> = (0..l.len())
            .map(|i| {
                let p = q[l.parent[i] as usize];
                (p - l.n[i] * l.n[i].dot(p)).normalize_or(l.n[i].any_orthonormal_vector())
            })
            .collect();
        smooth_orient(l, &mut qf, if li == 0 { 30 } else { 12 });
        q = qf;
    }
    let qs: Vec<Vec<DVec3>> = {
        // orientations at every level (restriction by parent averaging of the fine solution)
        let mut v = vec![q.clone()];
        for li in 0..nl - 1 {
            let l = &levels[li];
            let coarse = &levels[li + 1];
            let mut acc = vec![DVec3::ZERO; coarse.len()];
            for i in 0..l.len() {
                let p = l.parent[i] as usize;
                let cur = acc[p];
                let (a, b) = if cur == DVec3::ZERO { (v[li][i], v[li][i]) } else { compat_orient(cur, coarse.n[p], v[li][i], l.n[i]) };
                acc[p] = (a + b).normalize_or(a);
            }
            for (p, a) in acc.iter_mut().enumerate() {
                let nn = coarse.n[p];
                *a = (*a - nn * nn.dot(*a)).normalize_or(nn.any_orthonormal_vector());
            }
            v.push(acc);
        }
        v
    };
    progress.done.store(2, Ordering::Relaxed);
    // 4. position field, coarse to fine
    let mut o: Vec<DVec3> = levels[nl - 1].x.clone();
    smooth_pos(&levels[nl - 1], &qs[nl - 1], &mut o, s, 50);
    for li in (0..nl - 1).rev() {
        let l = &levels[li];
        let mut of: Vec<DVec3> = (0..l.len())
            .map(|i| {
                let p = o[l.parent[i] as usize];
                p - l.n[i] * l.n[i].dot(p - l.x[i])
            })
            .collect();
        smooth_pos(l, &qs[li], &mut of, s, if li == 0 { 40 } else { 12 });
        o = of;
    }
    stage_t[1] = t1.elapsed().as_secs_f64();
    if progress.cancel.load(Ordering::Relaxed) {
        return None;
    }
    // 5. extraction
    set(2, 1);
    let t2 = Instant::now();
    let l0 = &levels[0];
    let q0 = &qs[0];
    let mut uf = Uf((0..nv as u32).collect());
    let mut links: Vec<(u32, u32)> = vec![];
    for i in 0..nv {
        for &j in l0.nb(i) {
            let j = j as usize;
            if j < i {
                continue;
            }
            let (_, qj) = compat_orient(q0[i], l0.n[i], q0[j], l0.n[j]);
            let (ki, kj, _, _) = compat_pos_index(l0.x[i], l0.n[i], q0[i], o[i], l0.x[j], l0.n[j], qj, o[j], s);
            let d = (ki - kj).abs();
            if d.max_element() > 1 || (d.x == 1 && d.y == 1) {
                continue;
            }
            if d.x + d.y == 0 {
                uf.union(i as u32, j as u32);
            } else {
                links.push((i as u32, j as u32));
            }
        }
    }
    let mut cid = vec![u32::MAX; nv];
    let mut cpos: Vec<DVec3> = vec![];
    let mut cnrm: Vec<DVec3> = vec![];
    let mut ccount: Vec<f64> = vec![];
    for i in 0..nv {
        let r = uf.find(i as u32) as usize;
        if cid[r] == u32::MAX {
            cid[r] = cpos.len() as u32;
            cpos.push(DVec3::ZERO);
            cnrm.push(DVec3::ZERO);
            ccount.push(0.0);
        }
        let c = cid[r] as usize;
        cid[i] = cid[r];
        cpos[c] += o[i];
        cnrm[c] += l0.n[i];
        ccount[c] += 1.0;
    }
    for c in 0..cpos.len() {
        cpos[c] /= ccount[c];
        cnrm[c] = cnrm[c].normalize_or(DVec3::Y);
    }
    let mut cadj: HashMap<(u32, u32), u32> = HashMap::new();
    for (i, j) in links {
        let (a, b) = (cid[i as usize], cid[j as usize]);
        if a != b {
            *cadj.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let nc = cpos.len();
    let mut nbs: Vec<Vec<u32>> = vec![vec![]; nc];
    for (&(a, b), _) in &cadj {
        nbs[a as usize].push(b);
        nbs[b as usize].push(a);
    }
    // sort neighbours counter-clockwise around the normal
    for v in 0..nc {
        let nn = cnrm[v];
        let u = nn.any_orthonormal_vector();
        let w = nn.cross(u);
        let p = cpos[v];
        nbs[v].sort_by(|&a, &b| {
            let da = cpos[a as usize] - p;
            let db = cpos[b as usize] - p;
            let aa = da.dot(w).atan2(da.dot(u));
            let ab = db.dot(w).atan2(db.dot(u));
            aa.partial_cmp(&ab).unwrap()
        });
    }
    let mut used: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    let mut faces: Vec<Vec<u32>> = vec![];
    for a in 0..nc as u32 {
        for &b in &nbs[a as usize].clone() {
            if used.contains(&(a, b)) {
                continue;
            }
            let mut face = vec![a, b];
            let (mut prev, mut cur) = (a, b);
            let mut ok = false;
            for _ in 0..6 {
                let l = &nbs[cur as usize];
                let Some(idx) = l.iter().position(|&x| x == prev) else { break };
                let nxt = l[(idx + l.len() - 1) % l.len()];
                if nxt == a {
                    ok = true;
                    break;
                }
                face.push(nxt);
                prev = cur;
                cur = nxt;
            }
            if ok && face.len() >= 3 && face.len() <= 6 {
                for k in 0..face.len() {
                    used.insert((face[k], face[(k + 1) % face.len()]));
                }
                faces.push(face);
            }
        }
    }
    // 6. split every polygon into quads (pure quad mesh at the target size)
    let mut pos = cpos;
    let mut mids: HashMap<(u32, u32), u32> = HashMap::new();
    let mut quads: Vec<Vec<u32>> = vec![];
    for f in &faces {
        let m = f.len();
        let c = f.iter().fold(DVec3::ZERO, |acc, v| acc + pos[*v as usize]) / m as f64;
        pos.push(c);
        let ci = pos.len() as u32 - 1;
        let mid: Vec<u32> = (0..m)
            .map(|k| {
                let (a, b) = (f[k], f[(k + 1) % m]);
                *mids.entry((a.min(b), a.max(b))).or_insert_with(|| {
                    pos.push((pos[a as usize] + pos[b as usize]) * 0.5);
                    pos.len() as u32 - 1
                })
            })
            .collect();
        for k in 0..m {
            quads.push(vec![f[k], mid[k], ci, mid[(k + m - 1) % m]]);
        }
    }
    stage_t[2] = t2.elapsed().as_secs_f64();
    // 7. onto the exact surface, then relax the smooth parts tangentially
    set(3, 1);
    let t3 = Instant::now();
    for p in pos.iter_mut() {
        if let Some(q) = field.project(*p) {
            *p = q;
        }
    }
    // cut the exact curves in: shape-to-shape curves, then fillet rails
    let mut pinned = vec![false; pos.len()];
    knife(&field, &mut pos, &mut quads, &mut pinned, |sm| {
        let k = sm.owner.key();
        (k.prim as u64) << 40 ^ (k.copy as u64) << 20 ^ (k.level as u64)
    });
    knife(&field, &mut pos, &mut quads, &mut pinned, |sm| (sm.blend > 1e-7) as u64);
    let mut vnb: Vec<Vec<u32>> = vec![vec![]; pos.len()];
    for f in &quads {
        for k in 0..f.len() {
            let (a, b) = (f[k], f[(k + 1) % f.len()]);
            vnb[a as usize].push(b);
            vnb[b as usize].push(a);
        }
    }
    for l in vnb.iter_mut() {
        l.sort_unstable();
        l.dedup();
    }
    for _ in 0..3 {
        let snapshot = pos.clone();
        for v in 0..pos.len() {
            if vnb[v].len() < 3 || pinned[v] {
                continue;
            }
            let nv0 = field.grad(snapshot[v]).normalize_or_zero();
            // keep vertices on sharp edges where they are
            if vnb[v].iter().any(|&u| field.grad(snapshot[u as usize]).normalize_or_zero().dot(nv0) < 0.8) {
                continue;
            }
            let avg = vnb[v].iter().fold(DVec3::ZERO, |a, &u| a + snapshot[u as usize]) / vnb[v].len() as f64;
            let d = avg - snapshot[v];
            let moved = snapshot[v] + (d - nv0 * nv0.dot(d)) * 0.5;
            if let Some(p) = field.project(moved) {
                pos[v] = p;
            }
        }
    }
    let mut out_faces: Vec<Vec<u32>> = quads;
    // orient along the outward gradient
    for f in out_faces.iter_mut() {
        let mut nn = DVec3::ZERO;
        let mut c = DVec3::ZERO;
        for k in 0..f.len() {
            nn += pos[f[k] as usize].cross(pos[f[(k + 1) % f.len()] as usize]);
            c += pos[f[k] as usize];
        }
        if nn.dot(field.grad(c / f.len() as f64)) < 0.0 {
            f.reverse();
        }
    }
    // close what extraction left open
    let mut fp = vec![0u32; out_faces.len()];
    let loops = super::boundary_loops(&out_faces, &pos, st.size * 3.0);
    let holes = loops.len();
    let mut es: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    for f in &out_faces {
        for k in 0..f.len() {
            let (a, b) = (f[k], f[(k + 1) % f.len()]);
            es.insert((a.min(b), a.max(b)));
        }
    }
    for lp in loops.into_iter().flat_map(super::split_pinched) {
        super::fill_loop(&field, &pos, &lp, &mut out_faces, &mut fp, &mut es);
    }
    stage_t[3] = t3.elapsed().as_secs_f64();
    let mut out = Baked {
        positions: pos,
        faces: out_faces.into_iter().map(|verts| BFace { verts, normals: vec![], mat: 0, patch: 0 }).collect(),
        stats: BakeStats::default(),
    };
    out.stats.holes_filled = holes;
    set(4, out.faces.len() as u32);
    let t4 = Instant::now();
    let bs = super::BakeSettings { tol: 0.01, max_edge: st.size, trim_len: st.size, threads: st.threads, divisions: 0.0 };
    super::finish(&field, &mut out, &bs, progress);
    stage_t[4] = t4.elapsed().as_secs_f64();
    out.stats.seconds = t0.elapsed().as_secs_f64();
    out.stats.stage_seconds = stage_t;
    out.stats.patches = nc;
    Some(out)
}

/// Cuts the curve where `side` changes into the mesh: each crossing edge is
/// split at the exact crossing (or its near end is moved onto it), and every
/// face crossed is split between its two crossing points.
fn knife(field: &Field, pos: &mut Vec<DVec3>, faces: &mut Vec<Vec<u32>>, pinned: &mut Vec<bool>, side: impl Fn(&super::field::Sample) -> u64) {
    let sv: Vec<u64> = pos.iter().map(|p| side(&field.eval(*p))).collect();
    let mut on = vec![false; pos.len()];
    let mut split: HashMap<(u32, u32), u32> = HashMap::new();
    let mut edges: Vec<(u32, u32)> = vec![];
    for f in faces.iter() {
        for k in 0..f.len() {
            let (a, b) = (f[k], f[(k + 1) % f.len()]);
            if a < b {
                edges.push((a, b));
            } else {
                edges.push((b, a));
            }
        }
    }
    edges.sort_unstable();
    edges.dedup();
    for (a, b) in edges {
        let (ai, bi) = (a as usize, b as usize);
        if on[ai] || on[bi] || sv[ai] == sv[bi] {
            continue;
        }
        let (pa, pb) = (pos[ai], pos[bi]);
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        let mut best = pa;
        for _ in 0..26 {
            let t = 0.5 * (lo + hi);
            let p = pa.lerp(pb, t);
            let q = field.project(p).unwrap_or(p);
            if side(&field.eval(q)) == sv[ai] {
                lo = t;
                best = q;
            } else {
                hi = t;
            }
        }
        let t = 0.5 * (lo + hi);
        if t < 0.3 && !pinned[ai] {
            pos[ai] = best;
            pinned[ai] = true;
            on[ai] = true;
        } else if t > 0.7 && !pinned[bi] {
            pos[bi] = best;
            pinned[bi] = true;
            on[bi] = true;
        } else {
            pos.push(best);
            pinned.push(true);
            on.push(true);
            split.insert((a, b), pos.len() as u32 - 1);
        }
    }
    let mut out: Vec<Vec<u32>> = Vec::with_capacity(faces.len() + 64);
    for f in faces.iter() {
        let mut lp = vec![];
        for k in 0..f.len() {
            let (a, b) = (f[k], f[(k + 1) % f.len()]);
            lp.push(a);
            if let Some(&m) = split.get(&(a.min(b), a.max(b))) {
                lp.push(m);
            }
        }
        let cut: Vec<usize> = (0..lp.len()).filter(|&i| on[lp[i] as usize]).collect();
        let m = lp.len();
        if cut.len() == 2 && (cut[1] - cut[0]) != 1 && !(cut[0] == 0 && cut[1] == m - 1) {
            let (i, j) = (cut[0], cut[1]);
            let f1: Vec<u32> = lp[i..=j].to_vec();
            let mut f2: Vec<u32> = lp[j..].to_vec();
            f2.extend_from_slice(&lp[..=i]);
            if f1.len() >= 3 && f2.len() >= 3 {
                out.push(f1);
                out.push(f2);
                continue;
            }
        }
        out.push(lp);
    }
    *faces = out;
}
