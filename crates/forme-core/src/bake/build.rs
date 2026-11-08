//! Constructive bake ("par opération").
//!
//! Instead of meshing the final surface blindly, each step of the history
//! builds its own clean topology from its parameters:
//! - every primitive keeps its native quad mesh (grids, bevel rows,
//!   revolve columns) wherever the other operations do not reach;
//! - every intersection is traced exactly: the corner curve where the two
//!   surfaces meet and, with a fillet, its two rails where the blend starts;
//! - the fillet is made of rows between its rails, like a bevel modifier;
//! - edge loops bridge each native mesh to the rails with graded rings.
//!
//! The result is all quads when the loops match, and every vertex is on
//! the exact surface.

use super::field::{prim_project, prim_raw, Field, Instance, PrimDef, Step};
use super::gen::{self, GenSettings};
use super::{finish, BFace, BakeSettings, Baked, Progress};
use crate::program::compile;
use crate::scene::Scene;
use glam::DVec3;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct BuildSettings {
    /// Target edge length (mm).
    pub size: f64,
    /// Chord tolerance on curved parts (mm).
    pub tol: f64,
    /// Rows across a fillet (0 = automatic).
    pub fillet_rows: usize,
    pub threads: usize,
}

/// Face groups, kept per face (useful to look at the structure).
pub const P_NATIVE: u32 = 0;
pub const P_LOFT: u32 = 1;
pub const P_FILLET: u32 = 2;
pub const P_TOOL_LOFT: u32 = 3;
pub const P_TOOL: u32 = 4;

/// One primitive copy placed in the world.
#[derive(Clone, Copy)]
struct Surf<'a> {
    pr: &'a PrimDef,
    inst: Instance,
}

impl Surf<'_> {
    fn d(&self, p: DVec3) -> f64 {
        prim_raw(self.pr, self.inst.to_local.transform_point3(p)) * self.pr.sc
    }
    fn project(&self, p: DVec3) -> DVec3 {
        let l = prim_project(self.pr, self.inst.to_local.transform_point3(p), 0.0);
        self.inst.to_world.transform_point3(l)
    }
}

fn grad(f: &dyn Fn(DVec3) -> f64, p: DVec3) -> DVec3 {
    let h = 1e-6;
    DVec3::new(
        f(p + DVec3::X * h) - f(p - DVec3::X * h),
        f(p + DVec3::Y * h) - f(p - DVec3::Y * h),
        f(p + DVec3::Z * h) - f(p - DVec3::Z * h),
    ) / (2.0 * h)
}

/// Closest point (Gauss–Newton, minimum-norm steps) on the curve f1 = f2 = 0.
fn solve2(f1: &dyn Fn(DVec3) -> f64, f2: &dyn Fn(DVec3) -> f64, p0: DVec3, cap: f64) -> DVec3 {
    let mut p = p0;
    for _ in 0..60 {
        let (a, b) = (f1(p), f2(p));
        if a.abs() < 1e-11 && b.abs() < 1e-11 {
            break;
        }
        let (g1, g2) = (grad(f1, p), grad(f2, p));
        let (m11, m12, m22) = (g1.dot(g1), g1.dot(g2), g2.dot(g2));
        let det = m11 * m22 - m12 * m12;
        let mut dx = if det > 1e-12 * m11 * m22 {
            let l1 = (m22 * a - m12 * b) / det;
            let l2 = (m11 * b - m12 * a) / det;
            -(g1 * l1 + g2 * l2)
        } else {
            // tangent surfaces: settle on each in turn
            -(g1 * (a / m11.max(1e-12))) - g2 * (b / m22.max(1e-12))
        };
        let l = dx.length();
        if l > cap {
            dx *= cap / l;
        }
        p += dx;
    }
    p
}

#[derive(Default)]
struct Out {
    pos: Vec<DVec3>,
    faces: Vec<Vec<u32>>,
    patch: Vec<u32>,
}

impl Out {
    fn add(&mut self, p: DVec3) -> u32 {
        self.pos.push(p);
        self.pos.len() as u32 - 1
    }
    fn face(&mut self, f: Vec<u32>, patch: u32) {
        self.faces.push(f);
        self.patch.push(patch);
    }
}

/// Native mesh of a primitive, in the world, appended to `out`.
fn native(out: &mut Out, s: &Surf, gs: &GenSettings) -> Vec<Vec<u32>> {
    let m = gen::generate(s.pr, 0.0, gs);
    let base = out.pos.len() as u32;
    for p in &m.pos {
        out.pos.push(s.inst.to_world.transform_point3(*p));
    }
    m.faces.iter().map(|f| f.iter().map(|v| v + base).collect()).collect()
}

/// Splits faces into kept ones (no vertex to `clear`) and connected
/// cleared regions, grown until their outline has no notch (no kept face
/// touching a region by two edges), so that the outline is a clean loop.
fn regions(pos: &[DVec3], faces: &[Vec<u32>], clear: &dyn Fn(DVec3) -> bool, must: &dyn Fn(DVec3) -> bool, allow: &dyn Fn(&[u32]) -> bool) -> (Vec<bool>, Vec<Vec<usize>>) {
    let cl: HashMap<u32, bool> = faces.iter().flatten().map(|&v| (v, clear(pos[v as usize]))).collect();
    let ms: HashMap<u32, bool> = faces.iter().flatten().map(|&v| (v, must(pos[v as usize]))).collect();
    let forced: Vec<bool> = faces.iter().map(|f| f.iter().any(|v| ms[v])).collect();
    let allowed: Vec<bool> = faces.iter().enumerate().map(|(i, f)| forced[i] || allow(f)).collect();
    let mut kept: Vec<bool> = faces.iter().enumerate().map(|(i, f)| !(forced[i] || (allowed[i] && f.iter().any(|v| cl[v])))).collect();
    let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, f) in faces.iter().enumerate() {
        for k in 0..f.len() {
            let (a, b) = (f[k], f[(k + 1) % f.len()]);
            edge_faces.entry((a.min(b), a.max(b))).or_default().push(i);
        }
    }
    let neighbours = |i: usize| -> Vec<usize> {
        let f = &faces[i];
        let mut n = vec![];
        for k in 0..f.len() {
            let (a, b) = (f[k], f[(k + 1) % f.len()]);
            for &j in &edge_faces[&(a.min(b), a.max(b))] {
                if j != i {
                    n.push(j);
                }
            }
        }
        n
    };
    loop {
        let mut changed = false;
        for i in 0..faces.len() {
            if kept[i] && allowed[i] {
                let c = neighbours(i).iter().filter(|&&j| !kept[j]).count();
                if c >= 2 {
                    kept[i] = false;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    // connected components of the cleared faces
    let mut comp = vec![usize::MAX; faces.len()];
    let mut comps: Vec<Vec<usize>> = vec![];
    for s in 0..faces.len() {
        if kept[s] || comp[s] != usize::MAX {
            continue;
        }
        let id = comps.len();
        let mut stack = vec![s];
        comp[s] = id;
        let mut list = vec![];
        while let Some(i) = stack.pop() {
            list.push(i);
            for j in neighbours(i) {
                if !kept[j] && comp[j] == usize::MAX {
                    comp[j] = id;
                    stack.push(j);
                }
            }
        }
        comps.push(list);
    }
    (kept, comps)
}

/// Boundary loops of a set of consistently oriented faces.
fn outline(faces: &[Vec<u32>], set: &[usize]) -> Vec<Vec<u32>> {
    let mut directed: HashSet<(u32, u32)> = HashSet::new();
    for &i in set {
        let f = &faces[i];
        for k in 0..f.len() {
            directed.insert((f[k], f[(k + 1) % f.len()]));
        }
    }
    let mut next: HashMap<u32, u32> = HashMap::new();
    for &(a, b) in &directed {
        if !directed.contains(&(b, a)) {
            next.insert(a, b);
        }
    }
    let mut loops = vec![];
    let mut seen: HashSet<u32> = HashSet::new();
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort();
    for s in starts {
        if seen.contains(&s) {
            continue;
        }
        let mut lp = vec![s];
        seen.insert(s);
        let mut v = next[&s];
        while v != s {
            if !seen.insert(v) {
                break;
            }
            lp.push(v);
            match next.get(&v) {
                Some(&n) => v = n,
                None => break,
            }
        }
        loops.push(lp);
    }
    loops
}

/// Zero-level contour of `f` over a set of faces, as closed polylines.
fn contour(pos: &[DVec3], faces: &[Vec<u32>], set: &[usize], f: &dyn Fn(DVec3) -> f64) -> Vec<Vec<DVec3>> {
    let val: HashMap<u32, f64> = set.iter().flat_map(|&i| faces[i].iter()).map(|&v| (v, f(pos[v as usize]))).collect();
    let mut pts: HashMap<(u32, u32), DVec3> = HashMap::new();
    let mut adj: HashMap<(u32, u32), Vec<(u32, u32)>> = HashMap::new();
    for &i in set {
        let fc = &faces[i];
        let mut cross = vec![];
        for k in 0..fc.len() {
            let (a, b) = (fc[k], fc[(k + 1) % fc.len()]);
            let (fa, fb) = (val[&a], val[&b]);
            if (fa < 0.0) != (fb < 0.0) {
                let key = (a.min(b), a.max(b));
                let t = fa / (fa - fb);
                pts.insert(key, pos[a as usize].lerp(pos[b as usize], t));
                cross.push(key);
            }
        }
        if cross.len() == 2 {
            adj.entry(cross[0]).or_default().push(cross[1]);
            adj.entry(cross[1]).or_default().push(cross[0]);
        } else if cross.len() == 4 {
            // saddle: pair consecutive crossings
            adj.entry(cross[0]).or_default().push(cross[1]);
            adj.entry(cross[1]).or_default().push(cross[0]);
            adj.entry(cross[2]).or_default().push(cross[3]);
            adj.entry(cross[3]).or_default().push(cross[2]);
        }
    }
    let mut loops = vec![];
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    let mut keys: Vec<(u32, u32)> = adj.keys().copied().collect();
    keys.sort();
    for s in keys {
        if seen.contains(&s) {
            continue;
        }
        let mut lp = vec![pts[&s]];
        seen.insert(s);
        let (mut prev, mut cur) = (s, adj[&s][0]);
        let mut closed = false;
        loop {
            if cur == s {
                closed = true;
                break;
            }
            if !seen.insert(cur) {
                break;
            }
            lp.push(pts[&cur]);
            let n = &adj[&cur];
            let nx = if n[0] != prev { n[0] } else if n.len() > 1 { n[1] } else { break };
            prev = cur;
            cur = nx;
        }
        if closed && lp.len() >= 3 {
            loops.push(lp);
        }
    }
    loops
}

fn face_normal(pos: &[DVec3], f: &[u32]) -> DVec3 {
    let mut n = DVec3::ZERO;
    for k in 0..f.len() {
        n += pos[f[k] as usize].cross(pos[f[(k + 1) % f.len()] as usize]);
    }
    n.normalize_or_zero()
}

/// Face close to the tangent plane of the nearest rail point (rings are
/// laid by projection, they must not wrap around a bevel).
fn flat(pos: &[DVec3], f: &[u32], rail: &[(DVec3, DVec3)]) -> bool {
    let c = f.iter().fold(DVec3::ZERO, |s, v| s + pos[*v as usize]) / f.len() as f64;
    let mut best = (f64::INFINITY, DVec3::ZERO);
    for (p, n) in rail {
        let d = p.distance_squared(c);
        if d < best.0 {
            best = (d, *n);
        }
    }
    face_normal(pos, f).dot(best.1) > 0.77
}

/// Distance from a point to closed polylines.
fn dist_to_loops(p: DVec3, loops: &[Vec<DVec3>]) -> f64 {
    let mut d = f64::INFINITY;
    for lp in loops {
        let n = lp.len();
        for i in 0..n {
            let (a, b) = (lp[i], lp[(i + 1) % n]);
            let e = b - a;
            let t = ((p - a).dot(e) / e.length_squared().max(1e-18)).clamp(0.0, 1.0);
            d = d.min(p.distance(a + e * t));
        }
    }
    d
}

/// Inserts exact points until no segment of the closed curve is longer than `max_len`.
fn densify(mut lp: Vec<DVec3>, on_curve: &dyn Fn(DVec3) -> DVec3, max_len: f64) -> Vec<DVec3> {
    for p in lp.iter_mut() {
        *p = on_curve(*p);
    }
    for _ in 0..12 {
        let n = lp.len();
        let mut out = Vec::with_capacity(n * 2);
        let mut split = false;
        for i in 0..n {
            let (a, b) = (lp[i], lp[(i + 1) % n]);
            out.push(a);
            if a.distance(b) > max_len {
                out.push(on_curve((a + b) * 0.5));
                split = true;
            }
        }
        lp = out;
        if !split {
            break;
        }
    }
    lp
}

fn arc_params(lp: &[DVec3]) -> (Vec<f64>, f64) {
    let n = lp.len();
    let mut s = vec![0.0; n];
    let mut acc = 0.0;
    for i in 1..n {
        acc += lp[i].distance(lp[i - 1]);
        s[i] = acc;
    }
    let total = acc + lp[n - 1].distance(lp[0]);
    (s.iter().map(|x| x / total).collect(), total)
}

/// Point at arc fraction `t` (wrapping) of a closed polyline.
fn at(lp: &[DVec3], s: &[f64], t: f64) -> DVec3 {
    let t = t.rem_euclid(1.0);
    let n = lp.len();
    let i = match s.binary_search_by(|x| x.partial_cmp(&t).unwrap()) {
        Ok(i) => return lp[i],
        Err(i) => i - 1,
    };
    let (s0, s1) = (s[i], if i + 1 < n { s[i + 1] } else { 1.0 });
    let w = (t - s0) / (s1 - s0).max(1e-15);
    lp[i].lerp(lp[(i + 1) % n], w)
}

/// Samples a closed curve once per vertex of `outer`, at the same arc
/// fractions, rotated and oriented to sit closest to it.
fn match_samples(outer: &[DVec3], curve: &[DVec3]) -> Vec<DVec3> {
    let (so, _) = arc_params(outer);
    let (sc, _) = arc_params(curve);
    let cost = |dir: f64, ph: f64| -> f64 { so.iter().zip(outer).map(|(t, o)| at(curve, &sc, ph + dir * t).distance_squared(*o)).sum() };
    let mut best = (f64::INFINITY, 1.0, 0.0);
    for dir in [1.0, -1.0] {
        for k in 0..720 {
            let ph = k as f64 / 720.0;
            let c = cost(dir, ph);
            if c < best.0 {
                best = (c, dir, ph);
            }
        }
    }
    let (_, dir, mut ph) = best;
    let mut step = 1.0 / 720.0;
    for _ in 0..30 {
        let (a, b) = (cost(dir, ph - step * 0.5), cost(dir, ph + step * 0.5));
        ph += if a < b { -step * 0.25 } else { step * 0.25 };
        step *= 0.7;
    }
    so.iter().map(|t| at(curve, &sc, ph + dir * t)).collect()
}

/// Cyclic shift / direction of loop `b` that best follows loop `a` (same length).
fn align_loop(pos: &[DVec3], a: &[DVec3], b: &[u32]) -> Vec<u32> {
    let n = b.len();
    let mut best = (f64::INFINITY, vec![]);
    for dir in [false, true] {
        let mut bb = b.to_vec();
        if dir {
            bb.reverse();
        }
        for s in 0..n {
            let c: f64 = (0..n).map(|i| a[i].distance_squared(pos[bb[(i + s) % n] as usize])).sum();
            if c < best.0 {
                best = (c, (0..n).map(|i| bb[(i + s) % n]).collect());
            }
        }
    }
    best.1
}

/// Ring weights between two loops: band heights graded from `h0` to `h1`.
fn ring_weights(gap: f64, h0: f64, h1: f64) -> Vec<f64> {
    let n = ((gap / (0.5 * (h0 + h1)).max(1e-9)).round() as usize).max(1);
    let hs: Vec<f64> = (0..n).map(|i| if n == 1 { 1.0 } else { h0 + (h1 - h0) * i as f64 / (n - 1) as f64 }).collect();
    let tot: f64 = hs.iter().sum();
    let mut acc = 0.0;
    let mut w = vec![];
    for h in &hs[..n - 1] {
        acc += h;
        w.push(acc / tot);
    }
    w
}

/// Quads between two aligned loops of the same length (plus graded rings).
fn loft(out: &mut Out, inner: &[u32], outer: &[u32], weights: &[f64], onto: &dyn Fn(DVec3) -> DVec3, patch: u32) {
    let n = inner.len();
    let mut rows: Vec<Vec<u32>> = vec![inner.to_vec()];
    for &w in weights {
        let r: Vec<u32> = (0..n)
            .map(|j| {
                let p = out.pos[inner[j] as usize].lerp(out.pos[outer[j] as usize], w);
                let q = onto(p);
                out.add(q)
            })
            .collect();
        rows.push(r);
    }
    rows.push(outer.to_vec());
    for w in rows.windows(2) {
        for j in 0..n {
            let j2 = (j + 1) % n;
            out.face(vec![w[0][j], w[0][j2], w[1][j2], w[1][j]], patch);
        }
    }
}

/// Bridge between two loops of different lengths: quads where both advance,
/// triangles where only one does.
fn bridge(out: &mut Out, a: &[u32], b: &[u32], patch: u32) {
    let (na, nb) = (a.len(), b.len());
    let (mut i, mut j) = (0usize, 0usize);
    while i < na || j < nb {
        let ta = (i + 1) as f64 / na as f64;
        let tb = (j + 1) as f64 / nb as f64;
        let (a0, a1, b0, b1) = (a[i % na], a[(i + 1) % na], b[j % nb], b[(j + 1) % nb]);
        if i < na && j < nb && (ta - tb).abs() < 0.5 / na.max(nb) as f64 {
            out.face(vec![a0, a1, b1, b0], patch);
            i += 1;
            j += 1;
        } else if j >= nb || (i < na && ta < tb) {
            out.face(vec![a0, a1, b0], patch);
            i += 1;
        } else {
            out.face(vec![a0, b1, b0], patch);
            j += 1;
        }
    }
}

/// Faces oriented consistently (propagation over shared edges), then
/// outward on the whole according to the exact gradient.
fn orient(out: &mut Out, field: &Field) {
    let nf = out.faces.len();
    let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, f) in out.faces.iter().enumerate() {
        for k in 0..f.len() {
            let (a, b) = (f[k], f[(k + 1) % f.len()]);
            edge_faces.entry((a.min(b), a.max(b))).or_default().push(i);
        }
    }
    let has_dir = |f: &Vec<u32>, a: u32, b: u32| (0..f.len()).any(|k| f[k] == a && f[(k + 1) % f.len()] == b);
    let mut done = vec![false; nf];
    for s in 0..nf {
        if done[s] {
            continue;
        }
        done[s] = true;
        let mut comp = vec![s];
        let mut stack = vec![s];
        while let Some(i) = stack.pop() {
            let f = out.faces[i].clone();
            for k in 0..f.len() {
                let (a, b) = (f[k], f[(k + 1) % f.len()]);
                for &j in &edge_faces[&(a.min(b), a.max(b))] {
                    if done[j] {
                        continue;
                    }
                    if has_dir(&out.faces[j], a, b) {
                        out.faces[j].reverse();
                    }
                    done[j] = true;
                    comp.push(j);
                    stack.push(j);
                }
            }
        }
        // outward vote over the component
        let mut vote = 0.0;
        for &i in &comp {
            let f = &out.faces[i];
            let mut n = DVec3::ZERO;
            let mut c = DVec3::ZERO;
            for k in 0..f.len() {
                n += out.pos[f[k] as usize].cross(out.pos[f[(k + 1) % f.len()] as usize]);
                c += out.pos[f[k] as usize];
            }
            vote += n.dot(field.grad(c / f.len() as f64)).signum();
        }
        if vote < 0.0 {
            for &i in &comp {
                out.faces[i].reverse();
            }
        }
    }
}

/// Quad size the constructive bake would use for `scene`, or None when its
/// history is not one it handles (the classic bake is used instead).
pub fn plan(scene: &Scene, st: &BakeSettings) -> Option<f64> {
    let field = Field::new(&compile(scene));
    plan_field(&field, st)
}

fn plan_field(field: &Field, st: &BakeSettings) -> Option<f64> {
    let steps = field.steps()?;
    if steps.is_empty() || steps.len() > 2 {
        return None;
    }
    for (i, s) in steps.iter().enumerate() {
        let pr = &field.prims[s.prim as usize];
        if field.instances(s.prim).len() != 1 || pr.shell > 0.0 || (i > 0 && (s.mode > 1 || s.chamfer)) {
            return None;
        }
    }
    let pr = &field.prims[steps[0].prim as usize];
    let (lo, hi) = gen::prim_bounds(pr, 0.0);
    let extent = ((hi - lo).max_element() - 0.4).max(1e-3) * pr.sc;
    Some(extent / st.divisions.max(1.0))
}

/// Bake entry point: the constructive build when the history allows it.
pub fn build_auto(scene: &Scene, st: &BakeSettings, progress: &Progress) -> Option<Baked> {
    let size = plan(scene, st)?;
    let bs = BuildSettings { size, tol: (st.tol * 2.0).max(0.01), fillet_rows: 0, threads: st.threads };
    let mut b = build(scene, &bs, progress)?;
    b.stats.quad_size = size;
    Some(b)
}

/// Builds the mesh of `scene` operation by operation. None when the history
/// is not (yet) one this builder handles: the caller falls back.
pub fn build(scene: &Scene, st: &BuildSettings, progress: &Progress) -> Option<Baked> {
    let t0 = Instant::now();
    progress.set(0, 1);
    let field = Field::new(&compile(scene));
    let steps: Vec<Step> = field.steps()?;
    if steps.is_empty() || steps.len() > 2 {
        return None;
    }
    let surf = |s: &Step| -> Option<Surf> {
        let pr = &field.prims[s.prim as usize];
        let inst = field.instances(s.prim);
        if inst.len() != 1 || pr.shell > 0.0 {
            return None;
        }
        Some(Surf { pr, inst: inst[0] })
    };
    let body = surf(&steps[0])?;
    let gs = GenSettings { tol: st.tol, max_edge: st.size, segs: 0 };
    let mut out = Out::default();
    progress.set(1, 1);
    let body_faces = native(&mut out, &body, &gs);
    if steps.len() == 1 {
        for f in body_faces {
            out.face(f, P_NATIVE);
        }
    } else {
        let step = steps[1];
        let tool = surf(&step)?;
        if step.mode > 1 || step.chamfer {
            return None;
        }
        let carve = step.mode == 1;
        let k = step.k;
        let clear = st.size;
        let sa = |p: DVec3| body.d(p);
        let sb = |p: DVec3| tool.d(p);
        // rails and corner as exact curves
        let rail_a = |p: DVec3| solve2(&sa, &|x| sb(x) - k, p, st.size);
        let corner = |p: DVec3| solve2(&sa, &sb, p, st.size);
        let ka = if carve { -k } else { k };
        let rail_b = |p: DVec3| solve2(&sb, &|x| sa(x) - ka, p, st.size);
        // body: cleared inside its rail and around it (room for the rings)
        let all: Vec<usize> = (0..body_faces.len()).collect();
        let rails: Vec<Vec<DVec3>> = contour(&out.pos, &body_faces, &all, &|p| sb(p) - k).into_iter().map(|c| densify(c, &rail_a, st.size / 4.0)).collect();
        // rings are laid by projection: they stay on the smooth part around the rail
        let rail_n: Vec<(DVec3, DVec3)> = rails.iter().flatten().map(|p| (*p, grad(&sa, *p).normalize_or_zero())).collect();
        let (kept_a, comps_a) = regions(&out.pos, &body_faces, &|p| dist_to_loops(p, &rails) < clear, &|p| sb(p) < k, &|f| flat(&out.pos, f, &rail_n));
        for (i, f) in body_faces.iter().enumerate() {
            if kept_a[i] {
                out.face(f.clone(), P_NATIVE);
            }
        }
        struct Hole {
            rail_a: Vec<u32>,
            rail_b: Vec<u32>,
        }
        let mut holes: Vec<Hole> = vec![];
        for comp in &comps_a {
            let outl = outline(&body_faces, comp);
            let curves = contour(&out.pos, &body_faces, comp, &|p| sb(p) - k);
            if curves.is_empty() {
                if comp.iter().any(|&i| body_faces[i].iter().any(|&v| sb(out.pos[v as usize]) < 0.0)) {
                    // the tool cuts through the whole region: not handled yet
                    return None;
                }
                // the tool does not reach the surface here: keep it native
                for &i in comp {
                    out.face(body_faces[i].clone(), P_NATIVE);
                }
                continue;
            }
            if outl.len() != 1 || curves.len() != 1 {
                return None;
            }
            let ring_ids = &outl[0];
            let ring: Vec<DVec3> = ring_ids.iter().map(|v| out.pos[*v as usize]).collect();
            let dense = densify(curves[0].clone(), &rail_a, st.size / 16.0);
            let samples: Vec<DVec3> = match_samples(&ring, &dense).into_iter().map(rail_a).collect();
            let pa: Vec<u32> = samples.iter().map(|p| out.add(*p)).collect();
            // fillet rows between the rails, along the cross-section through the corner
            let pb: Vec<u32> = if k > 1e-9 {
                let cs: Vec<DVec3> = samples.iter().map(|p| corner(*p)).collect();
                let qs: Vec<DVec3> = cs.iter().map(|c| rail_b(*c)).collect();
                let n = pa.len();
                let col = (0..n).map(|j| samples[j].distance(samples[(j + 1) % n])).sum::<f64>() / n as f64;
                let prof = (0..n).map(|j| samples[j].distance(cs[j]) + cs[j].distance(qs[j])).sum::<f64>() / n as f64 * 0.8;
                let rows = if st.fillet_rows > 0 { st.fillet_rows } else { ((prof / col).round() as usize).max(2) };
                let qb: Vec<u32> = qs.iter().map(|q| out.add(*q)).collect();
                let mut grid: Vec<Vec<u32>> = vec![pa.clone()];
                for r in 1..rows {
                    let t = r as f64 / rows as f64;
                    let row: Vec<u32> = (0..n)
                        .map(|j| {
                            let (p, c, q) = (samples[j], cs[j], qs[j]);
                            let b = p * (1.0 - t) * (1.0 - t) + c * 2.0 * t * (1.0 - t) + q * t * t;
                            let x = field.project(b).unwrap_or(b);
                            out.add(x)
                        })
                        .collect();
                    grid.push(row);
                }
                grid.push(qb.clone());
                for w in grid.windows(2) {
                    for j in 0..n {
                        let j2 = (j + 1) % n;
                        out.face(vec![w[0][j], w[0][j2], w[1][j2], w[1][j]], P_FILLET);
                    }
                }
                qb
            } else {
                pa.clone()
            };
            // rings from the rail to the native outline, on the body surface
            let n = pa.len();
            let gap = (0..n).map(|j| samples[j].distance(ring[j])).sum::<f64>() / n as f64;
            let h0 = (0..n).map(|j| samples[j].distance(samples[(j + 1) % n])).sum::<f64>() / n as f64;
            let h1 = (0..n).map(|j| ring[j].distance(ring[(j + 1) % n])).sum::<f64>() / n as f64;
            let w = ring_weights(gap, h0, h1);
            loft(&mut out, &pa, ring_ids, &w, &|p| body.project(p), P_LOFT);
            holes.push(Hole { rail_a: pa, rail_b: pb });
        }
        // the tool: native mesh with matching columns, kept where it is the surface
        let kcols = holes.first().map(|h| h.rail_b.len()).unwrap_or(0);
        let gs_t = GenSettings { segs: kcols, ..gs };
        let tool_faces = native(&mut out, &tool, &gs_t);
        // clearance shrinks until part of the tool is native (small bosses)
        let mut split = None;
        let rails_b: Vec<Vec<DVec3>> = holes.iter().map(|h| h.rail_b.iter().map(|v| out.pos[*v as usize]).collect()).collect();
        let rail_nb: Vec<(DVec3, DVec3)> = rails_b.iter().flatten().map(|p| (*p, grad(&sb, *p).normalize_or_zero())).collect();
        for c in [clear, clear * 0.5, clear * 0.25, 0.0] {
            let must_b = |p: DVec3| if carve { sa(p) > -k } else { sa(p) < k };
            let pos_t = &out.pos;
            let r = regions(&out.pos, &tool_faces, &|p| dist_to_loops(p, &rails_b) < c, &must_b, &|f| flat(pos_t, f, &rail_nb));
            if r.0.iter().any(|&x| x) {
                split = Some(r);
                break;
            }
        }
        let (kept_b, comps_b) = split?;
        for (i, f) in tool_faces.iter().enumerate() {
            if kept_b[i] {
                out.face(f.clone(), P_TOOL);
            }
        }
        let mut used = vec![false; comps_b.len()];
        for h in &holes {
            let q: Vec<DVec3> = h.rail_b.iter().map(|v| out.pos[*v as usize]).collect();
            // tool region holding this rail: the one nearest to it
            let mut best = (f64::INFINITY, usize::MAX);
            for (ci, comp) in comps_b.iter().enumerate() {
                for &fi in comp {
                    for &v in &tool_faces[fi] {
                        let d = out.pos[v as usize].distance(q[0]);
                        if d < best.0 {
                            best = (d, ci);
                        }
                    }
                }
            }
            if best.1 == usize::MAX {
                return None;
            }
            used[best.1] = true;
            let outl = outline(&tool_faces, &comps_b[best.1]);
            // the loop of the kept part, nearest to the rail
            let ring_ids = outl
                .iter()
                .min_by(|a, b| {
                    let da = a.iter().map(|v| out.pos[*v as usize].distance(q[0])).fold(f64::INFINITY, f64::min);
                    let db = b.iter().map(|v| out.pos[*v as usize].distance(q[0])).fold(f64::INFINITY, f64::min);
                    da.partial_cmp(&db).unwrap()
                })?
                .clone();
            let n = q.len();
            let h0 = (0..n).map(|j| q[j].distance(q[(j + 1) % n])).sum::<f64>() / n as f64;
            if ring_ids.len() == n {
                let ring_ids = align_loop(&out.pos, &q, &ring_ids);
                let ring: Vec<DVec3> = ring_ids.iter().map(|v| out.pos[*v as usize]).collect();
                let gap = (0..n).map(|j| q[j].distance(ring[j])).sum::<f64>() / n as f64;
                let w = ring_weights(gap, h0, st.size);
                loft(&mut out, &h.rail_b, &ring_ids, &w, &|p| tool.project(p), P_TOOL_LOFT);
            } else {
                let ring: Vec<DVec3> = ring_ids.iter().map(|v| out.pos[*v as usize]).collect();
                let m = ring.len();
                let mut best = (f64::INFINITY, 0usize);
                for s in 0..m {
                    let d = ring[s].distance(q[0]);
                    if d < best.0 {
                        best = (d, s);
                    }
                }
                let mut r2: Vec<u32> = (0..m).map(|i| ring_ids[(i + best.1) % m]).collect();
                // same turning direction as the rail
                let fwd = out.pos[r2[1] as usize].distance(q[1]) < out.pos[r2[m - 1] as usize].distance(q[1]);
                if !fwd {
                    r2 = std::iter::once(r2[0]).chain(r2[1..].iter().rev().copied()).collect();
                }
                bridge(&mut out, &h.rail_b, &r2, P_TOOL_LOFT);
            }
            let _ = &h.rail_a;
        }
        // tool regions with no rail: outside the body, dropped
        let _ = used;
    }
    // compact: drop unused vertices
    let mut remap = vec![u32::MAX; out.pos.len()];
    let mut pos = vec![];
    for f in &mut out.faces {
        for v in f.iter_mut() {
            if remap[*v as usize] == u32::MAX {
                remap[*v as usize] = pos.len() as u32;
                pos.push(out.pos[*v as usize]);
            }
            *v = remap[*v as usize];
        }
    }
    out.pos = pos;
    orient(&mut out, &field);
    let mut b = Baked::default();
    for (f, p) in out.faces.into_iter().zip(out.patch) {
        b.faces.push(BFace { verts: f, normals: vec![], mat: 0, patch: p });
    }
    b.positions = out.pos;
    let bs = BakeSettings { tol: st.tol, max_edge: st.size, trim_len: st.size, threads: st.threads, divisions: 0.0 };
    progress.set(4, b.faces.len() as u32);
    finish(&field, &mut b, &bs, progress);
    b.stats.terms = steps.len();
    b.stats.patches = steps.len();
    b.stats.constructive = true;
    b.stats.seconds = t0.elapsed().as_secs_f64();
    Some(b)
}
