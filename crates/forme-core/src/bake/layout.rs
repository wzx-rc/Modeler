//! Bake "par lignes": the mesh is deduced from the construction lines.
//!
//! 1. Every face of every shape that shows on the final surface becomes a
//!    chart: a flat face of a box is a rectangle in its plane, a rounded
//!    edge is a strip (along the edge × across its arc). The other shapes
//!    cut the chart along their own faces: these cuts are the construction
//!    lines (junctions, rails), found exactly in 2D. Two charts share each
//!    line, so both sides of a line get the same vertices.
//! 2. Fillets first: the number of rows across each strip is fixed (bevel
//!    segments), the strips are drawn orange in the role view.
//! 3. Loops: from each re-entrant corner (sharp, or the middle of a concave
//!    fillet) lines run straight across the face and on around the part,
//!    face after face, until they come back: the edge loops a modeller cuts.
//! 4. Each face is now cut into four-sided patches. The number of segments
//!    of every line is solved so that opposite sides of each patch agree,
//!    then every patch is filled with a grid of quads (transfinite
//!    interpolation in its chart), every vertex exactly on its face.
//!
//! Handled for now: boxes with sharp or rounded edges, combined by sharp
//! Add / Carve. Anything else returns Err and the caller falls back.

use super::build::{P_FILLET, P_LOFT};
use super::field::{box_edge_signs, prim_raw, Field, Instance, PrimDef, Step};
use super::lines::edge_r;
use super::{finish, BFace, BakeSettings, Baked, Progress};
use crate::program::compile;
use crate::scene::Scene;
use glam::{DVec2, DVec3};
use std::collections::{HashMap, VecDeque};
use std::f64::consts::FRAC_PI_2;
use std::time::Instant;

/// Faces of a box: 0..5 flat (axis * 2 + negative side), 6 + e rounded edge e.
const FILLET0: u32 = 6;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
struct FaceId {
    prim: u32,
    face: u32,
}

/// One step of the history: a box placed in the world.
struct Shape<'a> {
    pr: &'a PrimDef,
    inst: Instance,
    prim: u32,
    step: usize,
    carve: bool,
}

impl Shape<'_> {
    fn local(&self, p: DVec3) -> DVec3 {
        self.inst.to_local.transform_point3(p)
    }
    fn d(&self, p: DVec3) -> f64 {
        prim_raw(self.pr, self.local(p)) * self.pr.sc
    }
    /// Face of the box under `p` (a point on its surface).
    fn face_at(&self, p: DVec3) -> u32 {
        let l = self.local(p);
        let mut best = (f64::INFINITY, 0u32);
        for f in 0..6u32 {
            let v = self.face_fn_local(f, l).abs();
            if v < best.0 {
                best = (v, f);
            }
        }
        let s = self.pr.size;
        for e in 0..12usize {
            let r = edge_r(self.pr, e);
            if r <= 0.0 {
                continue;
            }
            let (ax, sb, sc) = box_edge_signs(e);
            let (b, c) = ((ax + 1) % 3, (ax + 2) % 3);
            if l[b] * sb >= s[b] - r && l[c] * sc >= s[c] - r {
                let v = self.face_fn_local(FILLET0 + e as u32, l).abs();
                if v < best.0 {
                    best = (v, FILLET0 + e as u32);
                }
            }
        }
        best.1
    }
    /// Signed distance (local units) to the surface carrying face `f`.
    fn face_fn_local(&self, f: u32, l: DVec3) -> f64 {
        let s = self.pr.size;
        if f < FILLET0 {
            let ax = (f / 2) as usize;
            let sg = if f % 2 == 0 { 1.0 } else { -1.0 };
            l[ax] * sg - s[ax]
        } else {
            let e = (f - FILLET0) as usize;
            let r = edge_r(self.pr, e);
            let (ax, sb, sc) = box_edge_signs(e);
            let (b, c) = ((ax + 1) % 3, (ax + 2) % 3);
            DVec2::new(l[b] - sb * (s[b] - r), l[c] - sc * (s[c] - r)).length() - r
        }
    }
    fn face_fn(&self, f: u32, p: DVec3) -> f64 {
        self.face_fn_local(f, self.local(p)) * self.pr.sc
    }
}

fn edge_index(axis: usize, s1: f64, s2: f64) -> usize {
    axis * 4
        + match (s1 >= 0.0, s2 >= 0.0) {
            (true, true) => 0,
            (false, true) => 1,
            (false, false) => 2,
            (true, false) => 3,
        }
}

fn unit(ax: usize) -> DVec3 {
    let mut v = DVec3::ZERO;
    v[ax] = 1.0;
    v
}

fn perp(v: DVec2) -> DVec2 {
    DVec2::new(-v.y, v.x)
}

/// Signed turn from direction `a` to `b` (left positive).
fn turn(a: DVec2, b: DVec2) -> f64 {
    a.perp_dot(b).atan2(a.dot(b))
}

/// Flat chart (world mm in the face plane) or strip chart (along the edge,
/// arc length across).
#[derive(Clone, Copy)]
enum Map {
    Plane { o: DVec3, u: DVec3, v: DVec3 },
    Cyl { o: DVec3, a: DVec3, b: DVec3, c: DVec3, r: f64 },
}

impl Map {
    fn to3(&self, q: DVec2) -> DVec3 {
        match *self {
            Map::Plane { o, u, v } => o + u * q.x + v * q.y,
            Map::Cyl { o, a, b, c, r } => {
                let th = q.y / r;
                o + a * q.x + (b * th.cos() + c * th.sin()) * r
            }
        }
    }
    fn to2(&self, p: DVec3) -> DVec2 {
        match *self {
            Map::Plane { o, u, v } => {
                let d = p - o;
                DVec2::new(d.dot(u), d.dot(v))
            }
            Map::Cyl { o, a, b, c, r } => {
                let d = p - o;
                DVec2::new(d.dot(a), d.dot(c).atan2(d.dot(b)) * r)
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Cons {
    /// Side of the chart rectangle: 0 bottom, 1 right, 2 top, 3 left.
    Rect(usize),
    /// Rounded corner of a flat face (edge square to the face).
    Arc(usize),
    /// Must lie outside (Add) or inside (Carve) the model before the step.
    Prev,
    /// Must lie outside a later shape.
    Later(usize),
}

struct Chart {
    id: FaceId,
    shape: usize,
    map: Map,
    lo: DVec2,
    hi: DVec2,
    side: [FaceId; 4],
    /// (centre, radius, quadrant signs, rounded edge face)
    arcs: Vec<(DVec2, f64, DVec2, FaceId)>,
    /// Strip: radius and rows across.
    strip: Option<(f64, usize)>,
    cons: Vec<Cons>,
}

impl Chart {
    fn row(&self) -> f64 {
        match self.strip {
            Some((r, segs)) => r * FRAC_PI_2 / segs as f64,
            None => 0.0,
        }
    }
}

/// A line between two nodes, shared by two charts (or inside one: a loop).
#[derive(Clone)]
struct GEdge {
    a: usize,
    b: usize,
    pts: Vec<DVec3>,
    charts: [usize; 2],
    ray: bool,
    alive: bool,
}

struct Layout<'a> {
    field: &'a Field,
    shapes: Vec<Shape<'a>>,
    prev: Vec<Option<Field>>,
    charts: Vec<Chart>,
    nodes: Vec<DVec3>,
    edges: Vec<GEdge>,
    /// Per chart: closed loops of (edge, forward), region on the left.
    bnd: Vec<Vec<Vec<(usize, bool)>>>,
    /// Per chart: loop edges inside it.
    rays: Vec<Vec<usize>>,
    /// Per chart: loop nodes inside the face (diagonal hooks).
    inner: Vec<Vec<usize>>,
    origins: Vec<usize>,
    node_tol: f64,
}

impl Layout<'_> {
    fn shape_of(&self, prim: u32) -> Option<&Shape<'_>> {
        self.shapes.iter().find(|s| s.prim == prim)
    }

    fn cval(&self, ch: &Chart, k: Cons, q: DVec2) -> f64 {
        match k {
            Cons::Rect(0) => q.y - ch.lo.y,
            Cons::Rect(1) => ch.hi.x - q.x,
            Cons::Rect(2) => ch.hi.y - q.y,
            Cons::Rect(_) => q.x - ch.lo.x,
            Cons::Arc(i) => {
                let (c, r, sg, _) = ch.arcs[i];
                let w = ((q - c) * sg).max(DVec2::ZERO);
                r - w.length()
            }
            Cons::Prev => {
                let sh = &self.shapes[ch.shape];
                let d = self.prev[ch.shape].as_ref().map(|f| f.dist(ch.map.to3(q))).unwrap_or(1e9);
                if sh.carve {
                    -d
                } else {
                    d
                }
            }
            Cons::Later(j) => self.shapes[j].d(ch.map.to3(q)),
        }
    }

    /// Smallest constraint (the region is where it is positive).
    fn fmin(&self, ch: &Chart, q: DVec2) -> (f64, Cons) {
        let mut best = (f64::INFINITY, Cons::Rect(0));
        for &k in &ch.cons {
            let v = self.cval(ch, k, q);
            if v < best.0 {
                best = (v, k);
            }
        }
        best
    }

    fn label(&self, ch: &Chart, k: Cons, q: DVec2) -> FaceId {
        match k {
            Cons::Rect(s) => ch.side[s],
            Cons::Arc(i) => ch.arcs[i].3,
            Cons::Prev => {
                let p = ch.map.to3(q);
                let me = &self.shapes[ch.shape];
                let s = self.shapes.iter().filter(|s| s.step < me.step).min_by(|a, b| a.d(p).abs().total_cmp(&b.d(p).abs())).unwrap();
                FaceId { prim: s.prim, face: s.face_at(p) }
            }
            Cons::Later(j) => {
                let p = ch.map.to3(q);
                FaceId { prim: self.shapes[j].prim, face: self.shapes[j].face_at(p) }
            }
        }
    }

    /// The function whose zero set carries a piece of boundary.
    fn psi(&self, ch: &Chart, k: Cons, l: FaceId, q: DVec2) -> f64 {
        match k {
            Cons::Rect(_) | Cons::Arc(_) => self.cval(ch, k, q),
            _ => self.shape_of(l.prim).map(|s| s.face_fn(l.face, ch.map.to3(q))).unwrap_or(0.0),
        }
    }

    fn node(&mut self, p: DVec3) -> usize {
        for (i, n) in self.nodes.iter().enumerate() {
            if n.distance(p) < self.node_tol {
                return i;
            }
        }
        self.nodes.push(p);
        self.nodes.len() - 1
    }

    fn pts2(&self, e: usize, c: usize) -> Vec<DVec2> {
        let m = self.charts[c].map;
        self.edges[e].pts.iter().map(|p| m.to2(*p)).collect()
    }

    /// Polyline of a half-edge in chart `c`, from its start.
    fn hpts(&self, e: usize, fwd: bool, c: usize) -> Vec<DVec2> {
        let mut p = self.pts2(e, c);
        if !fwd {
            p.reverse();
        }
        p
    }

    fn start_dir(&self, e: usize, fwd: bool, c: usize) -> DVec2 {
        let p = self.hpts(e, fwd, c);
        let k = p.len().min(3) - 1;
        (p[k.max(1).min(p.len() - 1)] - p[0]).normalize_or_zero()
    }

    fn end_dir(&self, e: usize, fwd: bool, c: usize) -> DVec2 {
        let p = self.hpts(e, fwd, c);
        let n = p.len();
        let k = n.saturating_sub(3).min(n - 2);
        (p[n - 1] - p[k]).normalize_or_zero()
    }

    /// Splits edge `e` at the 3D point `x` (on it); returns the node.
    fn split(&mut self, e: usize, x: DVec3) -> usize {
        let n = self.node(x);
        let ed = self.edges[e].clone();
        if n == ed.a || n == ed.b {
            return n;
        }
        // closest segment
        let mut best = (f64::INFINITY, 0usize);
        for i in 0..ed.pts.len() - 1 {
            let (a, b) = (ed.pts[i], ed.pts[i + 1]);
            let ab = b - a;
            let t = ((x - a).dot(ab) / ab.length_squared().max(1e-30)).clamp(0.0, 1.0);
            let d = (a + ab * t).distance(x);
            if d < best.0 {
                best = (d, i);
            }
        }
        let i = best.1;
        let mut p1: Vec<DVec3> = ed.pts[..=i].to_vec();
        p1.push(self.nodes[n]);
        let mut p2 = vec![self.nodes[n]];
        p2.extend_from_slice(&ed.pts[i + 1..]);
        // drop points that coincide with the new node
        if p1.len() > 2 && p1[p1.len() - 2].distance(self.nodes[n]) < self.node_tol {
            p1.remove(p1.len() - 2);
        }
        if p2.len() > 2 && p2[1].distance(self.nodes[n]) < self.node_tol {
            p2.remove(1);
        }
        let e1 = self.edges.len();
        self.edges.push(GEdge { a: ed.a, b: n, pts: p1, ..ed.clone() });
        let e2 = self.edges.len();
        self.edges.push(GEdge { a: n, b: ed.b, pts: p2, ..ed.clone() });
        self.edges[e].alive = false;
        let mut cs = vec![ed.charts[0]];
        if ed.charts[1] != ed.charts[0] {
            cs.push(ed.charts[1]);
        }
        for c in cs {
            for lp in &mut self.bnd[c] {
                if let Some(k) = lp.iter().position(|x| x.0 == e) {
                    let fwd = lp[k].1;
                    let rep = if fwd { [(e1, true), (e2, true)] } else { [(e2, false), (e1, false)] };
                    lp.splice(k..=k, rep);
                }
            }
            if let Some(k) = self.rays[c].iter().position(|&x| x == e) {
                self.rays[c].splice(k..=k, [e1, e2]);
            }
        }
        n
    }
}

/// Gradient of a 2D function by central differences.
fn grad2(f: &dyn Fn(DVec2) -> f64, q: DVec2, e: f64) -> DVec2 {
    DVec2::new(f(q + DVec2::new(e, 0.0)) - f(q - DVec2::new(e, 0.0)), f(q + DVec2::new(0.0, e)) - f(q - DVec2::new(0.0, e))) / (2.0 * e)
}

/// Newton projection on the zero set of `f`.
fn proj1(f: &dyn Fn(DVec2) -> f64, mut q: DVec2, e: f64) -> DVec2 {
    for _ in 0..40 {
        let v = f(q);
        if v.abs() < 1e-12 {
            break;
        }
        let g = grad2(f, q, e);
        let gg = g.length_squared();
        if gg < 1e-20 {
            break;
        }
        q -= g * (v / gg);
    }
    q
}

/// Where two curves cross, when they cross clearly (not tangent).
fn solve_cross(f: &dyn Fn(DVec2) -> f64, g: &dyn Fn(DVec2) -> f64, q0: DVec2, e: f64) -> Option<DVec2> {
    let mut q = q0;
    for _ in 0..50 {
        let (a, b) = (f(q), g(q));
        if a.abs() < 1e-11 && b.abs() < 1e-11 {
            return Some(q);
        }
        let ga = grad2(f, q, e);
        let gb = grad2(g, q, e);
        let det = ga.perp_dot(gb);
        if det.abs() < 0.1 * ga.length() * gb.length() {
            return None;
        }
        q += DVec2::new((-a * gb.y + b * ga.y) / det, (-b * ga.x + a * gb.x) / det);
    }
    (f(q).abs() < 1e-8 && g(q).abs() < 1e-8).then_some(q)
}

/// Closed boundary loops of {f > 0} on a grid (marching squares), region
/// on the left.
fn march(f: &dyn Fn(DVec2) -> f64, lo: DVec2, hi: DVec2, step: f64) -> Vec<Vec<DVec2>> {
    let size = hi - lo;
    let nx = ((size.x / step).ceil() as usize).clamp(2, 1500) + 3;
    let ny = ((size.y / step).ceil() as usize).clamp(2, 1500) + 3;
    let dx = size.x / (nx - 3) as f64;
    let dy = size.y / (ny - 3) as f64;
    let g0 = lo - DVec2::new(1.5 * dx, 1.5 * dy);
    let at = |i: usize, j: usize| g0 + DVec2::new(i as f64 * dx, j as f64 * dy);
    let mut val = vec![0.0f64; (nx + 1) * (ny + 1)];
    for j in 0..=ny {
        for i in 0..=nx {
            val[j * (nx + 1) + i] = f(at(i, j));
        }
    }
    let v = |i: usize, j: usize| val[j * (nx + 1) + i];
    let inside = |i: usize, j: usize| v(i, j) > 0.0;
    // edge ids: horizontal (i,j)-(i+1,j) = 2*(j*(nx+1)+i), vertical (i,j)-(i,j+1) = +1
    let hid = |i: usize, j: usize| 2 * (j * (nx + 1) + i);
    let vid = |i: usize, j: usize| 2 * (j * (nx + 1) + i) + 1;
    let mut next: HashMap<usize, usize> = HashMap::new();
    for j in 0..ny {
        for i in 0..nx {
            let c = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
            let ins: Vec<bool> = c.iter().map(|&(a, b)| inside(a, b)).collect();
            let eid = [hid(i, j), vid(i + 1, j), hid(i, j + 1), vid(i, j)];
            let exits: Vec<usize> = (0..4).filter(|&k| ins[k] && !ins[(k + 1) % 4]).collect();
            let entries: Vec<usize> = (0..4).filter(|&k| !ins[k] && ins[(k + 1) % 4]).collect();
            if exits.len() == 1 {
                next.insert(eid[exits[0]], eid[entries[0]]);
            } else if exits.len() == 2 {
                let centre = (v(i, j) + v(i + 1, j) + v(i + 1, j + 1) + v(i, j + 1)) * 0.25 > 0.0;
                for &x in &exits {
                    // centre inside: cut the outside corner after the exit
                    let en = if centre { (x + 1) % 4 } else { (x + 3) % 4 };
                    next.insert(eid[x], eid[en]);
                }
            }
        }
    }
    let point = |id: usize| -> DVec2 {
        let k = id / 2;
        let (i, j) = (k % (nx + 1), k / (nx + 1));
        let (a, b) = if id % 2 == 0 { ((i, j), (i + 1, j)) } else { ((i, j), (i, j + 1)) };
        let (pa, pb) = (at(a.0, a.1), at(b.0, b.1));
        let (mut lo_t, mut hi_t) = (0.0, 1.0);
        let a_in = inside(a.0, a.1);
        for _ in 0..40 {
            let m = (lo_t + hi_t) * 0.5;
            if (f(pa.lerp(pb, m)) > 0.0) == a_in {
                lo_t = m;
            } else {
                hi_t = m;
            }
        }
        pa.lerp(pb, (lo_t + hi_t) * 0.5)
    };
    let mut out = vec![];
    let mut used: HashMap<usize, bool> = HashMap::new();
    let mut keys: Vec<usize> = next.keys().copied().collect();
    keys.sort();
    for s in keys {
        if used.contains_key(&s) {
            continue;
        }
        let mut lp = vec![];
        let mut cur = s;
        loop {
            used.insert(cur, true);
            lp.push(point(cur));
            match next.get(&cur) {
                Some(&n) if n == s => break,
                Some(&n) if !used.contains_key(&n) => cur = n,
                _ => {
                    lp.clear();
                    break;
                }
            }
        }
        if lp.len() >= 3 {
            out.push(lp);
        }
    }
    out
}

/// Settings of the line bake.
#[derive(Clone, Copy, Debug)]
pub struct LayoutSettings {
    /// Target edge length (mm).
    pub size: f64,
    /// Chord tolerance on fillets (mm).
    pub tol: f64,
    pub threads: usize,
    /// Concave fillets hook into the face's grid by a diagonal (Romain's
    /// choice) instead of running their rows straight across the face.
    pub diagonal: bool,
}

/// Rows across a fillet of radius `r` and opening `angle`, even, at least 2.
fn fillet_segs(r: f64, angle: f64, tol: f64) -> usize {
    let step = 2.0 * (1.0 - (tol / r).min(0.9)).acos();
    let n = ((angle / step.max(1e-3)).ceil() as usize).max(2);
    n + n % 2
}

/// Whether the history is one this bake handles (Err: why not).
pub fn handles(scene: &Scene) -> Result<(), String> {
    let field = Field::new(&compile(scene));
    let steps: Vec<Step> = field.steps().ok_or("historique en arbre")?;
    if steps.is_empty() {
        return Err("scène vide".into());
    }
    check(&field, &steps)
}

fn check(field: &Field, steps: &[Step]) -> Result<(), String> {
    for (i, s) in steps.iter().enumerate() {
        let pr = &field.prims[s.prim as usize];
        if pr.kind != 0 || pr.taper.abs() > 1e-6 || pr.shell > 0.0 {
            return Err("par lignes : seulement des boîtes pour l'instant".into());
        }
        if field.instances(s.prim).len() != 1 {
            return Err("par lignes : copies pas encore gérées".into());
        }
        if i > 0 && (s.mode > 1 || s.chamfer || s.k > 1e-9) {
            return Err("par lignes : seulement Add / Carve sans fillet de jonction pour l'instant".into());
        }
        if (0..12).any(|e| edge_r(pr, e) < 0.0) {
            return Err("par lignes : chanfreins pas encore gérés".into());
        }
    }
    Ok(())
}

/// Bake by construction lines.
pub fn build(scene: &Scene, st: &LayoutSettings, progress: &Progress) -> Result<Baked, String> {
    let t0 = Instant::now();
    let field = Field::new(&compile(scene));
    let steps: Vec<Step> = field.steps().ok_or("historique en arbre")?;
    if steps.is_empty() {
        return Err("scène vide".into());
    }
    let (lo, hi) = scene.bounds();
    let extent = if lo.x <= hi.x { ((hi - lo).max_element() as f64).max(1.0) } else { 20.0 };
    check(&field, &steps)?;
    // shapes, one per step
    let mut shapes = vec![];
    for (i, s) in steps.iter().enumerate() {
        let inst = field.instances(s.prim);
        shapes.push(Shape { pr: &field.prims[s.prim as usize], inst: inst[0], prim: s.prim, step: i, carve: i > 0 && s.mode == 1 });
    }
    let prev: Vec<Option<Field>> = (0..shapes.len()).map(|i| (i > 0).then(|| field.prefix(&steps, i))).collect();
    let tol_f = (st.tol * 2.0).max(0.02);

    // charts: flat faces and rounded edges of every shape
    let mut charts: Vec<Chart> = vec![];
    for (si, sh) in shapes.iter().enumerate() {
        let pr = sh.pr;
        let s = pr.size;
        let sc = pr.sc;
        let tw = sh.inst.to_world;
        let rr = |e: usize| edge_r(pr, e).max(0.0);
        let mut cons_base = vec![Cons::Rect(0), Cons::Rect(1), Cons::Rect(2), Cons::Rect(3)];
        if si > 0 {
            cons_base.push(Cons::Prev);
        }
        for j in (si + 1)..shapes.len() {
            cons_base.push(Cons::Later(j));
        }
        for a in 0..3usize {
            for sg in [1.0f64, -1.0] {
                let (b, c) = ((a + 1) % 3, (a + 2) % 3);
                let face = (a * 2 + (sg < 0.0) as usize) as u32;
                let o = tw.transform_point3(unit(a) * (sg * s[a]));
                let u = tw.transform_vector3(unit(b)).normalize();
                let v = tw.transform_vector3(unit(c)).normalize();
                // edge parallel to `along`, on side `so` of axis `other`, on this face
                let side_edge = |along: usize, other: usize, so: f64| -> usize {
                    let mut sv = DVec3::ZERO;
                    sv[a] = sg;
                    sv[other] = so;
                    edge_index(along, sv[(along + 1) % 3], sv[(along + 2) % 3])
                };
                let es = [side_edge(b, c, -1.0), side_edge(c, b, 1.0), side_edge(b, c, 1.0), side_edge(c, b, -1.0)];
                let axes = [(c, -1.0), (b, 1.0), (c, 1.0), (b, -1.0)];
                let mut side = [FaceId { prim: sh.prim, face: 0 }; 4];
                for k in 0..4 {
                    side[k] = if rr(es[k]) > 0.0 {
                        FaceId { prim: sh.prim, face: FILLET0 + es[k] as u32 }
                    } else {
                        let (ax, so) = axes[k];
                        FaceId { prim: sh.prim, face: (ax * 2 + (so < 0.0) as usize) as u32 }
                    };
                }
                let lo = DVec2::new(-s[b] + rr(es[3]), -s[c] + rr(es[0])) * sc;
                let hi = DVec2::new(s[b] - rr(es[1]), s[c] - rr(es[2])) * sc;
                let mut arcs = vec![];
                for (sb, scn) in [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
                    let e = edge_index(a, sb, scn);
                    let r = rr(e);
                    if r > 0.0 {
                        arcs.push((DVec2::new(sb * (s[b] - r), scn * (s[c] - r)) * sc, r * sc, DVec2::new(sb, scn), FaceId { prim: sh.prim, face: FILLET0 + e as u32 }));
                    }
                }
                let mut cons = cons_base.clone();
                for i in 0..arcs.len() {
                    cons.push(Cons::Arc(i));
                }
                charts.push(Chart { id: FaceId { prim: sh.prim, face }, shape: si, map: Map::Plane { o, u, v }, lo, hi, side, arcs, strip: None, cons });
            }
        }
        for e in 0..12usize {
            let r = rr(e);
            if r <= 0.0 {
                continue;
            }
            let (ax, sb, scn) = box_edge_signs(e);
            let (b, c) = ((ax + 1) % 3, (ax + 2) % 3);
            let mut cl = DVec3::ZERO;
            cl[b] = sb * (s[b] - r);
            cl[c] = scn * (s[c] - r);
            let o = tw.transform_point3(cl);
            let am = tw.transform_vector3(unit(ax)).normalize();
            let bm = tw.transform_vector3(unit(b) * sb).normalize();
            let cm = tw.transform_vector3(unit(c) * scn).normalize();
            let rad = r * sc;
            let side = [
                FaceId { prim: sh.prim, face: (b * 2 + (sb < 0.0) as usize) as u32 },
                FaceId { prim: sh.prim, face: (ax * 2) as u32 },
                FaceId { prim: sh.prim, face: (c * 2 + (scn < 0.0) as usize) as u32 },
                FaceId { prim: sh.prim, face: (ax * 2 + 1) as u32 },
            ];
            let segs = fillet_segs(rad, FRAC_PI_2, tol_f).max(((rad * FRAC_PI_2 / st.size).round() as usize + 1) / 2 * 2);
            charts.push(Chart {
                id: FaceId { prim: sh.prim, face: FILLET0 + e as u32 },
                shape: si,
                map: Map::Cyl { o, a: am, b: bm, c: cm, r: rad },
                lo: DVec2::new(-s[ax] * sc, 0.0),
                hi: DVec2::new(s[ax] * sc, rad * FRAC_PI_2),
                side,
                arcs: vec![],
                strip: Some((rad, segs)),
                cons: cons_base.clone(),
            });
        }
    }
    progress.set(1, charts.len() as u32);

    let mut lay = Layout { field: &field, shapes, prev, charts, nodes: vec![], edges: vec![], bnd: vec![], rays: vec![], inner: vec![], origins: vec![], node_tol: extent * 2e-6 };
    let nc = lay.charts.len();
    lay.bnd = vec![vec![]; nc];
    lay.rays = vec![vec![]; nc];
    lay.inner = vec![vec![]; nc];
    let fd = extent * 1e-7;

    // 1. pieces of boundary of every chart, between nodes
    struct Piece {
        chart: usize,
        a: usize,
        b: usize,
        label: FaceId,
        pts: Vec<DVec3>,
    }
    let mut pieces: Vec<Piece> = vec![];
    let mut chart_loops: Vec<Vec<Vec<usize>>> = vec![vec![]; nc];
    let mut raw_loops: Vec<(usize, Vec<(FaceId, Vec<DVec3>)>)> = vec![];
    for ci in 0..nc {
        let ch = &lay.charts[ci];
        let size = ch.hi - ch.lo;
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        let mut grid = (extent / 260.0).min(size.min_element() / 6.0);
        if let Some((r, _)) = ch.strip {
            grid = grid.min(r * 0.25);
        }
        for (_, r, _, _) in &ch.arcs {
            grid = grid.min(r * 0.25);
        }
        let f = |q: DVec2| lay.fmin(ch, q).0;
        let loops = march(&f, ch.lo, ch.hi, grid);
        for lp in loops {
            // refine every point onto the curve it belongs to
            let mut pts: Vec<(DVec2, Cons, FaceId)> = vec![];
            for q in lp {
                let (_, k) = lay.fmin(ch, q);
                let l = lay.label(ch, k, q);
                let q2 = proj1(&|x| lay.psi(ch, k, l, x), q, fd);
                if q2.distance(q) > grid * 2.0 {
                    continue;
                }
                pts.push((q2, k, l));
            }
            let n = pts.len();
            if n < 3 {
                continue;
            }
            // nodes where the curve or the face across changes
            let mut marks: Vec<(usize, DVec2)> = vec![];
            for i in 0..n {
                let (qa, ka, la) = pts[i];
                let (qb, kb, lb) = pts[(i + 1) % n];
                if ka == kb && la == lb {
                    continue;
                }
                let fa = |x: DVec2| lay.psi(ch, ka, la, x);
                let fb = |x: DVec2| lay.psi(ch, kb, lb, x);
                let mut node = solve_cross(&fa, &fb, (qa + qb) * 0.5, fd).filter(|x| x.distance((qa + qb) * 0.5) < grid * 3.0);
                if node.is_none() {
                    // tangent change: bisection on which curve is under the point
                    let (mut s0, mut s1) = (0.0, 1.0);
                    let mut last = qa;
                    for _ in 0..50 {
                        let m = (s0 + s1) * 0.5;
                        let x = qa.lerp(qb, m);
                        let (_, k) = lay.fmin(ch, x);
                        let l = lay.label(ch, k, x);
                        let xp = proj1(&|y| lay.psi(ch, k, l, y), x, fd);
                        if k == ka && l == la {
                            s0 = m;
                        } else {
                            s1 = m;
                        }
                        last = xp;
                    }
                    node = Some(last);
                }
                marks.push((i, node.unwrap()));
            }
            if marks.is_empty() {
                return Err("par lignes : contour fermé sans coin (trou rond) pas encore géré".into());
            }
            let mut raw = vec![];
            for (k, &(i, q)) in marks.iter().enumerate() {
                let (j, nq) = marks[(k + 1) % marks.len()];
                // points strictly between the two nodes
                let mut poly = vec![q];
                let mut t = (i + 1) % n;
                let label = pts[t].2;
                loop {
                    let p = pts[t].0;
                    if p.distance(q) > grid && p.distance(nq) > grid {
                        poly.push(p);
                    }
                    if t == j {
                        break;
                    }
                    t = (t + 1) % n;
                }
                poly.push(nq);
                raw.push((label, poly.iter().map(|x| ch.map.to3(*x)).collect::<Vec<DVec3>>()));
            }
            raw_loops.push((ci, raw));
        }
        progress.tick();
    }
    for (ci, raw) in raw_loops {
        let mut ids = vec![];
        for (label, pts) in raw {
            let a = lay.node(pts[0]);
            let b = lay.node(*pts.last().unwrap());
            if a == b {
                return Err("par lignes : bord refermé sur un seul coin".into());
            }
            pieces.push(Piece { chart: ci, a, b, label, pts });
            ids.push(pieces.len() - 1);
        }
        chart_loops[ci].push(ids);
    }

    // 2. each piece meets its twin in the chart across
    let chart_of: HashMap<FaceId, usize> = lay.charts.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
    let mut piece_edge: Vec<Option<(usize, bool)>> = vec![None; pieces.len()];
    for i in 0..pieces.len() {
        if piece_edge[i].is_some() {
            continue;
        }
        let p = &pieces[i];
        let me = lay.charts[p.chart].id;
        let Some(&other) = chart_of.get(&p.label) else {
            return Err(format!("par lignes : face voisine absente ({:?})", p.label));
        };
        let mid = p.pts[p.pts.len() / 2];
        let twin = (0..pieces.len())
            .filter(|&j| j != i && piece_edge[j].is_none() && pieces[j].chart == other && pieces[j].label == me && ((pieces[j].a == p.a && pieces[j].b == p.b) || (pieces[j].a == p.b && pieces[j].b == p.a)))
            .min_by(|&x, &y| {
                let dx = pieces[x].pts.iter().map(|q| q.distance(mid)).fold(f64::INFINITY, f64::min);
                let dy = pieces[y].pts.iter().map(|q| q.distance(mid)).fold(f64::INFINITY, f64::min);
                dx.total_cmp(&dy)
            });
        let Some(j) = twin else {
            return Err("par lignes : ligne sans partenaire de l'autre côté".into());
        };
        let e = lay.edges.len();
        let mut pts = p.pts.clone();
        pts[0] = lay.nodes[p.a];
        let last = pts.len() - 1;
        pts[last] = lay.nodes[p.b];
        lay.edges.push(GEdge { a: p.a, b: p.b, pts, charts: [p.chart, other], ray: false, alive: true });
        piece_edge[i] = Some((e, true));
        piece_edge[j] = Some((e, pieces[j].a == p.a));
    }
    for ci in 0..nc {
        for lp in &chart_loops[ci] {
            let l: Vec<(usize, bool)> = lp.iter().map(|&i| piece_edge[i].unwrap()).collect();
            lay.bnd[ci].push(l);
        }
    }

    // 3. loop origins: concave fillet ends (middle of the arc) and sharp
    // re-entrant corners
    let mut queue: VecDeque<(usize, usize, DVec2)> = VecDeque::new();
    let mut forced: Vec<(usize, usize)> = vec![];
    for ci in 0..nc {
        if lay.charts[ci].strip.is_some() {
            continue;
        }
        let loops = lay.bnd[ci].clone();
        for lp in loops {
            let n = lp.len();
            for k in 0..n {
                let (e, fwd) = lp[k];
                let other = lay.edges[e].charts.iter().copied().find(|&c| c != ci).unwrap_or(ci);
                let pp = lay.hpts(e, fwd, ci);
                let total: f64 = pp.windows(3).map(|w| turn(w[1] - w[0], w[2] - w[1])).sum();
                if lay.charts[other].strip.is_some() && total.abs() > 0.6 {
                    // the arc in the strip's own chart: its middle row
                    let sm = lay.charts[other].map;
                    let (na, nb) = (lay.edges[e].a, lay.edges[e].b);
                    let (qa, qb) = (sm.to2(lay.nodes[na]), sm.to2(lay.nodes[nb]));
                    let row = lay.charts[other].row();
                    let mid = (qa + qb) * 0.5;
                    let mid = DVec2::new(mid.x, (mid.y / row).round() * row);
                    // the straight lines on both sides give the exact directions
                    let (ep, fp) = lp[(k + n - 1) % n];
                    let (en, fnx) = lp[(k + 1) % n];
                    let t_in = lay.end_dir(ep, fp, ci);
                    let t_out = lay.start_dir(en, fnx, ci);
                    let m = lay.split(e, sm.to3(mid));
                    if total < 0.0 && st.diagonal {
                        // concave fillet hooked diagonally: from its middle a
                        // line runs to a grid node one quad away from the
                        // corner, where the two loops of the face cross
                        lay.origins.push(m);
                        let map = lay.charts[ci].map;
                        let (ps, pe) = if fwd { (na, nb) } else { (nb, na) };
                        let (p0, q0) = (map.to2(lay.nodes[ps]), map.to2(lay.nodes[pe]));
                        let (a1, a2) = (t_in.normalize(), -t_out.normalize());
                        // corner where the two walls would meet
                        let den = a1.perp_dot(t_out);
                        if den.abs() < 1e-6 {
                            return Err("par lignes : fillet entre deux murs parallèles".into());
                        }
                        let s = (q0 - p0).perp_dot(t_out) / den;
                        let x = p0 + a1 * s;
                        let g2 = x + (a1 + a2) * st.size;
                        let g = lay.node(map.to3(g2));
                        lay.origins.push(g);
                        lay.inner[ci].push(g);
                        let re = lay.edges.len();
                        lay.edges.push(GEdge { a: m, b: g, pts: vec![lay.nodes[m], lay.nodes[g]], charts: [ci, ci], ray: true, alive: true });
                        lay.rays[ci].push(re);
                        for d in [a1, -a1, a2, -a2] {
                            queue.push_back((ci, g, d));
                        }
                        queue.push_back((other, m, DVec2::ZERO));
                    } else if total < 0.0 {
                        // concave fillet: a re-entrant corner in its middle
                        lay.origins.push(m);
                        queue.push_back((ci, m, t_in));
                        queue.push_back((ci, m, -t_out));
                        queue.push_back((other, m, DVec2::ZERO));
                    } else {
                        // convex fillet: loops from its rails, its middle is a
                        // corner of the rounded patch
                        forced.push((ci, m));
                        queue.push_back((ci, na, DVec2::ZERO));
                        queue.push_back((ci, nb, DVec2::ZERO));
                    }
                }
            }
        }
        // sharp re-entrant corners
        let loops = lay.bnd[ci].clone();
        for lp in loops {
            let n = lp.len();
            for k in 0..n {
                let (ea, fa) = lp[k];
                let (eb, fb) = lp[(k + 1) % n];
                let ti = lay.end_dir(ea, fa, ci);
                let to = lay.start_dir(eb, fb, ci);
                if turn(ti, to) < -0.5 {
                    let v = if fa { lay.edges[ea].b } else { lay.edges[ea].a };
                    lay.origins.push(v);
                    queue.push_back((ci, v, ti));
                    queue.push_back((ci, v, -to));
                }
            }
        }
    }

    // 4. trace the loops across the faces
    let snap = (st.size * 0.02).max(extent * 1e-4);
    let mut guard = 0;
    while let Some((ci, n0, d0)) = queue.pop_front() {
        guard += 1;
        if guard > 4000 {
            return Err("par lignes : boucles sans fin".into());
        }
        let o = lay.charts[ci].map.to2(lay.nodes[n0]);
        // direction: given, or square to the boundary into the face
        let d = if d0 == DVec2::ZERO {
            match inward(&lay, ci, n0) {
                Some(d) => d,
                None => continue,
            }
        } else {
            d0.normalize()
        };
        // already a loop leaving this node this way
        let exists = lay.rays[ci].iter().any(|&r| {
            let ed = &lay.edges[r];
            let (from, to) = if ed.a == n0 { (ed.a, ed.b) } else if ed.b == n0 { (ed.b, ed.a) } else { return false };
            let m = lay.charts[ci].map;
            (m.to2(lay.nodes[to]) - m.to2(lay.nodes[from])).normalize_or_zero().dot(d) > 0.999
        });
        if exists {
            continue;
        }
        // first crossing with the chart's boundary
        let mut best: Option<(f64, usize, usize, f64)> = None;
        for lp in &lay.bnd[ci] {
            for &(e, _) in lp {
                let p2 = lay.pts2(e, ci);
                for i in 0..p2.len() - 1 {
                    let (p, q) = (p2[i], p2[i + 1]);
                    let r = q - p;
                    let den = d.perp_dot(r);
                    if den.abs() < 1e-14 {
                        continue;
                    }
                    let s = (p - o).perp_dot(r) / den;
                    let w = (p - o).perp_dot(d) / den;
                    if s > extent * 1e-6 && (-1e-9..=1.0 + 1e-9).contains(&w) && best.is_none_or(|b| s < b.0) {
                        best = Some((s, e, i, w));
                    }
                }
            }
        }
        // a loop node inside the face on the way (where loops cross)
        let mut stop: Option<(f64, usize)> = None;
        for &g in &lay.inner[ci] {
            if g == n0 {
                continue;
            }
            let w = lay.charts[ci].map.to2(lay.nodes[g]) - o;
            let s = w.dot(d);
            if s > extent * 1e-6 && (w - d * s).length() < snap && best.is_none_or(|b| s < b.0) && stop.is_none_or(|x| s < x.0) {
                stop = Some((s, g));
            }
        }
        if let Some((_, g)) = stop {
            let re = lay.edges.len();
            lay.edges.push(GEdge { a: n0, b: g, pts: vec![lay.nodes[n0], lay.nodes[g]], charts: [ci, ci], ray: true, alive: true });
            lay.rays[ci].push(re);
            continue;
        }
        let Some((_, e, i, w)) = best else {
            return Err("par lignes : boucle qui ne ressort pas de la face".into());
        };
        let ed = lay.edges[e].clone();
        let x = ed.pts[i].lerp(ed.pts[i + 1], w.clamp(0.0, 1.0));
        let n1 = if x.distance(lay.nodes[ed.a]) < snap {
            ed.a
        } else if x.distance(lay.nodes[ed.b]) < snap {
            ed.b
        } else {
            lay.split(e, x)
        };
        if n1 == n0 {
            continue;
        }
        let re = lay.edges.len();
        lay.edges.push(GEdge { a: n0, b: n1, pts: vec![lay.nodes[n0], lay.nodes[n1]], charts: [ci, ci], ray: true, alive: true });
        lay.rays[ci].push(re);
        if lay.origins.contains(&n1) {
            continue;
        }
        // on into the face across
        let next = ed.charts.iter().copied().find(|&c| c != ci);
        if let Some(nc2) = next {
            if corner_angle(&lay, nc2, n1).is_some_and(|t| t.abs() > 0.5) {
                continue;
            }
            queue.push_back((nc2, n1, DVec2::ZERO));
        }
    }

    // 5. loops crossing each other inside a face
    for ci in 0..nc {
        let mut tries = 0;
        'again: loop {
            tries += 1;
            if tries > 2000 {
                return Err("par lignes : croisements de boucles".into());
            }
            let rs = lay.rays[ci].clone();
            let m = lay.charts[ci].map;
            for x in 0..rs.len() {
                for y in (x + 1)..rs.len() {
                    let (ea, eb) = (&lay.edges[rs[x]], &lay.edges[rs[y]]);
                    if ea.a == eb.a || ea.a == eb.b || ea.b == eb.a || ea.b == eb.b {
                        continue;
                    }
                    let (p, p2) = (m.to2(lay.nodes[ea.a]), m.to2(lay.nodes[ea.b]));
                    let (q, q2) = (m.to2(lay.nodes[eb.a]), m.to2(lay.nodes[eb.b]));
                    let (r, s) = (p2 - p, q2 - q);
                    let den = r.perp_dot(s);
                    if den.abs() < 1e-14 {
                        continue;
                    }
                    let t = (q - p).perp_dot(s) / den;
                    let u = (q - p).perp_dot(r) / den;
                    if t > 1e-6 && t < 1.0 - 1e-6 && u > 1e-6 && u < 1.0 - 1e-6 {
                        let x3 = m.to3(p + r * t);
                        let (a, b) = (rs[x], rs[y]);
                        let n = lay.split(a, x3);
                        let x3 = lay.nodes[n];
                        lay.split(b, x3);
                        continue 'again;
                    }
                }
            }
            break;
        }
    }

    // 6. patches: faces of the line graph in each chart
    let mut patches: Vec<(usize, [Vec<(usize, bool)>; 4])> = vec![];
    for ci in 0..nc {
        let mut hes: Vec<(usize, bool, bool)> = vec![]; // (edge, fwd, inside)
        for lp in &lay.bnd[ci] {
            for &(e, f) in lp {
                hes.push((e, f, true));
                hes.push((e, !f, false));
            }
        }
        for &r in &lay.rays[ci] {
            hes.push((r, true, true));
            hes.push((r, false, true));
        }
        if hes.is_empty() {
            continue;
        }
        let from = |h: &(usize, bool, bool)| if h.1 { lay.edges[h.0].a } else { lay.edges[h.0].b };
        let to = |h: &(usize, bool, bool)| if h.1 { lay.edges[h.0].b } else { lay.edges[h.0].a };
        let mut out_at: HashMap<usize, Vec<(f64, usize)>> = HashMap::new();
        for (i, h) in hes.iter().enumerate() {
            let d = lay.start_dir(h.0, h.1, ci);
            out_at.entry(from(h)).or_default().push((d.y.atan2(d.x), i));
        }
        for v in out_at.values_mut() {
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        let twin = |i: usize| -> usize { hes.iter().position(|h| h.0 == hes[i].0 && h.1 != hes[i].1).unwrap() };
        let mut seen = vec![false; hes.len()];
        for s in 0..hes.len() {
            if seen[s] {
                continue;
            }
            let mut cyc = vec![];
            let mut cur = s;
            let ok;
            loop {
                if seen[cur] {
                    ok = cur == s;
                    break;
                }
                seen[cur] = true;
                cyc.push(cur);
                let t = twin(cur);
                let list = &out_at[&to(&hes[cur])];
                let p = list.iter().position(|x| x.1 == t).unwrap();
                cur = list[(p + list.len() - 1) % list.len()].1;
            }
            if !ok || cyc.iter().any(|&h| !hes[h].2) {
                continue;
            }
            // corners: where the boundary turns
            let k = cyc.len();
            let mut corners = vec![];
            let mut area = 0.0;
            for i in 0..k {
                let (a, b) = (hes[cyc[(i + k - 1) % k]], hes[cyc[i]]);
                let t = turn(lay.end_dir(a.0, a.1, ci), lay.start_dir(b.0, b.1, ci));
                if t > 0.5 || forced.contains(&(ci, from(&b))) {
                    corners.push(i);
                }
                let p = lay.hpts(b.0, b.1, ci);
                for w in p.windows(2) {
                    area += w[0].perp_dot(w[1]);
                }
            }
            if area <= 0.0 {
                return Err("par lignes : face avec un trou non relié".into());
            }
            if corners.len() != 4 && std::env::var("DEBUG_LAYOUT").is_ok() {
                eprintln!("chart {:?}: {} corners", lay.charts[ci].id, corners.len());
                for i in 0..k {
                    let (a, b) = (hes[cyc[(i + k - 1) % k]], hes[cyc[i]]);
                    let t = turn(lay.end_dir(a.0, a.1, ci), lay.start_dir(b.0, b.1, ci));
                    let n0 = from(&b);
                    eprintln!("  he e{} fwd {} ray {} from n{} {:?} -> n{} turn {:.1}°", b.0, b.1, lay.edges[b.0].ray, n0, lay.nodes[n0], to(&b), t.to_degrees());
                }
                eprintln!("  origins {:?}", lay.origins);
            }
            if corners.len() != 4 {
                return Err(format!("par lignes : patch à {} coins sur {:?}", corners.len(), lay.charts[ci].id));
            }
            let mut sides: [Vec<(usize, bool)>; 4] = Default::default();
            for (si, side) in sides.iter_mut().enumerate() {
                let (c0, c1) = (corners[si], corners[(si + 1) % 4]);
                let mut i = c0;
                while i != c1 {
                    side.push((hes[cyc[i]].0, hes[cyc[i]].1));
                    i = (i + 1) % k;
                }
            }
            patches.push((ci, sides));
        }
    }

    // 7. segments per line: opposite sides of every patch agree
    let ne = lay.edges.len();
    let mut count = vec![0usize; ne];
    let mut fixed = vec![false; ne];
    let mut target = vec![1.0f64; ne];
    for e in 0..ne {
        let ed = &lay.edges[e];
        if !ed.alive {
            continue;
        }
        let len: f64 = ed.pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        let ray_len = if ed.ray {
            // a loop across a strip follows its arc
            let m = lay.charts[ed.charts[0]].map;
            let (a, b) = (m.to2(lay.nodes[ed.a]), m.to2(lay.nodes[ed.b]));
            a.distance(b)
        } else {
            len
        };
        target[e] = ray_len / st.size;
        count[e] = (target[e].round() as usize).max(1);
        for &c in &ed.charts {
            let ch = &lay.charts[c];
            if ch.strip.is_some() {
                let m = ch.map;
                let d = m.to2(lay.nodes[ed.b]) - m.to2(lay.nodes[ed.a]);
                if d.y.abs() > d.x.abs() {
                    fixed[e] = true;
                    count[e] = ((d.y.abs() / ch.row()).round() as usize).max(1);
                }
            }
        }
    }
    let sum = |side: &Vec<(usize, bool)>, count: &Vec<usize>| -> usize { side.iter().map(|x| count[x.0]).sum() };
    let mut it = 0;
    loop {
        it += 1;
        if it > 200_000 {
            return Err("par lignes : nombres de segments impossibles à accorder".into());
        }
        let mut bad = None;
        for (_, s) in &patches {
            for (a, b) in [(0, 2), (1, 3)] {
                let (na, nb) = (sum(&s[a], &count), sum(&s[b], &count));
                if na != nb {
                    bad = Some(if na < nb { (s[a].clone(), s[b].clone()) } else { (s[b].clone(), s[a].clone()) });
                    break;
                }
            }
            if bad.is_some() {
                break;
            }
        }
        let Some((small, big)) = bad else { break };
        let up = small.iter().filter(|x| !fixed[x.0]).max_by(|x, y| (target[x.0] / count[x.0] as f64).total_cmp(&(target[y.0] / count[y.0] as f64)));
        if let Some(&(e, _)) = up {
            count[e] += 1;
            continue;
        }
        let down = big.iter().filter(|x| !fixed[x.0] && count[x.0] > 1).min_by(|x, y| (target[x.0] / count[x.0] as f64).total_cmp(&(target[y.0] / count[y.0] as f64)));
        match down {
            Some(&(e, _)) => count[e] -= 1,
            None => return Err("par lignes : rangées de fillet impossibles à raccorder".into()),
        }
    }

    // 8. where the vertices sit on every line: the free lines take the
    // spacing of the line across their patch (fillet rows run on straight)
    let elen: Vec<f64> = (0..ne)
        .map(|e| {
            let ed = &lay.edges[e];
            if !ed.alive {
                0.0
            } else if ed.ray {
                let m = lay.charts[ed.charts[0]].map;
                m.to2(lay.nodes[ed.a]).distance(m.to2(lay.nodes[ed.b]))
            } else {
                ed.pts.windows(2).map(|w| w[0].distance(w[1])).sum()
            }
        })
        .collect();
    let mut par: Vec<Vec<f64>> = (0..ne).map(|e| (1..count[e]).map(|i| i as f64 / count[e] as f64).collect()).collect();
    // positions along a side (corner to corner), 0..1
    let side_par = |side: &Vec<(usize, bool)>, par: &Vec<Vec<f64>>| -> Vec<f64> {
        let mut out = vec![0.0];
        let mut at = 0.0;
        for &(e, f) in side {
            let mut t: Vec<f64> = par[e].clone();
            if !f {
                t = t.iter().rev().map(|x| 1.0 - x).collect();
            }
            for x in t {
                out.push(at + x * elen[e]);
            }
            at += elen[e];
            out.push(at);
        }
        let tot = at.max(1e-30);
        out.iter().map(|x| x / tot).collect()
    };
    for _ in 0..60 {
        let mut acc: Vec<Vec<(f64, f64)>> = par.iter().map(|v| vec![(0.0, 0.0); v.len()]).collect();
        for (_, sides) in &patches {
            for (a, b) in [(0usize, 2usize), (2, 0), (1, 3), (3, 1)] {
                // with diagonal hooks the fillet rows stay in their corner
                if st.diagonal && sides[a].iter().any(|x| fixed[x.0]) {
                    continue;
                }
                let pa = side_par(&sides[a], &par);
                let n = pa.len() - 1;
                let tot: f64 = sides[b].iter().map(|x| elen[x.0]).sum();
                let mut start = 0usize;
                let mut before = 0.0;
                for &(e, f) in &sides[b] {
                    let c = count[e];
                    for k in 1..c {
                        let j = start + k;
                        let want = (1.0 - pa[n - j]) * tot - before;
                        let mut t = want / elen[e].max(1e-30);
                        let mut idx = k - 1;
                        if !f {
                            t = 1.0 - t;
                            idx = c - k - 1;
                        }
                        acc[e][idx].0 += t.clamp(0.0, 1.0);
                        acc[e][idx].1 += 1.0;
                    }
                    start += c;
                    before += elen[e];
                }
            }
        }
        for e in 0..ne {
            if fixed[e] || par[e].is_empty() {
                continue;
            }
            let c = count[e] as f64;
            for (k, (s, w)) in acc[e].iter().enumerate() {
                if *w > 0.0 {
                    par[e][k] = 0.5 * par[e][k] + 0.5 * s / w;
                }
            }
            // keep the order, with a minimal spacing
            let gap = 0.15 / c;
            for k in 0..par[e].len() {
                let lo_k = if k == 0 { gap } else { par[e][k - 1] + gap };
                par[e][k] = par[e][k].max(lo_k);
            }
            for k in (0..par[e].len()).rev() {
                let hi_k = if k + 1 == par[e].len() { 1.0 - gap } else { par[e][k + 1] - gap };
                par[e][k] = par[e][k].min(hi_k);
            }
        }
    }

    // vertices on every line, then a grid in every patch
    let mut pos: Vec<DVec3> = lay.nodes.clone();
    let mut samples: Vec<Vec<u32>> = vec![vec![]; ne];
    for e in 0..ne {
        let ed = &lay.edges[e];
        if !ed.alive {
            continue;
        }
        let mut ids = vec![ed.a as u32];
        if ed.ray {
            let m = lay.charts[ed.charts[0]].map;
            let (a, b) = (m.to2(lay.nodes[ed.a]), m.to2(lay.nodes[ed.b]));
            for &t in &par[e] {
                pos.push(m.to3(a.lerp(b, t)));
                ids.push(pos.len() as u32 - 1);
            }
        } else {
            let cum: Vec<f64> = std::iter::once(0.0)
                .chain(ed.pts.windows(2).scan(0.0, |s, w| {
                    *s += w[0].distance(w[1]);
                    Some(*s)
                }))
                .collect();
            let total = *cum.last().unwrap();
            for &tp in &par[e] {
                let t = total * tp;
                let k = cum.partition_point(|&c| c < t).clamp(1, cum.len() - 1);
                let w = (t - cum[k - 1]) / (cum[k] - cum[k - 1]).max(1e-30);
                pos.push(ed.pts[k - 1].lerp(ed.pts[k], w));
                ids.push(pos.len() as u32 - 1);
            }
        }
        ids.push(ed.b as u32);
        samples[e] = ids;
    }
    let mut faces: Vec<Vec<u32>> = vec![];
    let mut roles: Vec<u32> = vec![];
    for (ci, sides) in &patches {
        let ch = &lay.charts[*ci];
        let side_ids = |k: usize| -> Vec<u32> {
            let mut v: Vec<u32> = vec![];
            for &(e, f) in &sides[k] {
                let mut s = samples[e].clone();
                if !f {
                    s.reverse();
                }
                if !v.is_empty() {
                    s.remove(0);
                }
                v.extend(s);
            }
            v
        };
        let (s0, s1, mut s2, mut s3) = (side_ids(0), side_ids(1), side_ids(2), side_ids(3));
        s2.reverse();
        s3.reverse();
        let (n, m) = (s0.len() - 1, s1.len() - 1);
        if s2.len() != n + 1 || s3.len() != m + 1 {
            return Err("par lignes : patch mal accordé".into());
        }
        let q2: HashMap<u32, DVec2> = s0.iter().chain(&s1).chain(&s2).chain(&s3).map(|&v| (v, ch.map.to2(pos[v as usize]))).collect();
        let q = |v: u32| q2[&v];
        let params = |s: &[u32]| -> Vec<f64> {
            let mut c = vec![0.0];
            for w in s.windows(2) {
                c.push(c.last().unwrap() + q(w[0]).distance(q(w[1])));
            }
            let t = c.last().copied().unwrap_or(1.0).max(1e-30);
            c.iter().map(|x| x / t).collect()
        };
        let (ub, ut, vl, vr) = (params(&s0), params(&s2), params(&s3), params(&s1));
        let (b0, bn, t0, tn) = (q(s0[0]), q(s0[n]), q(s2[0]), q(s2[n]));
        let mut grid = vec![vec![0u32; m + 1]; n + 1];
        for i in 0..=n {
            grid[i][0] = s0[i];
            grid[i][m] = s2[i];
        }
        for j in 0..=m {
            grid[0][j] = s3[j];
            grid[n][j] = s1[j];
        }
        for i in 1..n {
            for j in 1..m {
                let (mut u, mut v) = ((ub[i] + ut[i]) * 0.5, (vl[j] + vr[j]) * 0.5);
                for _ in 0..4 {
                    u = (1.0 - v) * ub[i] + v * ut[i];
                    v = (1.0 - u) * vl[j] + u * vr[j];
                }
                let p = q(s0[i]) * (1.0 - v) + q(s2[i]) * v + q(s3[j]) * (1.0 - u) + q(s1[j]) * u
                    - (b0 * ((1.0 - u) * (1.0 - v)) + bn * (u * (1.0 - v)) + t0 * ((1.0 - u) * v) + tn * (u * v));
                pos.push(ch.map.to3(p));
                grid[i][j] = pos.len() as u32 - 1;
            }
        }
        let role = if ch.strip.is_some() { P_FILLET } else { P_LOFT };
        for i in 0..n {
            for j in 0..m {
                faces.push(vec![grid[i][j], grid[i + 1][j], grid[i + 1][j + 1], grid[i][j + 1]]);
                roles.push(role);
            }
        }
    }
    if faces.is_empty() {
        return Err("par lignes : aucune face".into());
    }

    // 9. compact, orient outward, check
    let mut remap = vec![u32::MAX; pos.len()];
    let mut cpos = vec![];
    for f in &mut faces {
        for v in f.iter_mut() {
            if remap[*v as usize] == u32::MAX {
                remap[*v as usize] = cpos.len() as u32;
                cpos.push(pos[*v as usize]);
            }
            *v = remap[*v as usize];
        }
    }
    orient(&cpos, &mut faces, &field);
    let mut b = Baked { positions: cpos, ..Default::default() };
    for (f, r) in faces.into_iter().zip(roles) {
        b.faces.push(BFace { verts: f, normals: vec![], mat: 0, patch: r });
    }
    let bs = BakeSettings { tol: st.tol, max_edge: st.size, trim_len: st.size, threads: st.threads, divisions: 0.0 };
    progress.set(4, b.faces.len() as u32);
    finish(&field, &mut b, &bs, progress);
    if b.stats.open_edges > 0 || b.stats.nonmanifold_edges > 0 {
        return Err(format!("par lignes : maillage mal fermé ({} bords ouverts)", b.stats.open_edges + b.stats.nonmanifold_edges));
    }
    if b.stats.max_dev > (st.size * 0.25).max(0.2) {
        return Err(format!("par lignes : écart de {:.2} mm à la surface", b.stats.max_dev).replace('.', ","));
    }
    b.stats.terms = lay.shapes.len();
    b.stats.patches = patches.len();
    b.stats.constructive = true;
    b.stats.lines = true;
    b.stats.quad_size = st.size;
    b.stats.seconds = t0.elapsed().as_secs_f64();
    let _ = lay.field;
    Ok(b)
}

/// Turn of the boundary of chart `c` at node `n` (None when `n` is not on it).
fn corner_angle(lay: &Layout, c: usize, n: usize) -> Option<f64> {
    for lp in &lay.bnd[c] {
        let k = lp.len();
        for i in 0..k {
            let (ea, fa) = lp[i];
            let end = if fa { lay.edges[ea].b } else { lay.edges[ea].a };
            if end == n {
                let (eb, fb) = lp[(i + 1) % k];
                return Some(turn(lay.end_dir(ea, fa, c), lay.start_dir(eb, fb, c)));
            }
        }
    }
    None
}

/// Direction square to the boundary of chart `c` at node `n`, into the face.
fn inward(lay: &Layout, c: usize, n: usize) -> Option<DVec2> {
    for lp in &lay.bnd[c] {
        let k = lp.len();
        for i in 0..k {
            let (ea, fa) = lp[i];
            let end = if fa { lay.edges[ea].b } else { lay.edges[ea].a };
            if end == n {
                let (eb, fb) = lp[(i + 1) % k];
                let t = (lay.end_dir(ea, fa, c) + lay.start_dir(eb, fb, c)).normalize_or_zero();
                return (t != DVec2::ZERO).then(|| perp(t));
            }
        }
    }
    None
}

/// Faces oriented consistently over shared edges, then outward.
fn orient(pos: &[DVec3], faces: &mut [Vec<u32>], field: &Field) {
    let nf = faces.len();
    let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, f) in faces.iter().enumerate() {
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
            let f = faces[i].clone();
            for k in 0..f.len() {
                let (a, b) = (f[k], f[(k + 1) % f.len()]);
                for &j in &edge_faces[&(a.min(b), a.max(b))] {
                    if done[j] {
                        continue;
                    }
                    if has_dir(&faces[j], a, b) {
                        faces[j].reverse();
                    }
                    done[j] = true;
                    comp.push(j);
                    stack.push(j);
                }
            }
        }
        let mut vote = 0.0;
        for &i in &comp {
            let f = &faces[i];
            let mut n = DVec3::ZERO;
            let mut c = DVec3::ZERO;
            for k in 0..f.len() {
                n += pos[f[k] as usize].cross(pos[f[(k + 1) % f.len()] as usize]);
                c += pos[f[k] as usize];
            }
            vote += n.dot(field.grad(c / f.len() as f64)).signum();
        }
        if vote < 0.0 {
            for &i in &comp {
                faces[i].reverse();
            }
        }
    }
}
