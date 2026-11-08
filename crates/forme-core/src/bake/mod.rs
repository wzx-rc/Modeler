//! Bake: parametric history → clean export mesh.
//!
//! Instead of polygonising a voxel field, every primitive copy is tessellated
//! from its own parameters (rings of revolution, rounded boxes, quad spheres,
//! rounded extrusions), then trimmed exactly where the final surface stops
//! belonging to it, and the trimmed patches are sewn together. Kept regions
//! stay the clean parametric quads; only trim curves get extra vertices.
//! One shot: the result does not keep any link with the scene.

pub mod field;
pub mod gen;
pub mod patch;
pub mod quad;
pub mod build;

use crate::mesh::Mesh;
use crate::program::compile;
use crate::scene::{Material, Scene};
use field::{Field, TermKey};
use gen::{GenSettings, PolyMesh};
use glam::{DVec2, DVec3, Vec3};
use patch::{PatchOut, PatchSettings, Term};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Draft,
    Standard,
    Max,
}

impl Quality {
    pub fn label(self) -> &'static str {
        match self {
            Quality::Draft => "Brouillon",
            Quality::Standard => "Standard",
            Quality::Max => "Max",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BakeSettings {
    pub tol: f64,
    pub max_edge: f64,
    pub trim_len: f64,
    pub threads: usize,
    /// Bake par opération: quads across the main body.
    pub divisions: f64,
}

impl BakeSettings {
    pub fn preset(q: Quality) -> Self {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(16);
        match q {
            Quality::Draft => BakeSettings { tol: 0.05, max_edge: 8.0, trim_len: 1.0, threads, divisions: 5.0 },
            Quality::Standard => BakeSettings { tol: 0.015, max_edge: 5.0, trim_len: 0.5, threads, divisions: 8.0 },
            Quality::Max => BakeSettings { tol: 0.006, max_edge: 4.0, trim_len: 0.5, threads, divisions: 12.0 },
        }
    }
}

/// Shared progress, readable from the UI thread while a bake runs.
#[derive(Default)]
pub struct Progress {
    pub stage: AtomicU32,
    pub done: AtomicU32,
    pub total: AtomicU32,
    pub cancel: AtomicBool,
}

pub const STAGES: [&str; 5] = ["Analyse de l'historique", "Surfaces paramétriques", "Découpe exacte", "Couture", "Normales"];

impl Progress {
    pub fn fraction(&self) -> f32 {
        let s = self.stage.load(Ordering::Relaxed) as f32;
        let t = self.total.load(Ordering::Relaxed).max(1) as f32;
        let d = self.done.load(Ordering::Relaxed) as f32;
        // stage weights: the trim stage dominates
        let w = [0.02f32, 0.13, 0.7, 0.1, 0.05];
        let mut f = 0.0;
        for i in 0..(s as usize).min(5) {
            f += w[i];
        }
        if (s as usize) < 5 {
            f += w[s as usize] * (d / t).min(1.0);
        }
        f.min(1.0)
    }
    fn set(&self, stage: u32, total: u32) {
        self.stage.store(stage, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }
    fn tick(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Debug, Default)]
pub struct BakeStats {
    pub terms: usize,
    pub patches: usize,
    pub quads: usize,
    pub tris: usize,
    pub ngons: usize,
    pub verts: usize,
    pub open_edges: usize,
    pub nonmanifold_edges: usize,
    pub holes_filled: usize,
    pub max_dev: f64,
    pub mean_dev: f64,
    pub seconds: f64,
    pub stage_seconds: [f64; 5],
    /// Built operation by operation (all-quad topology), not by surfaces.
    pub constructive: bool,
    /// Bake par opération: target quad size (mm).
    pub quad_size: f64,
}

#[derive(Clone, Debug)]
pub struct BFace {
    pub verts: Vec<u32>,
    pub normals: Vec<DVec3>,
    pub mat: u32,
    /// Patch (term) the face comes from, u32::MAX for hole fillers.
    pub patch: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Baked {
    pub positions: Vec<DVec3>,
    pub faces: Vec<BFace>,
    pub stats: BakeStats,
}

fn par_for<T: Send, F: Fn(usize) -> T + Sync>(n: usize, threads: usize, order: &[usize], f: F) -> Vec<Option<T>> {
    let next = AtomicU32::new(0);
    let out: Mutex<Vec<Option<T>>> = Mutex::new((0..n).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed) as usize;
                if i >= order.len() {
                    break;
                }
                let idx = order[i];
                let r = f(idx);
                out.lock().unwrap()[idx] = Some(r);
            });
        }
    });
    out.into_inner().unwrap()
}

/// Runs the whole bake. Returns None if cancelled.
pub fn bake(scene: &Scene, st: &BakeSettings, progress: &Progress) -> Option<Baked> {
    // the history this builder handles gets a topology built per operation
    if let Some(b) = build::build_auto(scene, st, progress) {
        return Some(b);
    }
    if progress.cancelled() {
        return None;
    }
    let t0 = Instant::now();
    let mut stage_t = [0.0f64; 5];
    progress.set(0, 1);
    let prog = compile(scene);
    let field = Field::new(&prog);
    if field.is_empty() {
        return Some(Baked::default());
    }
    // every (primitive, level) that can own part of the surface, and its copies
    let levels = field.term_levels();
    let mut terms: Vec<Term> = vec![];
    for &(prim, level) in &levels {
        let pr = &field.prims[prim as usize];
        for inst in field.instances(prim) {
            let key = field::Owner { prim, copy: inst.copy, level }.key();
            terms.push(Term { key, inst, level_local: level / pr.sc });
        }
    }
    stage_t[0] = t0.elapsed().as_secs_f64();

    // parametric meshes, one per (primitive, level), shared by all copies
    let t1 = Instant::now();
    progress.set(1, levels.len() as u32);
    let gs = GenSettings { tol: st.tol, max_edge: st.max_edge, segs: 0 };
    let order: Vec<usize> = (0..levels.len()).collect();
    let bases: Vec<PolyMesh> = par_for(levels.len(), st.threads, &order, |i| {
        let (prim, level) = levels[i];
        let pr = &field.prims[prim as usize];
        let m = if progress.cancelled() { PolyMesh::default() } else { gen::generate(pr, level / pr.sc, &gs) };
        progress.tick();
        m
    })
    .into_iter()
    .map(|m| m.unwrap_or_default())
    .collect();
    if progress.cancelled() {
        return None;
    }
    let base_of: HashMap<(u32, i64), usize> = levels.iter().enumerate().map(|(i, &(p, l))| ((p, (l * 1e5).round() as i64), i)).collect();
    stage_t[1] = t1.elapsed().as_secs_f64();

    // trim every copy against the final surface
    let t2 = Instant::now();
    progress.set(2, terms.len() as u32);
    let ps = PatchSettings { trim_len: st.trim_len, min_size: st.trim_len / 8.0, max_level: 9 };
    let mut order: Vec<usize> = (0..terms.len()).collect();
    let size_of = |t: &Term| bases[base_of[&(t.key.prim, t.key.level)]].faces.len();
    order.sort_by_key(|&i| std::cmp::Reverse(size_of(&terms[i])));
    // world box and smallest dimension of every term (to find small features)
    let boxes: Vec<(DVec3, DVec3, f64)> = terms
        .iter()
        .map(|t| {
            let b = &bases[base_of[&(t.key.prim, t.key.level)]];
            let mut lo = DVec3::splat(f64::MAX);
            let mut hi = DVec3::splat(f64::MIN);
            for p in &b.pos {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
            if b.pos.is_empty() {
                return (DVec3::ZERO, DVec3::ZERO, 0.0);
            }
            let size = (hi - lo).min_element() * field.prims[t.key.prim as usize].sc;
            let mut wlo = DVec3::splat(f64::MAX);
            let mut whi = DVec3::splat(f64::MIN);
            for c in 0..8 {
                let q = DVec3::new(if c & 1 == 0 { lo.x } else { hi.x }, if c & 2 == 0 { lo.y } else { hi.y }, if c & 4 == 0 { lo.z } else { hi.z });
                let w = t.inst.to_world.transform_point3(q);
                wlo = wlo.min(w);
                whi = whi.max(w);
            }
            let pad = DVec3::splat(field.blend_bound + 0.05);
            (wlo - pad, whi + pad, size)
        })
        .collect();
    // terms that visibly own something somewhere (others are not features)
    let far = field.blend_bound * 1.5 + 1e-3;
    let live: Vec<bool> = par_for(terms.len(), st.threads, &order, |i| {
        let t = &terms[i];
        let b = &bases[base_of[&(t.key.prim, t.key.level)]];
        b.pos.iter().any(|p| {
            let s = field.eval(t.inst.to_world.transform_point3(*p));
            s.owner.key() == t.key && s.d.abs() <= far
        })
    })
    .into_iter()
    .map(|l| l.unwrap_or(false))
    .collect();
    let patches: Vec<PatchOut> = par_for(terms.len(), st.threads, &order, |i| {
        let t = &terms[i];
        let (lo, hi, _) = boxes[i];
        let features: Vec<patch::Feature> = boxes
            .iter()
            .enumerate()
            .filter(|&(j, b)| j != i && live[j] && b.2 > 0.0 && b.2 < st.max_edge * 2.0 && b.0.cmple(hi).all() && lo.cmple(b.1).all())
            .map(|(_, b)| patch::Feature { lo: b.0, hi: b.1, size: b.2 })
            .collect();
        let tt = Instant::now();
        let r = if progress.cancelled() { PatchOut::default() } else { patch::process(&field, t, &bases[base_of[&(t.key.prim, t.key.level)]], &features, &[], &ps) };
        progress.tick();
        if std::env::var("BAKE_TIME").is_ok() && tt.elapsed().as_secs_f64() > 0.5 {
            eprintln!("patch prim {} copy {} level {} : {:.2}s faces {} refined {} cuts {}", t.key.prim, t.key.copy, t.key.level, tt.elapsed().as_secs_f64(), r.faces.len(), r.refined, r.cuts);
        }
        r
    })
    .into_iter()
    .map(|p| p.unwrap_or_default())
    .collect();
    if progress.cancelled() {
        return None;
    }
    // cross-check: every trim point a patch found must face a trim edge of the
    // term on the other side; otherwise that term missed the curve (a feature
    // hidden between its vertices) and is processed again around those points
    let mut patches = patches;
    let index_of: HashMap<TermKey, usize> = terms.iter().enumerate().map(|(i, t)| (t.key, i)).collect();
    for _round in 0..1 {
        let mut hints: Vec<Vec<DVec3>> = vec![vec![]; terms.len()];
        for p in &patches {
            for (x, o) in p.pos.iter().zip(&p.other) {
                if let Some(k) = o {
                    if let Some(&i) = index_of.get(k) {
                        hints[i].push(*x);
                    }
                }
            }
        }
        let redo: Vec<usize> = (0..terms.len()).filter(|&i| !hints[i].is_empty() && !explained(&patches[i], &hints[i], st.trim_len)).collect();
        if std::env::var("BAKE_EDGES").is_ok() || std::env::var("BAKE_TIME").is_ok() {
            eprintln!("round {_round}: redo {:?}", redo.iter().map(|&i| (terms[i].key.prim, terms[i].key.copy, terms[i].key.level, hints[i].len())).collect::<Vec<_>>());
        }
        if redo.is_empty() {
            break;
        }
        progress.set(2, redo.len() as u32);
        let redone = par_for(redo.len(), st.threads, &(0..redo.len()).collect::<Vec<_>>(), |k| {
            let i = redo[k];
            let t = &terms[i];
            let (lo, hi, _) = boxes[i];
            let features: Vec<patch::Feature> = boxes
                .iter()
                .enumerate()
                .filter(|&(j, b)| j != i && live[j] && b.2 > 0.0 && b.2 < st.max_edge * 2.0 && b.0.cmple(hi).all() && lo.cmple(b.1).all())
                .map(|(_, b)| patch::Feature { lo: b.0, hi: b.1, size: b.2 })
                .collect();
            let r = patch::process(&field, t, &bases[base_of[&(t.key.prim, t.key.level)]], &features, &hints[i], &ps);
            progress.tick();
            r
        });
        for (k, r) in redone.into_iter().enumerate() {
            if let Some(r) = r {
                patches[redo[k]] = r;
            }
        }
    }
    stage_t[2] = t2.elapsed().as_secs_f64();

    // sew
    let t3 = Instant::now();
    progress.set(3, 4);
    let mut out = stitch(&field, patches, st, progress);
    stage_t[3] = t3.elapsed().as_secs_f64();

    let t4 = Instant::now();
    progress.set(4, out.faces.len() as u32);
    finish(&field, &mut out, st, progress);
    stage_t[4] = t4.elapsed().as_secs_f64();
    out.stats.terms = terms.len();
    out.stats.seconds = t0.elapsed().as_secs_f64();
    out.stats.stage_seconds = stage_t;
    Some(out)
}

/// True if every hint lies near a trim edge of the patch.
fn explained(p: &PatchOut, hints: &[DVec3], trim_len: f64) -> bool {
    if p.faces.is_empty() {
        return false;
    }
    let ec = edge_counts(&p.faces);
    let cell = trim_len.max(1e-3);
    let key = |x: DVec3| ((x.x / cell).floor() as i64, (x.y / cell).floor() as i64, (x.z / cell).floor() as i64);
    let mut grid: HashMap<(i64, i64, i64), Vec<(DVec3, DVec3)>> = HashMap::new();
    for (&(a, b), _) in ec.iter() {
        if ec.contains_key(&(b, a)) {
            continue;
        }
        let (pa, pb) = (p.pos[a as usize], p.pos[b as usize]);
        let (k0, k1) = (key(pa.min(pb)), key(pa.max(pb)));
        for x in k0.0..=k1.0 {
            for y in k0.1..=k1.1 {
                for z in k0.2..=k1.2 {
                    grid.entry((x, y, z)).or_default().push((pa, pb));
                }
            }
        }
    }
    let tol = trim_len * 0.3;
    let dbg = std::env::var("BAKE_EDGES").is_ok();
    let mut miss = 0;
    let res = hints.iter().filter(|&&h| {
        let k = key(h);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(l) = grid.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) {
                        for (a, b) in l {
                            let e = *b - *a;
                            let t = ((h - *a).dot(e) / e.length_squared().max(1e-30)).clamp(0.0, 1.0);
                            if (*a + e * t).distance(h) < tol {
                                return false;
                            }
                        }
                    }
                }
            }
        }
        true
    })
    .inspect(|h| {
        miss += 1;
        if dbg && miss <= 3 {
            eprintln!("   unexplained hint {:?}", h);
        }
    })
    .count();
    if dbg && res > 0 {
        eprintln!("   {res} of {} hints unexplained", hints.len());
    }
    res == 0
}

#[derive(Clone, Copy)]
struct GV {
    patch: u32,
    other: Option<TermKey>,
}

fn edge_counts(faces: &[Vec<u32>]) -> HashMap<(u32, u32), u32> {
    let mut m: HashMap<(u32, u32), u32> = HashMap::new();
    for f in faces {
        for i in 0..f.len() {
            let (a, b) = (f[i], f[(i + 1) % f.len()]);
            *m.entry((a, b)).or_default() += 1;
        }
    }
    m
}

fn stitch(field: &Field, patches: Vec<PatchOut>, st: &BakeSettings, progress: &Progress) -> Baked {
    let mut pos: Vec<DVec3> = vec![];
    let mut info: Vec<GV> = vec![];
    let mut faces: Vec<Vec<u32>> = vec![];
    let mut face_patch: Vec<u32> = vec![];
    let mut keys: Vec<TermKey> = vec![];
    let mut used = 0;
    for p in patches.into_iter() {
        if p.faces.is_empty() {
            continue;
        }
        used += 1;
        let pid = keys.len() as u32;
        keys.push(p.key.unwrap());
        let off = pos.len() as u32;
        for (i, x) in p.pos.iter().enumerate() {
            pos.push(*x);
            info.push(GV { patch: pid, other: p.other[i] });
        }
        for f in p.faces {
            faces.push(f.iter().map(|v| v + off).collect());
            face_patch.push(pid);
        }
    }
    progress.tick();
    // orient each patch as a whole along the outward gradient (carved terms
    // come in reversed); per face votes would flip thin slivers
    let mut vote = vec![0.0f64; keys.len()];
    for (f, &p) in faces.iter().zip(&face_patch) {
        let mut n = DVec3::ZERO;
        let mut c = DVec3::ZERO;
        for i in 0..f.len() {
            let a = pos[f[i] as usize];
            n += a.cross(pos[f[(i + 1) % f.len()] as usize]);
            c += a;
        }
        c /= f.len() as f64;
        if n.length_squared() > 1e-30 {
            vote[p as usize] += n.dot(field.grad(c).normalize_or_zero());
        }
    }
    for (f, &p) in faces.iter_mut().zip(&face_patch) {
        if vote[p as usize] < 0.0 {
            f.reverse();
        }
    }
    // weld coincident trim points of different patches
    let weld_eps = 5e-4;
    let boundary_verts = |faces: &[Vec<u32>]| -> Vec<u32> {
        let ec = edge_counts(faces);
        let mut v: Vec<u32> = vec![];
        for (&(a, b), _) in ec.iter() {
            if !ec.contains_key(&(b, a)) {
                v.push(a);
                v.push(b);
            }
        }
        v.sort();
        v.dedup();
        v
    };
    let cell = st.trim_len.max(0.05);
    let hkey = |p: DVec3| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64, (p.z / cell).floor() as i64);
    // every vertex takes part: a kept vertex lying on a trim curve can meet
    // another patch's trim point
    let bverts: Vec<u32> = (0..pos.len() as u32).collect();
    let wcell = weld_eps * 4.0;
    let hkey_w = |p: DVec3| ((p.x / wcell).floor() as i64, (p.y / wcell).floor() as i64, (p.z / wcell).floor() as i64);
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    for &v in &bverts {
        grid.entry(hkey_w(pos[v as usize])).or_default().push(v);
    }
    let hkey_weld = hkey_w;
    let mut remap: Vec<u32> = (0..pos.len() as u32).collect();
    for &v in &bverts {
        if remap[v as usize] != v {
            continue;
        }
        let k = hkey_weld(pos[v as usize]);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = grid.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) {
                        for &w in list {
                            if w > v && remap[w as usize] == w && pos[w as usize].distance(pos[v as usize]) < weld_eps {
                                remap[w as usize] = v;
                            }
                        }
                    }
                }
            }
        }
    }
    for f in faces.iter_mut() {
        for v in f.iter_mut() {
            *v = remap[*v as usize];
        }
    }
    // faces pinched by the weld split into simple polygons
    let mut split_faces: Vec<Vec<u32>> = vec![];
    let mut split_patch: Vec<u32> = vec![];
    for (f, &p) in faces.iter().zip(&face_patch) {
        let mut f = f.clone();
        f.dedup();
        while f.len() > 1 && f.first() == f.last() {
            f.pop();
        }
        for sub in split_pinched(f) {
            split_faces.push(sub);
            split_patch.push(p);
        }
    }
    faces = split_faces;
    face_patch = split_patch;
    let keep_f: Vec<bool> = faces.iter().map(|f| f.len() >= 3).collect();
    let mut k = keep_f.iter();
    faces.retain(|_| *k.next().unwrap());
    let mut k = keep_f.iter();
    face_patch.retain(|_| *k.next().unwrap());
    progress.tick();
    // T-junctions: insert each patch's trim points into the facing patch's trim edges
    let ec = edge_counts(&faces);
    let mut edge_patch: HashMap<(u32, u32), u32> = HashMap::new();
    for (f, &p) in faces.iter().zip(&face_patch) {
        for i in 0..f.len() {
            edge_patch.insert((f[i], f[(i + 1) % f.len()]), p);
        }
    }
    let bedges: Vec<(u32, u32)> = ec.keys().filter(|&&(a, b)| !ec.contains_key(&(b, a))).copied().collect();
    let bverts = boundary_verts(&faces);
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    for &v in &bverts {
        grid.entry(hkey(pos[v as usize])).or_default().push(v);
    }
    let mut inserts: HashMap<(u32, u32), Vec<(f64, u32)>> = HashMap::new();
    let mut best_edge: HashMap<u32, (f64, (u32, u32))> = HashMap::new();
    for &(a, b) in &bedges {
        let (pa, pb) = (pos[a as usize], pos[b as usize]);
        let e = pb - pa;
        let len = e.length();
        if len < 1e-12 {
            continue;
        }
        let tol = 0.2 * len + 1e-5;
        let pa_patch = edge_patch[&(a, b)];
        let lo = pa.min(pb) - DVec3::splat(tol);
        let hi = pa.max(pb) + DVec3::splat(tol);
        let (k0, k1) = (hkey(lo), hkey(hi));
        let mut found: Vec<(f64, u32)> = vec![];
        for x in k0.0..=k1.0 {
            for y in k0.1..=k1.1 {
                for z in k0.2..=k1.2 {
                    let Some(list) = grid.get(&(x, y, z)) else { continue };
                    for &v in list {
                        if v == a || v == b {
                            continue;
                        }
                        let gv = info[v as usize];
                        if gv.patch == pa_patch {
                            continue;
                        }
                        // the point must face this patch (or one of the edge's
                        // ends must face the point's patch)
                        let edge_key = keys[pa_patch as usize];
                        let facing = match gv.other {
                            Some(k) => k == edge_key || k.prim == u32::MAX || k == keys[gv.patch as usize],
                            None => true,
                        } || [info[a as usize].other, info[b as usize].other].iter().flatten().any(|k| *k == keys[gv.patch as usize]);
                        if !facing {
                            continue;
                        }
                        let p = pos[v as usize];
                        let t = (p - pa).dot(e) / (len * len);
                        if t <= 0.0 || t >= 1.0 {
                            continue;
                        }
                        let q = pa + e * t;
                        if q.distance(p) > tol || p.distance(pa) < 2.0 * weld_eps || p.distance(pb) < 2.0 * weld_eps {
                            continue;
                        }
                        found.push((t, v));
                        let d = q.distance(p);
                        let e = best_edge.entry(v).or_insert((f64::MAX, (a, b)));
                        if d < e.0 {
                            *e = (d, (a, b));
                        }
                    }
                }
            }
        }
        if !found.is_empty() {
            found.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
            found.dedup_by_key(|x| x.1);
            inserts.insert((a, b), found);
        }
    }
    // a trim point goes into its nearest facing edge only
    for (e, list) in inserts.iter_mut() {
        list.retain(|(_, v)| best_edge.get(v).map(|b| b.1 == *e).unwrap_or(false));
    }
    for f in faces.iter_mut() {
        if !(0..f.len()).any(|i| inserts.contains_key(&(f[i], f[(i + 1) % f.len()]))) {
            continue;
        }
        let mut nf = vec![];
        for i in 0..f.len() {
            let (a, b) = (f[i], f[(i + 1) % f.len()]);
            nf.push(a);
            if let Some(list) = inserts.get(&(a, b)) {
                for (_, v) in list {
                    nf.push(*v);
                }
            }
        }
        *f = nf;
    }
    progress.tick();
    if std::env::var("BAKE_EDGES").is_ok() {
        let mut und: HashMap<(u32, u32), Vec<(usize, bool)>> = HashMap::new();
        for (fi, f) in faces.iter().enumerate() {
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                und.entry((a.min(b), a.max(b))).or_default().push((fi, a < b));
            }
        }
        let mut shown = 0;
        for (fi, f) in faces.iter().enumerate() {
            if f.len() > 60 && shown < 2 {
                shown += 1;
                eprintln!("long face {fi} patch {:?}", keys.get(face_patch[fi] as usize));
                for &v in f {
                    let own = info[v as usize].patch == face_patch[fi];
                    eprintln!("   {} {:?}", if own { "own" } else { "ins" }, pos[v as usize]);
                }
            }
        }
        let mut shown = 0;
        for (&(a, b), uses) in &und {
            if uses.len() > 2 && shown < 6 {
                shown += 1;
                eprintln!("nonmanifold {:?}-{:?}", pos[a as usize], pos[b as usize]);
                for (fi, dir) in uses {
                    let k = keys.get(face_patch[*fi] as usize);
                    eprintln!("   face {fi} dir {dir} patch {:?} len {}", k, faces[*fi].len());
                }
            }
        }
    }
    // remaining gaps: thin bands between two trims that should have met (a
    // sliver too thin for any patch) and small holes at triple junctions
    let loops: Vec<Vec<u32>> = boundary_loops(&faces, &pos, st.trim_len * 2.0).into_iter().flat_map(split_pinched).collect();
    let loops: Vec<Vec<u32>> = bridge_parallel(&pos, loops, st.trim_len);
    let holes = loops.len();
    let mut edge_set: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    for f in &faces {
        for i in 0..f.len() {
            let (a, b) = (f[i], f[(i + 1) % f.len()]);
            edge_set.insert((a.min(b), a.max(b)));
        }
    }
    for lp in &loops {
        if std::env::var("BAKE_EDGES").is_ok() && lp.len() > 3 {
            eprintln!("hole {} verts", lp.len());
            for &v in lp.iter().take(10) {
                let g = info[v as usize];
                eprintln!("   {:?} patch {:?} other {:?}", pos[v as usize], keys.get(g.patch as usize).map(|k| (k.prim, k.copy, k.level)), g.other.map(|k| (k.prim, k.copy, k.level)));
            }
        }
        fill_loop(field, &pos, lp, &mut faces, &mut face_patch, &mut edge_set);
    }
    // repair: faces on non-manifold edges that are slivers or fillers are
    // dropped and the holes they leave filled again
    let mut holes = holes;
    for _ in 0..6 {
        let mut und: HashMap<(u32, u32), u32> = HashMap::new();
        for f in &faces {
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                *und.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let area = |f: &Vec<u32>| -> (f64, f64) {
            let mut n = DVec3::ZERO;
            let mut l = 0.0f64;
            for i in 0..f.len() {
                let (a, b) = (pos[f[i] as usize], pos[f[(i + 1) % f.len()] as usize]);
                n += a.cross(b);
                l = l.max(a.distance(b));
            }
            (n.length() * 0.5, l)
        };
        let mut drop = vec![false; faces.len()];
        let mut any = false;
        for (fi, f) in faces.iter().enumerate() {
            let bad = (0..f.len()).any(|i| {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                und[&(a.min(b), a.max(b))] > 2
            });
            if bad {
                let (ar, l) = area(f);
                if face_patch[fi] == u32::MAX || ar < 1e-3 * l * l {
                    drop[fi] = true;
                    any = true;
                }
            }
        }
        if !any {
            break;
        }
        let mut k = drop.iter();
        faces.retain(|_| !*k.next().unwrap());
        let mut k = drop.iter();
        face_patch.retain(|_| !*k.next().unwrap());
        let mut es: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        for f in &faces {
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                es.insert((a.min(b), a.max(b)));
            }
        }
        let loops: Vec<Vec<u32>> = boundary_loops(&faces, &pos, st.trim_len * 2.0).into_iter().flat_map(split_pinched).collect();
        holes += loops.len();
        for lp in &loops {
            fill_loop(field, &pos, lp, &mut faces, &mut face_patch, &mut es);
        }
    }
    progress.tick();
    // drop unused vertices
    let mut keep = vec![u32::MAX; pos.len()];
    let mut npos = vec![];
    for f in faces.iter_mut() {
        f.dedup();
        while f.len() > 1 && f.first() == f.last() {
            f.pop();
        }
        for v in f.iter_mut() {
            if keep[*v as usize] == u32::MAX {
                keep[*v as usize] = npos.len() as u32;
                npos.push(pos[*v as usize]);
            }
            *v = keep[*v as usize];
        }
    }
    let mut out = Baked {
        positions: npos,
        faces: faces.into_iter().zip(face_patch).filter(|(f, _)| f.len() >= 3).map(|(verts, patch)| BFace { verts, normals: vec![], mat: 0, patch }).collect(),
        stats: BakeStats::default(),
    };
    out.stats.patches = used;
    out.stats.holes_filled = holes;
    out
}

/// Closed boundary loops, oriented in the filling direction.
pub(crate) fn boundary_loops(faces: &[Vec<u32>], pos: &[DVec3], gap: f64) -> Vec<Vec<u32>> {
    let ec = edge_counts(faces);
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&(a, b), _) in ec.iter() {
        if !ec.contains_key(&(b, a)) {
            next.entry(b).or_default().push(a);
        }
    }
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort();
    let mut loops = vec![];
    for s in starts {
        while next.get(&s).map(|l| !l.is_empty()).unwrap_or(false) {
            let mut lp = vec![s];
            let mut cur = s;
            let mut closed = false;
            for _ in 0..1_000_000 {
                let Some(list) = next.get_mut(&cur) else { break };
                let Some(n) = list.pop() else { break };
                if n == s {
                    closed = true;
                    break;
                }
                lp.push(n);
                cur = n;
            }
            // an open chain whose ends are close is closed by a new edge
            let near = lp.len() >= 3 && pos[lp[0] as usize].distance(pos[*lp.last().unwrap() as usize]) < gap;
            if (closed || near) && lp.len() >= 3 {
                loops.push(lp);
            }
        }
    }
    loops
}

/// A loop that passes twice through a vertex is split into simple loops.
pub(crate) fn split_pinched(lp: Vec<u32>) -> Vec<Vec<u32>> {
    let mut out = vec![];
    let mut stack: Vec<u32> = vec![];
    let mut seen: HashMap<u32, usize> = HashMap::new();
    for v in lp {
        if let Some(&i) = seen.get(&v) {
            let sub: Vec<u32> = stack.drain(i..).collect();
            for x in &sub {
                seen.remove(x);
            }
            if sub.len() >= 3 {
                out.push(sub);
            }
        }
        seen.insert(v, stack.len());
        stack.push(v);
    }
    if stack.len() >= 3 {
        out.push(stack);
    }
    out
}

/// Two loops running alongside each other bound one thin band: join them by
/// a bridge so that the band is filled as a single polygon.
fn bridge_parallel(pos: &[DVec3], mut loops: Vec<Vec<u32>>, gap: f64) -> Vec<Vec<u32>> {
    let cell = gap.max(1e-3);
    let key = |p: DVec3| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64, (p.z / cell).floor() as i64);
    loop {
        let mut grid: HashMap<(i64, i64, i64), Vec<(usize, usize)>> = HashMap::new();
        for (li, lp) in loops.iter().enumerate() {
            for (k, &v) in lp.iter().enumerate() {
                grid.entry(key(pos[v as usize])).or_default().push((li, k));
            }
        }
        let mut merged = false;
        'outer: for li in 0..loops.len() {
            if loops[li].len() < 3 {
                continue;
            }
            // which other loop is closest to most of this loop's points
            let mut votes: HashMap<usize, (usize, f64, usize, usize)> = HashMap::new();
            for (k, &v) in loops[li].iter().enumerate() {
                let p = pos[v as usize];
                let c = key(p);
                let mut best: Option<(f64, usize, usize)> = None;
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        for dz in -1..=1 {
                            if let Some(l) = grid.get(&(c.0 + dx, c.1 + dy, c.2 + dz)) {
                                for &(lj, kj) in l {
                                    if lj == li {
                                        continue;
                                    }
                                    let d = pos[loops[lj][kj] as usize].distance(p);
                                    if d < gap && best.map(|b| d < b.0).unwrap_or(true) {
                                        best = Some((d, lj, kj));
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some((d, lj, kj)) = best {
                    let e = votes.entry(lj).or_insert((0, f64::MAX, 0, 0));
                    e.0 += 1;
                    if d < e.1 {
                        *e = (e.0, d, k, kj);
                    }
                }
            }
            for (&lj, &(count, _, ki, kj)) in votes.iter() {
                let small = loops[li].len().min(loops[lj].len());
                if count * 2 >= small.min(loops[li].len()) && count >= 3 {
                    let a = loops[li].clone();
                    let b = loops[lj].clone();
                    let mut m = vec![];
                    for t in 0..=a.len() {
                        m.push(a[(ki + t) % a.len()]);
                    }
                    for t in 0..=b.len() {
                        m.push(b[(kj + t) % b.len()]);
                    }
                    let (hi, lo) = if li > lj { (li, lj) } else { (lj, li) };
                    loops.remove(hi);
                    loops.remove(lo);
                    loops.push(m);
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            return loops;
        }
    }
}

/// Fills a loop with triangles, always cutting the ear with the shortest
/// diagonal among those facing outwards (works for thin bands and bridges).
pub(crate) fn fill_loop(field: &Field, pos: &[DVec3], lp: &[u32], faces: &mut Vec<Vec<u32>>, face_patch: &mut Vec<u32>, edges: &mut std::collections::HashSet<(u32, u32)>) {
    let mut idx: Vec<u32> = lp.to_vec();
    let mut guard = 0;
    while idx.len() > 3 && guard < lp.len() * 4 {
        guard += 1;
        let n = idx.len();
        // remove spikes a b a (left by bridges)
        if let Some(k) = (0..n).find(|&k| idx[(k + n - 1) % n] == idx[(k + 1) % n]) {
            let k2 = (k + 1) % n;
            let (r1, r2) = if k > k2 { (k, k2) } else { (k2, k) };
            idx.remove(r1);
            idx.remove(r2);
            continue;
        }
        let mut best: Option<(f64, usize)> = None;
        let mut fallback: Option<(f64, usize)> = None;
        for k in 0..n {
            let (a, b, c) = (pos[idx[(k + n - 1) % n] as usize], pos[idx[k] as usize], pos[idx[(k + 1) % n] as usize]);
            let diag = a.distance(c);
            let nrm = (b - a).cross(c - b);
            if fallback.map(|f| diag < f.0).unwrap_or(true) {
                fallback = Some((diag, k));
            }
            if nrm.length_squared() < 1e-30 {
                continue;
            }
            if best.map(|f| diag >= f.0).unwrap_or(false) {
                continue;
            }
            // a diagonal that already exists would make a non-manifold edge
            let (ia, ic) = (idx[(k + n - 1) % n], idx[(k + 1) % n]);
            if edges.contains(&(ia.min(ic), ia.max(ic))) && n > 3 {
                continue;
            }
            let g = field.grad((a + b + c) / 3.0);
            if nrm.dot(g) > 0.0 {
                best = Some((diag, k));
            }
        }
        let k = best.or(fallback).unwrap().1;
        let (ia, ic) = (idx[(k + n - 1) % n], idx[(k + 1) % n]);
        edges.insert((ia.min(ic), ia.max(ic)));
        faces.push(vec![idx[(k + n - 1) % n], idx[k], idx[(k + 1) % n]]);
        face_patch.push(u32::MAX);
        idx.remove(k);
    }
    if idx.len() == 3 && idx[0] != idx[1] && idx[1] != idx[2] && idx[0] != idx[2] {
        faces.push(idx);
        face_patch.push(u32::MAX);
    }
}

/// Materials, per-corner normals (split on creases), quality statistics.
pub(crate) fn finish(field: &Field, out: &mut Baked, st: &BakeSettings, progress: &Progress) {
    let pos = out.positions.clone();
    // exact normal once per vertex
    let nv_order: Vec<usize> = (0..pos.len()).collect();
    let vnorm: Vec<DVec3> = par_for(pos.len(), st.threads, &nv_order, |i| field.grad(pos[i]).normalize_or_zero()).into_iter().map(|n| n.unwrap()).collect();
    let n_faces = out.faces.len();
    let order: Vec<usize> = (0..n_faces).collect();
    let faces_ref = &out.faces;
    let res = par_for(n_faces, st.threads, &order, |fi| {
        let f = &faces_ref[fi];
        let m = f.verts.len();
        let c = f.verts.iter().fold(DVec3::ZERO, |s, v| s + pos[*v as usize]) / m as f64;
        let sc = field.eval(c);
        let mat = sc.mat;
        let mut newell = DVec3::ZERO;
        for i in 0..m {
            newell += pos[f.verts[i] as usize].cross(pos[f.verts[(i + 1) % m] as usize]);
        }
        let nface = newell.normalize_or_zero();
        // smooth where the exact normal agrees with the face, else crease
        let normals: Vec<DVec3> = f
            .verts
            .iter()
            .map(|&v| {
                let nv = vnorm[v as usize];
                if nv.dot(nface) > 0.9 || nface == DVec3::ZERO {
                    nv
                } else {
                    nface
                }
            })
            .collect();
        // deviation: face centre and longest edge midpoint against the exact surface
        let mut dev = sc.d.abs();
        let mut best = (0.0, 0);
        for i in 0..m {
            let l = pos[f.verts[i] as usize].distance(pos[f.verts[(i + 1) % m] as usize]);
            if l > best.0 {
                best = (l, i);
            }
        }
        let (a, b) = (pos[f.verts[best.1] as usize], pos[f.verts[(best.1 + 1) % m] as usize]);
        dev = dev.max(field.dist((a + b) * 0.5).abs());
        if fi % 64 == 0 {
            progress.done.fetch_add(64, Ordering::Relaxed);
        }
        (mat, normals, dev)
    });
    let mut sum = 0.0;
    let mut maxd = 0.0f64;
    for (f, r) in out.faces.iter_mut().zip(res) {
        let (mat, normals, dev) = r.unwrap();
        f.mat = mat;
        f.normals = normals;
        sum += dev;
        maxd = maxd.max(dev);
    }
    let s = &mut out.stats;
    s.max_dev = maxd;
    s.mean_dev = sum / n_faces.max(1) as f64;
    s.verts = out.positions.len();
    for f in &out.faces {
        match f.verts.len() {
            3 => s.tris += 1,
            4 => s.quads += 1,
            _ => s.ngons += 1,
        }
    }
    let mut und: HashMap<(u32, u32), u32> = HashMap::new();
    for f in &out.faces {
        let v = &f.verts;
        for i in 0..v.len() {
            let (a, b) = (v[i], v[(i + 1) % v.len()]);
            *und.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    s.open_edges = und.values().filter(|&&c| c == 1).count();
    s.nonmanifold_edges = und.values().filter(|&&c| c > 2).count();
}

impl Baked {
    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.faces.iter().map(|f| f.verts.len().saturating_sub(2)).sum()
    }

    /// Triangles (corner indices of the face) of one polygon.
    pub fn triangulate_face(&self, f: &BFace) -> Vec<[usize; 3]> {
        let m = f.verts.len();
        if m == 3 {
            return vec![[0, 1, 2]];
        }
        let p: Vec<DVec3> = f.verts.iter().map(|v| self.positions[*v as usize]).collect();
        let mut n = DVec3::ZERO;
        for i in 0..m {
            n += p[i].cross(p[(i + 1) % m]);
        }
        let n = n.normalize_or(DVec3::Y);
        let u = n.any_orthonormal_vector();
        let v = n.cross(u);
        let q: Vec<DVec2> = p.iter().map(|x| DVec2::new(x.dot(u), x.dot(v))).collect();
        gen::ear_clip(&q)
    }

    /// Triangle mesh with vertices split on creases and material changes.
    pub fn to_mesh(&self) -> Mesh {
        let mut mesh = Mesh::default();
        let mut map: HashMap<(u32, [i32; 3], u32), u32> = HashMap::new();
        for f in &self.faces {
            let ids: Vec<u32> = f
                .verts
                .iter()
                .zip(&f.normals)
                .map(|(&v, n)| {
                    let q = [(n.x * 1000.0).round() as i32, (n.y * 1000.0).round() as i32, (n.z * 1000.0).round() as i32];
                    *map.entry((v, q, f.mat)).or_insert_with(|| {
                        let p = self.positions[v as usize];
                        mesh.positions.push(Vec3::new(p.x as f32, p.y as f32, p.z as f32));
                        mesh.normals.push(Vec3::new(n.x as f32, n.y as f32, n.z as f32));
                        mesh.materials.push(f.mat);
                        mesh.positions.len() as u32 - 1
                    })
                })
                .collect();
            for t in self.triangulate_face(f) {
                mesh.indices.extend_from_slice(&[ids[t[0]], ids[t[1]], ids[t[2]]]);
            }
        }
        mesh
    }

    /// Wavefront OBJ keeping quads and n-gons, one group per material.
    pub fn to_obj(&self, materials: &[Material], name: &str) -> String {
        use std::fmt::Write as _;
        let mut s = String::with_capacity(self.positions.len() * 90 + self.faces.len() * 40);
        let st = &self.stats;
        let _ = writeln!(s, "# Forme bake — {} faces ({} quads, {} triangles, {} n-gons), {} sommets, unités : mm", self.faces.len(), st.quads, st.tris, st.ngons, st.verts);
        let _ = writeln!(s, "# écart max à la surface exacte : {:.4} mm", st.max_dev);
        let _ = writeln!(s, "o {}", name);
        // vertex colour = material of the first face using the vertex
        let mut vmat = vec![u32::MAX; self.positions.len()];
        for f in &self.faces {
            for &v in &f.verts {
                if vmat[v as usize] == u32::MAX {
                    vmat[v as usize] = f.mat;
                }
            }
        }
        for (p, m) in self.positions.iter().zip(&vmat) {
            let c = materials.get(*m as usize).map(|m| m.color).unwrap_or([0.6, 0.6, 0.6]);
            let _ = writeln!(s, "v {:.5} {:.5} {:.5} {:.4} {:.4} {:.4}", p.x, p.y, p.z, c[0], c[1], c[2]);
        }
        let mut nmap: HashMap<[i32; 3], usize> = HashMap::new();
        let mut nlist: Vec<DVec3> = vec![];
        let mut fn_idx: Vec<Vec<usize>> = vec![];
        for f in &self.faces {
            fn_idx.push(
                f.normals
                    .iter()
                    .map(|n| {
                        let q = [(n.x * 10000.0).round() as i32, (n.y * 10000.0).round() as i32, (n.z * 10000.0).round() as i32];
                        *nmap.entry(q).or_insert_with(|| {
                            nlist.push(*n);
                            nlist.len()
                        })
                    })
                    .collect(),
            );
        }
        for n in &nlist {
            let _ = writeln!(s, "vn {:.4} {:.4} {:.4}", n.x, n.y, n.z);
        }
        let mut by_mat: Vec<usize> = (0..self.faces.len()).collect();
        by_mat.sort_by_key(|&i| self.faces[i].mat);
        let mut cur = u32::MAX;
        for i in by_mat {
            let f = &self.faces[i];
            if f.mat != cur {
                cur = f.mat;
                let name = materials.get(cur as usize).map(|m| m.name.clone()).unwrap_or(format!("mat{cur}"));
                let _ = writeln!(s, "g {}", name.replace(' ', "_"));
            }
            s.push('f');
            for (v, n) in f.verts.iter().zip(&fn_idx[i]) {
                let _ = write!(s, " {}//{}", v + 1, n);
            }
            s.push('\n');
        }
        s
    }
}

/// Convenience used by tests and the capture tool.
pub fn bake_blocking(scene: &Scene, q: Quality) -> Baked {
    bake(scene, &BakeSettings::preset(q), &Progress::default()).unwrap_or_default()
}
