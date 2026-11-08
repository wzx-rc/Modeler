//! One term = one copy of one primitive at one iso level. Its clean
//! parametric mesh is refined only where an ownership change can hide inside a
//! face, every vertex is projected on the final surface, and the faces are
//! clipped exactly along the curves where another term takes over.

use super::field::{prim_project, Field, Instance, PrimDef, TermKey};
use super::gen::PolyMesh;
use glam::DVec3;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
pub struct PatchSettings {
    /// Target edge length along trim curves (mm).
    pub trim_len: f64,
    /// Smallest face size refinement may produce (mm).
    pub min_size: f64,
    pub max_level: u32,
}

#[derive(Clone, Debug)]
pub struct Term {
    pub key: TermKey,
    pub inst: Instance,
    /// Iso level in the primitive's local units.
    pub level_local: f64,
}

#[derive(Clone, Debug, Default)]
pub struct PatchOut {
    pub key: Option<TermKey>,
    pub pos: Vec<DVec3>,
    pub faces: Vec<Vec<u32>>,
    /// For cut vertices: the term on the other side of the trim curve.
    pub other: Vec<Option<TermKey>>,
    pub refined: usize,
    pub cuts: usize,
}

#[derive(Clone, Copy)]
struct V {
    local: DVec3,
    world: DVec3,
    x: DVec3,
    kept: bool,
    /// false: provisional "not kept" (owner at the unprojected point differs)
    exact: bool,
    margin: f64,
    near: f64,
    other: TermKey,
}

/// Another term small enough to hide inside a face: faces overlapping its
/// box are refined below its size, so that their vertices see it.
#[derive(Clone, Copy, Debug)]
pub struct Feature {
    pub lo: DVec3,
    pub hi: DVec3,
    pub size: f64,
}

struct Ctx<'a> {
    field: &'a Field,
    pr: &'a PrimDef,
    term: &'a Term,
    verts: Vec<V>,
    mids: HashMap<(u32, u32), u32>,
    far: f64,
}

impl Ctx<'_> {
    fn classify(&self, local: DVec3) -> V {
        let world = self.term.inst.to_world.transform_point3(local);
        let s0 = self.field.eval(world);
        let fd = s0.d;
        let mut v = V { local, world, x: world, kept: false, exact: true, margin: f64::INFINITY, near: fd.abs(), other: TermKey::NONE };
        if fd.abs() > self.far {
            return v;
        }
        if fd.abs() > 1e-7 && s0.owner.key() != self.term.key {
            v.exact = false;
            v.margin = s0.margin;
            v.other = s0.owner.key();
            return v;
        }
        self.exact(v)
    }

    fn exact(&self, mut v: V) -> V {
        v.exact = true;
        // outside any blend the surface is the primitives' own: no projection
        let s0 = self.field.eval(v.world);
        if s0.blend == 0.0 {
            // no blend reaches this point: kept iff it is on the surface and
            // ours (a hidden or removed point would project too far away)
            let k = s0.owner.key();
            v.x = v.world;
            v.kept = k == self.term.key && s0.d.abs() < 1e-8;
            v.margin = s0.margin;
            v.near = s0.d.abs();
            if !v.kept {
                v.other = k;
            }
            return v;
        }
        if let Some(x) = self.field.project(v.world) {
            let s = self.field.eval(x);
            let k = s.owner.key();
            v.x = x;
            // the projection must stay within the blends' reach: a vertex hidden
            // inside the solid can project onto a visible part of its own surface
            v.kept = k == self.term.key && x.distance(v.world) <= 2.0 * s.blend + 1e-6;
            v.margin = s.margin;
            v.near = x.distance(v.world);
            if !v.kept {
                v.other = if k == self.term.key { self.field.eval(v.world).owner.key() } else { k };
            }
        }
        v
    }

    /// Makes every vertex of a loop exact if the loop touches kept vertices.
    fn settle(&mut self, lp: &[u32]) {
        if lp.iter().any(|&v| self.verts[v as usize].kept) {
            for &v in lp {
                if !self.verts[v as usize].exact {
                    self.verts[v as usize] = self.exact(self.verts[v as usize]);
                }
            }
        }
    }

    fn bbox(&self, lp: &[u32]) -> (DVec3, DVec3) {
        let mut lo = DVec3::splat(f64::MAX);
        let mut hi = DVec3::splat(f64::MIN);
        for &v in lp {
            let p = self.verts[v as usize].world;
            lo = lo.min(p);
            hi = hi.max(p);
        }
        (lo, hi)
    }

    fn add(&mut self, local: DVec3) -> u32 {
        let v = self.classify(local);
        self.verts.push(v);
        self.verts.len() as u32 - 1
    }

    fn mid(&mut self, a: u32, b: u32) -> u32 {
        let key = (a.min(b), a.max(b));
        if let Some(&m) = self.mids.get(&key) {
            return m;
        }
        let l = (self.verts[a as usize].local + self.verts[b as usize].local) * 0.5;
        let l = prim_project(self.pr, l, self.term.level_local);
        let m = self.add(l);
        self.mids.insert(key, m);
        m
    }

    fn side(&self, a: u32, b: u32, out: &mut Vec<u32>) {
        if let Some(&m) = self.mids.get(&(a.min(b), a.max(b))) {
            self.side(a, m, out);
            out.push(m);
            self.side(m, b, out);
        }
    }

    /// Full vertex loop of a face: corners plus the hanging vertices that
    /// finer neighbours inserted on its sides.
    fn loop_of(&self, corners: &[u32]) -> Vec<u32> {
        let mut out = vec![];
        for i in 0..corners.len() {
            out.push(corners[i]);
            self.side(corners[i], corners[(i + 1) % corners.len()], &mut out);
        }
        out
    }

    fn diam(&self, corners: &[u32]) -> f64 {
        let mut d = 0.0f64;
        for i in 0..corners.len() {
            for j in i + 1..corners.len() {
                d = d.max(self.verts[corners[i] as usize].world.distance(self.verts[corners[j] as usize].world));
            }
        }
        d
    }
}

/// `hints`: trim points other patches found against this term; faces around
/// them are refined so that this patch sees the same curve.
pub fn process(field: &Field, term: &Term, base: &PolyMesh, features: &[Feature], hints: &[DVec3], st: &PatchSettings) -> PatchOut {
    let hcell = st.trim_len.max(1e-3);
    let hkey = |p: DVec3| ((p.x / hcell).floor() as i64, (p.y / hcell).floor() as i64, (p.z / hcell).floor() as i64);
    let mut hgrid: HashMap<(i64, i64, i64), Vec<DVec3>> = HashMap::new();
    for h in hints {
        hgrid.entry(hkey(*h)).or_default().push(*h);
    }
    let hint_in = |lo: DVec3, hi: DVec3| -> bool {
        let (a, b) = (hkey(lo), hkey(hi));
        if (b.0 - a.0 + 1) * (b.1 - a.1 + 1) * (b.2 - a.2 + 1) > 4096 {
            return hints.iter().any(|h| h.cmpge(lo).all() && h.cmple(hi).all());
        }
        for x in a.0..=b.0 {
            for y in a.1..=b.1 {
                for z in a.2..=b.2 {
                    if let Some(l) = hgrid.get(&(x, y, z)) {
                        if l.iter().any(|h| h.cmpge(lo).all() && h.cmple(hi).all()) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    };
    let pr = &field.prims[term.key.prim as usize];
    let mut ctx = Ctx { field, pr, term, verts: Vec::with_capacity(base.pos.len() * 2), mids: HashMap::new(), far: field.blend_bound * 1.5 + 1e-3 };
    for p in &base.pos {
        ctx.add(*p);
    }
    let mut out = PatchOut { key: Some(term.key), ..Default::default() };
    let mut faces: Vec<Vec<u32>> = base.faces.clone();
    let mut live: Vec<bool> = vec![true; faces.len()];
    let mut active: Vec<usize> = (0..faces.len()).collect();
    let dbg = std::env::var("BAKE_DBG").ok().and_then(|v| v.parse::<u32>().ok()) == Some(term.key.prim);
    for _level in 0..st.max_level {
        let mut split = vec![];
        let mut why = [0usize; 4];
        for &fi in &active {
            let lp = ctx.loop_of(&faces[fi]);
            ctx.settle(&lp);
            let kept: Vec<bool> = lp.iter().map(|&v| ctx.verts[v as usize].kept).collect();
            let any = kept.iter().any(|k| *k);
            let all = kept.iter().all(|k| *k);
            let diam = ctx.diam(&faces[fi]);
            let mixed = any && !all;
            if !hints.is_empty() && diam > if mixed { st.trim_len } else { st.trim_len * 0.5 } {
                let (lo, hi) = ctx.bbox(&lp);
                let pad = DVec3::splat(st.trim_len * 0.25);
                if hint_in(lo - pad, hi + pad) {
                    split.push(fi);
                    why[1] += 1;
                    continue;
                }
            }
            if any && !all {
                let transitions = (0..kept.len()).filter(|&i| kept[i] != kept[(i + 1) % kept.len()]).count();
                if (diam > st.trim_len || transitions > 2) && diam > st.min_size {
                    split.push(fi);
                    why[if transitions > 2 && diam <= st.trim_len { 1 } else { 0 }] += 1;
                }
                continue;
            }
            if diam <= st.trim_len * 0.5 {
                continue;
            }
            let near = lp.iter().map(|&v| ctx.verts[v as usize].near).fold(f64::INFINITY, f64::min);
            if near > diam {
                continue;
            }
            // a smaller term overlapping the face could hide between its vertices
            let (lo, hi) = ctx.bbox(&lp);
            if features.iter().any(|f| diam * 1.5 > f.size && f.lo.cmple(hi).all() && lo.cmple(f.hi).all()) {
                split.push(fi);
                why[2] += 1;
                continue;
            }
            // probe the centre of faces that touch the kept region
            if any && diam > st.trim_len {
                let mut l = DVec3::ZERO;
                for &v in &faces[fi] {
                    l += ctx.verts[v as usize].local;
                }
                let l = prim_project(pr, l / faces[fi].len() as f64, term.level_local);
                let c = ctx.exact(ctx.classify(l));
                if !c.kept {
                    split.push(fi);
                    why[3] += 1;
                }
            }
        }
        if dbg {
            eprintln!("  copy {} level {_level}: active {} split {} (mixed big {}, mixed multi {}, feature {}, centre {})", term.key.copy, active.len(), split.len(), why[0], why[1], why[2], why[3]);
        }
        if split.is_empty() {
            break;
        }
        out.refined += split.len();
        let mut next = vec![];
        for fi in split {
            let c = faces[fi].clone();
            live[fi] = false;
            let n = c.len();
            let side = |ctx: &Ctx, i: usize| ctx.verts[c[i] as usize].world.distance(ctx.verts[c[(i + 1) % n] as usize].world);
            let mut children: Vec<Vec<u32>> = vec![];
            let tri_aniso = n == 3 && {
                let l = [side(&ctx, 0), side(&ctx, 1), side(&ctx, 2)];
                l.iter().cloned().fold(0.0, f64::max) > 2.5 * l.iter().cloned().fold(f64::MAX, f64::min)
            };
            if n == 4 && (side(&ctx, 0) + side(&ctx, 2)) > 2.0 * (side(&ctx, 1) + side(&ctx, 3)) {
                // anisotropic faces are cut across their long direction only
                let (m0, m2) = (ctx.mid(c[0], c[1]), ctx.mid(c[2], c[3]));
                children.push(vec![c[0], m0, m2, c[3]]);
                children.push(vec![m0, c[1], c[2], m2]);
            } else if n == 4 && (side(&ctx, 1) + side(&ctx, 3)) > 2.0 * (side(&ctx, 0) + side(&ctx, 2)) {
                let (m1, m3) = (ctx.mid(c[1], c[2]), ctx.mid(c[3], c[0]));
                children.push(vec![c[0], c[1], m1, m3]);
                children.push(vec![m3, m1, c[2], c[3]]);
            } else if tri_aniso {
                let k = (0..3).max_by(|&a, &b| side(&ctx, a).partial_cmp(&side(&ctx, b)).unwrap()).unwrap();
                let (a, b, o) = (c[k], c[(k + 1) % 3], c[(k + 2) % 3]);
                let m = ctx.mid(a, b);
                children.push(vec![a, m, o]);
                children.push(vec![m, b, o]);
            } else {
                let m: Vec<u32> = (0..n).map(|i| ctx.mid(c[i], c[(i + 1) % n])).collect();
                if n == 3 {
                    children.push(vec![c[0], m[0], m[2]]);
                    children.push(vec![m[0], c[1], m[1]]);
                    children.push(vec![m[2], m[1], c[2]]);
                    children.push(vec![m[0], m[1], m[2]]);
                } else {
                    let mut l = DVec3::ZERO;
                    for v in &c {
                        l += ctx.verts[*v as usize].local;
                    }
                    let l = prim_project(pr, l / n as f64, term.level_local);
                    let ctr = ctx.add(l);
                    for k in 0..n {
                        children.push(vec![c[k], m[k], ctr, m[(k + n - 1) % n]]);
                    }
                }
            }
            for ch in children {
                faces.push(ch);
                live.push(true);
                next.push(faces.len() - 1);
            }
        }
        active = next;
    }
    if !ctx.verts.iter().any(|v| v.kept) {
        return out;
    }
    // clip every live face along the ownership boundary
    let mut cuts: HashMap<(u32, u32), u32> = HashMap::new();
    let mut extra: Vec<(DVec3, TermKey)> = vec![];
    let mut snapped: HashMap<u32, TermKey> = HashMap::new();
    let nv = ctx.verts.len() as u32;
    let mut kept_faces: Vec<Vec<u32>> = vec![];
    for fi in 0..faces.len() {
        if !live[fi] {
            continue;
        }
        let lp = ctx.loop_of(&faces[fi]);
        ctx.settle(&lp);
        let kept: Vec<bool> = lp.iter().map(|&v| ctx.verts[v as usize].kept).collect();
        if kept.iter().all(|k| *k) {
            kept_faces.push(lp);
            continue;
        }
        if !kept.iter().any(|k| *k) {
            continue;
        }
        let mut poly = vec![];
        let n = lp.len();
        for i in 0..n {
            let (a, b) = (lp[i], lp[(i + 1) % n]);
            let (ka, kb) = (kept[i], kept[(i + 1) % n]);
            if ka {
                poly.push(a);
            }
            if ka != kb {
                let key = (a.min(b), a.max(b));
                let id = *cuts.entry(key).or_insert_with(|| {
                    let (k, o) = if ka { (a, b) } else { (b, a) };
                    let (x, other) = cut_point(field, pr, term, &ctx.verts[k as usize], &ctx.verts[o as usize]);
                    if x.distance(ctx.verts[k as usize].x) < 1e-6 {
                        // the kept vertex sits on the trim curve itself
                        snapped.insert(k, other);
                        return k;
                    }
                    extra.push((x, other));
                    nv + extra.len() as u32 - 1
                });
                if poly.last() != Some(&id) {
                    poly.push(id);
                }
            }
        }
        while poly.len() > 1 && poly.first() == poly.last() {
            poly.pop();
        }
        if poly.len() >= 3 {
            kept_faces.push(poly);
        }
    }
    out.cuts = extra.len();
    // compact
    let mut remap: HashMap<u32, u32> = HashMap::new();
    for f in &mut kept_faces {
        for v in f.iter_mut() {
            let id = *remap.entry(*v).or_insert_with(|| {
                if *v < nv {
                    out.pos.push(ctx.verts[*v as usize].x);
                    out.other.push(snapped.get(v).copied());
                } else {
                    let (x, o) = extra[(*v - nv) as usize];
                    out.pos.push(x);
                    out.other.push(Some(o));
                }
                out.pos.len() as u32 - 1
            });
            *v = id;
        }
    }
    out.faces = kept_faces;
    out
}

/// Exact point of the trim curve on the segment kept → other (bisection on
/// ownership, every probe projected on the surface).
fn cut_point(field: &Field, pr: &PrimDef, term: &Term, k: &V, o: &V) -> (DVec3, TermKey) {
    let (pk, po) = (k.world, o.world);
    let mut lo = 0.0f64;
    let mut hi = 1.0f64;
    let mut best = k.x;
    let mut offset = k.x - pk;
    let mut other = o.other;
    let len = pk.distance(po).max(1e-9);
    let iters = ((len / 1e-7).log2().ceil() as usize).clamp(8, 40);
    for _ in 0..iters {
        let t = 0.5 * (lo + hi);
        let l = prim_project(pr, k.local.lerp(o.local, t), term.level_local);
        let p = term.inst.to_world.transform_point3(l);
        let s0 = field.eval(p);
        if s0.blend == 0.0 {
            // sharp region: the probe is kept iff it is on the surface and ours
            let key = s0.owner.key();
            if key == term.key && s0.d.abs() < 1e-8 {
                lo = t;
                best = p;
                offset = DVec3::ZERO;
            } else {
                hi = t;
                other = key;
            }
            continue;
        }
        match field.project(p + offset).or_else(|| field.project(p)) {
            Some(x) => {
                let s = field.eval(x);
                let key = s.owner.key();
                if key == term.key && x.distance(p) <= 2.0 * s.blend + 1e-6 {
                    lo = t;
                    best = x;
                    offset = x - p;
                } else {
                    hi = t;
                    // a probe hidden inside another term projects back onto
                    // its own surface: the neighbour is the owner at the probe
                    other = if key == term.key { field.eval(p).owner.key() } else { key };
                }
            }
            None => hi = t,
        }
    }
    if std::env::var("CUT_DBG").is_ok() && lo > 0.999 {
        eprintln!("cut: no transition k {:?} o {:?} o.exact {} o.kept {} o.near {:.3}", pk, po, o.exact, o.kept, o.near);
    }
    (best, other)
}
