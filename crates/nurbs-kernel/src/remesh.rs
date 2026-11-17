//! The « Export converter »: a field-aligned quad remesh of the whole part
//! (Romain, 17 nov. 2026: study QuadRemesher, then make our own), separate
//! from the face-by-face converter of `quads.rs`.
//!
//! The method is the published one of Instant Meshes (Jakob, Tarini,
//! Panozzo, Sorkine-Hornung 2015), written from the paper, with what the
//! B-rep gives for free:
//! 1. **Samples**: the kernel's triangles, welded, split until every edge is
//!    shorter than a third of a quad, relaxed, and put back on the exact
//!    surface (plane, cylinder, sphere, torus from the face's own geometry)
//!    or on the exact edge (the B-rep edge's polyline). Every B-rep edge is a
//!    feature line, including a fillet's tangent borders: fillets come out
//!    as bands of quad rows.
//! 2. **Orientation field** (4-RoSy): along a B-rep edge, the edge's
//!    direction; on a cylinder, cone or torus (fillets), the surface's own
//!    u / v directions, exact; on planes and other faces, smoothed from
//!    there (Gauss-Seidel, extrinsic matching).
//! 3. **Position field**: a lattice of the quad size at each sample, made
//!    consistent with its neighbours (extrinsic matching, rounding); a
//!    corner is a lattice point, an edge sample's lattice lies along its
//!    edge.
//! 4. **Extraction**: samples on the same lattice point are merged, unit
//!    lattice steps become edges, the faces of that graph are traced, and
//!    each n-gon is cut into n quads (all quads, half the size): points
//!    back on the exact surface and edges, a little relaxation.
//!
//! QuadRemesher's settings map to `Settings`: Target Quad Count →
//! `target_quads`; "Use existing hard edges" is always on (the B-rep's
//! edges); "Detect hard edges by angle" is not needed.

use crate::model::{Model, Quads, SurfKind};
use glam::DVec3 as V;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// About how many quads (QuadRemesher's "Target Quad Count").
    pub target_quads: usize,
    /// Sweeps of the orientation and position fields.
    pub iterations: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { target_quads: 1500, iterations: 120 }
    }
}

/// What the remesh did, for the status line and the tests.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub samples: usize,
    pub quad_size: f64,
    /// Polygons of the extracted graph by number of sides (3, 4, 5, 6+).
    pub polygons: [usize; 4],
    /// Loops too large to be a polygon (holes in the extracted graph).
    pub holes: usize,
}

// ------------------------------------------------------------- surfaces

#[derive(Clone, Copy, Debug)]
struct Surf {
    kind: SurfKind,
    o: V,
    a: V,
    r: f64,
    /// A torus's tube radius.
    minor: f64,
}

impl Surf {
    fn new(f: &crate::model::Face) -> Surf {
        let o = V::from(f.axis_origin);
        let a = V::from(f.axis_dir).normalize_or_zero();
        let mut minor = 0.0;
        if f.kind == SurfKind::Torus {
            let p = V::from(f.point) - o;
            let rad = p - a * a.dot(p);
            let ring = rad.normalize_or_zero() * f.radius;
            minor = (rad - ring + a * a.dot(p)).length();
        }
        Surf { kind: f.kind, o, a, r: f.radius, minor }
    }

    /// The nearest point on the surface (unbounded), when it is known exactly.
    fn project(&self, p: V) -> Option<V> {
        let d = p - self.o;
        match self.kind {
            SurfKind::Plane => Some(p - self.a * self.a.dot(d)),
            SurfKind::Cylinder => {
                let along = self.a.dot(d);
                let rad = d - self.a * along;
                (rad.length() > 1e-9).then(|| self.o + self.a * along + rad.normalize() * self.r)
            }
            SurfKind::Sphere => (d.length() > 1e-9).then(|| self.o + d.normalize() * self.r),
            SurfKind::Torus => {
                let rad = d - self.a * self.a.dot(d);
                if rad.length() < 1e-9 {
                    return None;
                }
                let ring = self.o + rad.normalize() * self.r;
                let e = p - ring;
                (e.length() > 1e-9).then(|| ring + e.normalize() * self.minor)
            }
            _ => None,
        }
    }

    /// The surface's own direction at p, when it has one (a round face's
    /// axis or circle direction): exact principal directions.
    fn direction(&self, p: V) -> Option<V> {
        match self.kind {
            SurfKind::Cylinder | SurfKind::Cone => Some(self.a),
            SurfKind::Torus => {
                let c = self.a.cross(p - self.o);
                (c.length() > 1e-9).then(|| c.normalize())
            }
            _ => None,
        }
    }
}

/// A B-rep edge as a polyline.
struct Curve {
    pts: Vec<V>,
}

impl Curve {
    /// Nearest point and the distance to it.
    fn project(&self, p: V) -> (V, f64) {
        let mut best = (self.pts[0], f64::MAX);
        for w in self.pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = if ab.length_squared() > 0.0 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
            let q = a + ab * t;
            let d = (p - q).length();
            if d < best.1 {
                best = (q, d);
            }
        }
        best
    }
}

// ------------------------------------------------------------- samples

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Face(u32),
    Edge(u32),
    Corner,
}

struct Geo {
    surfs: Vec<Surf>,
    curves: Vec<Curve>,
    /// Curves joining two faces (smaller face first).
    by_faces: HashMap<(u32, u32), Vec<u32>>,
    /// Each face's original triangles (for faces with no exact projection).
    face_tris: Vec<Vec<[V; 3]>>,
}

impl Geo {
    fn on_face(&self, f: u32, p: V) -> V {
        if let Some(q) = self.surfs[f as usize].project(p) {
            return q;
        }
        // nearest point on the face's triangles
        let mut best = (p, f64::MAX);
        for t in &self.face_tris[f as usize] {
            let q = closest_on_tri(p, t[0], t[1], t[2]);
            let d = (q - p).length_squared();
            if d < best.1 {
                best = (q, d);
            }
        }
        best.0
    }
    fn place(&self, k: Kind, p: V) -> V {
        match k {
            Kind::Face(f) => self.on_face(f, p),
            Kind::Edge(c) => self.curves[c as usize].project(p).0,
            Kind::Corner => p,
        }
    }
    /// The curve along the edge between two triangles of faces fa ≠ fb.
    fn curve_between(&self, fa: u32, fb: u32, mid: V) -> u32 {
        let near = |ids: &mut dyn Iterator<Item = u32>| ids.min_by(|x, y| self.curves[*x as usize].project(mid).1.total_cmp(&self.curves[*y as usize].project(mid).1));
        if let Some(ids) = self.by_faces.get(&(fa.min(fb), fa.max(fb))) {
            if let Some(c) = near(&mut ids.iter().copied()) {
                return c;
            }
        }
        near(&mut (0..self.curves.len() as u32)).unwrap_or(0)
    }
}

fn closest_on_tri(p: V, a: V, b: V, c: V) -> V {
    // Ericson, Real-Time Collision Detection 5.1.5
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let den = 1.0 / (va + vb + vc);
    a + ab * (vb * den) + ac * (vc * den)
}

/// The sample mesh: points, triangles (each with its face), kinds.
struct Mesh {
    x: Vec<V>,
    tris: Vec<[u32; 3]>,
    tf: Vec<u32>,
    kind: Vec<Kind>,
}

fn ekey(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

impl Mesh {
    /// Undirected edge → the faces of the triangles on it.
    fn edge_faces(&self) -> HashMap<(u32, u32), Vec<u32>> {
        let mut m: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
        for (t, f) in self.tris.iter().zip(&self.tf) {
            for k in 0..3 {
                m.entry(ekey(t[k], t[(k + 1) % 3])).or_default().push(*f);
            }
        }
        m
    }

    fn neighbours(&self) -> Vec<Vec<u32>> {
        let mut n: Vec<Vec<u32>> = vec![vec![]; self.x.len()];
        for t in &self.tris {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if !n[a as usize].contains(&b) {
                    n[a as usize].push(b);
                }
                if !n[b as usize].contains(&a) {
                    n[b as usize].push(a);
                }
            }
        }
        n
    }

    /// Kinds from the feature edges (edges between two different faces).
    fn classify(&mut self, geo: &Geo) {
        let ef = self.edge_faces();
        let mut feat: Vec<Vec<u32>> = vec![vec![]; self.x.len()];
        for (&(a, b), fs) in &ef {
            if fs.len() == 2 && fs[0] != fs[1] {
                let c = geo.curve_between(fs[0], fs[1], (self.x[a as usize] + self.x[b as usize]) * 0.5);
                feat[a as usize].push(c);
                feat[b as usize].push(c);
            } else if fs.len() != 2 {
                // open border: keep it fixed
                feat[a as usize].extend([u32::MAX, u32::MAX, u32::MAX]);
                feat[b as usize].extend([u32::MAX, u32::MAX, u32::MAX]);
            }
        }
        let mut face_of = vec![0u32; self.x.len()];
        for (t, f) in self.tris.iter().zip(&self.tf) {
            for v in t {
                face_of[*v as usize] = *f;
            }
        }
        self.kind = feat
            .iter()
            .enumerate()
            .map(|(v, cs)| match cs.len() {
                0 => Kind::Face(face_of[v]),
                2 if cs[0] == cs[1] && cs[0] != u32::MAX => Kind::Edge(cs[0]),
                _ => Kind::Corner,
            })
            .collect();
    }

    /// Splits every edge longer than `max` (feature edges stay on their
    /// curve, the others go onto their face's surface).
    fn refine(&mut self, geo: &Geo, max: f64) {
        for _ in 0..16 {
            let ef = self.edge_faces();
            let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
            for (&(a, b), fs) in &ef {
                let (pa, pb) = (self.x[a as usize], self.x[b as usize]);
                if (pa - pb).length() <= max {
                    continue;
                }
                let m = (pa + pb) * 0.5;
                let k = if fs.len() == 2 && fs[0] != fs[1] { Kind::Edge(geo.curve_between(fs[0], fs[1], m)) } else if fs.len() == 2 { Kind::Face(fs[0]) } else { Kind::Corner };
                let p = geo.place(k, m);
                mid.insert((a, b), self.x.len() as u32);
                self.x.push(p);
                self.kind.push(k);
            }
            if mid.is_empty() {
                return;
            }
            let mut tris = Vec::with_capacity(self.tris.len() * 3);
            let mut tf = Vec::with_capacity(self.tris.len() * 3);
            for (t, f) in self.tris.iter().zip(&self.tf) {
                let m = |k: usize| mid.get(&ekey(t[k], t[(k + 1) % 3])).copied();
                let ms = [m(0), m(1), m(2)];
                let n = ms.iter().filter(|x| x.is_some()).count();
                let mut out: Vec<[u32; 3]> = vec![];
                match n {
                    0 => out.push(*t),
                    3 => {
                        let (m0, m1, m2) = (ms[0].unwrap(), ms[1].unwrap(), ms[2].unwrap());
                        out.extend([[t[0], m0, m2], [m0, t[1], m1], [m2, m1, t[2]], [m0, m1, m2]]);
                    }
                    1 => {
                        let r = (0..3).find(|k| ms[*k].is_some()).unwrap();
                        let (v0, v1, v2) = (t[r], t[(r + 1) % 3], t[(r + 2) % 3]);
                        let m0 = ms[r].unwrap();
                        out.extend([[v0, m0, v2], [m0, v1, v2]]);
                    }
                    _ => {
                        // two split: rotate so the unsplit one is (v2, v0)
                        let r = (0..3).find(|k| ms[(*k + 2) % 3].is_none()).unwrap();
                        let (v0, v1, v2) = (t[r], t[(r + 1) % 3], t[(r + 2) % 3]);
                        let (m0, m1) = (ms[r].unwrap(), ms[(r + 1) % 3].unwrap());
                        out.push([m0, v1, m1]);
                        let d1 = (self.x[v0 as usize] - self.x[m1 as usize]).length();
                        let d2 = (self.x[m0 as usize] - self.x[v2 as usize]).length();
                        if d1 < d2 {
                            out.extend([[v0, m0, m1], [v0, m1, v2]]);
                        } else {
                            out.extend([[v0, m0, v2], [m0, m1, v2]]);
                        }
                    }
                }
                for o in out {
                    tris.push(o);
                    tf.push(*f);
                }
            }
            self.tris = tris;
            self.tf = tf;
        }
    }

    fn tri_normal(&self, t: [u32; 3]) -> V {
        let (a, b, c) = (self.x[t[0] as usize], self.x[t[1] as usize], self.x[t[2] as usize]);
        (b - a).cross(c - a)
    }

    /// Delaunay flips inside faces (never across a feature edge).
    fn flip(&mut self) {
        for _ in 0..6 {
            let mut at: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
            for (i, t) in self.tris.iter().enumerate() {
                for k in 0..3 {
                    at.entry(ekey(t[k], t[(k + 1) % 3])).or_default().push(i);
                }
            }
            let mut touched = vec![false; self.tris.len()];
            let mut flips = 0;
            for (&(a, b), ts) in &at {
                if ts.len() != 2 || touched[ts[0]] || touched[ts[1]] || self.tf[ts[0]] != self.tf[ts[1]] {
                    continue;
                }
                let (t0, t1) = (self.tris[ts[0]], self.tris[ts[1]]);
                let opp = |t: [u32; 3]| *t.iter().find(|v| **v != a && **v != b).unwrap();
                let (c, d) = (opp(t0), opp(t1));
                if c == d {
                    continue;
                }
                let x = |v: u32| self.x[v as usize];
                let ang = |o: V, p: V, q: V| (p - o).angle_between(q - o);
                if ang(x(c), x(a), x(b)) + ang(x(d), x(a), x(b)) <= std::f64::consts::PI + 1e-9 {
                    continue;
                }
                // keep t0's orientation: find a→b in t0
                let k = (0..3).find(|k| t0[*k] == a && t0[(k + 1) % 3] == b);
                let (a, b) = if k.is_some() { (a, b) } else { (b, a) };
                let n0 = self.tri_normal(t0) + self.tri_normal(t1);
                let (u0, u1) = ([c, a, d], [d, b, c]);
                if self.tri_normal(u0).dot(n0) <= 0.0 || self.tri_normal(u1).dot(n0) <= 0.0 {
                    continue;
                }
                // c d must not already be an edge
                if at.contains_key(&ekey(c, d)) {
                    continue;
                }
                self.tris[ts[0]] = u0;
                self.tris[ts[1]] = u1;
                touched[ts[0]] = true;
                touched[ts[1]] = true;
                flips += 1;
            }
            if flips == 0 {
                break;
            }
        }
    }

    /// Tangential relaxation, points kept on their surface / edge.
    fn relax(&mut self, geo: &Geo, iters: usize) {
        let nb = self.neighbours();
        for _ in 0..iters {
            let normals = self.normals();
            for v in 0..self.x.len() {
                let k = self.kind[v];
                let ns: Vec<V> = match k {
                    Kind::Corner => continue,
                    Kind::Face(_) => nb[v].iter().map(|j| self.x[*j as usize]).collect(),
                    Kind::Edge(c) => nb[v].iter().filter(|j| matches!(self.kind[**j as usize], Kind::Corner) || self.kind[**j as usize] == Kind::Edge(c)).map(|j| self.x[*j as usize]).collect(),
                };
                if ns.is_empty() {
                    continue;
                }
                let c = ns.iter().sum::<V>() / ns.len() as f64;
                let n = normals[v];
                let mut d = c - self.x[v];
                if let Kind::Face(_) = k {
                    d -= n * n.dot(d);
                }
                self.x[v] = geo.place(k, self.x[v] + d * 0.5);
            }
        }
    }

    fn normals(&self) -> Vec<V> {
        let mut n = vec![V::ZERO; self.x.len()];
        for t in &self.tris {
            let tn = self.tri_normal(*t);
            for v in t {
                n[*v as usize] += tn;
            }
        }
        n.iter().map(|v| v.normalize_or_zero()).collect()
    }
}

// ------------------------------------------------------------- fields

/// The rotation of `b` (4-fold symmetric, in the plane of `nb`) that best
/// matches `a`: both returned (a's representative, b's).
fn compat_o(a: V, na: V, b: V, nb: V) -> (V, V) {
    let aa = [a, na.cross(a)];
    let bb = [b, nb.cross(b)];
    let mut best = (0, 0, -1.0);
    for (i, x) in aa.iter().enumerate() {
        for (j, y) in bb.iter().enumerate() {
            let s = x.dot(*y).abs();
            if s > best.2 {
                best = (i, j, s);
            }
        }
    }
    let s = aa[best.0].dot(bb[best.1]).signum();
    (aa[best.0], bb[best.1] * if s == 0.0 { 1.0 } else { s })
}

/// A point close to both tangent planes and to the middle of the two
/// points (minimises the squared distances to the planes plus the distance
/// to the middle).
fn middle(x0: V, n0: V, x1: V, n1: V) -> V {
    let n0p0 = n0.dot(x0);
    let n0p1 = n0.dot(x1);
    let n1p0 = n1.dot(x0);
    let n1p1 = n1.dot(x1);
    let n0n1 = n0.dot(n1);
    let den = 1.0 / (1.0 - n0n1 * n0n1 + 1e-4);
    let l0 = 2.0 * (n0p1 - n0p0 - n0n1 * (n1p0 - n1p1)) * den;
    let l1 = 2.0 * (n1p0 - n1p1 - n0n1 * (n0p1 - n0p0)) * den;
    (x0 + x1) * 0.5 - (n0 * l0 + n1 * l1) * 0.25
}

fn lattice_floor(p: V, q: V, t: V, x: V, h: f64) -> (V, [i64; 2]) {
    let d = x - p;
    let i = (q.dot(d) / h).floor();
    let j = (t.dot(d) / h).floor();
    (p + q * (i * h) + t * (j * h), [i as i64, j as i64])
}

fn lattice_round(p: V, q: V, t: V, x: V, h: f64) -> V {
    let d = x - p;
    p + q * ((q.dot(d) / h).round() * h) + t * ((t.dot(d) / h).round() * h)
}

/// The pair of lattice points (one of each lattice) nearest each other
/// around the middle of the two samples, with their indices.
#[allow(clippy::too_many_arguments)]
fn compat_p(xi: V, ni: V, qi: V, pi: V, xj: V, nj: V, qj: V, pj: V, h: f64) -> ((V, [i64; 2]), (V, [i64; 2])) {
    let (ti, tj) = (ni.cross(qi), nj.cross(qj));
    let m = middle(xi, ni, xj, nj);
    let (oi, ii) = lattice_floor(pi, qi, ti, m, h);
    let (oj, ij) = lattice_floor(pj, qj, tj, m, h);
    let mut best = (f64::MAX, (oi, ii), (oj, ij));
    for a in 0..4 {
        let (ai, bi) = ((a & 1) as i64, (a >> 1) as i64);
        let pa = oi + qi * (ai as f64 * h) + ti * (bi as f64 * h);
        for b in 0..4 {
            let (aj, bj) = ((b & 1) as i64, (b >> 1) as i64);
            let pb = oj + qj * (aj as f64 * h) + tj * (bj as f64 * h);
            let d = (pa - pb).length_squared();
            if d < best.0 {
                best = (d, (pa, [ii[0] + ai, ii[1] + bi]), (pb, [ij[0] + aj, ij[1] + bj]));
            }
        }
    }
    (best.1, best.2)
}

// ------------------------------------------------------------- remesh

/// The part as quads, by field-aligned remeshing.
pub fn remesh(m: &Model, s: Settings) -> Result<(Quads, Report), String> {
    if m.triangles.is_empty() {
        return Err("no triangles".into());
    }
    // --- the exact geometry
    let surfs: Vec<Surf> = m.faces.iter().map(Surf::new).collect();
    let mut curves = vec![];
    let mut by_faces: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for e in &m.edges {
        if e.seam || e.faces[0] < 0 || e.faces[1] < 0 || e.faces[0] == e.faces[1] || e.pts.len() < 2 {
            continue;
        }
        let (a, b) = (e.faces[0] as u32, e.faces[1] as u32);
        by_faces.entry((a.min(b), a.max(b))).or_default().push(curves.len() as u32);
        curves.push(Curve { pts: e.pts.iter().map(|p| V::new(p[0] as f64, p[1] as f64, p[2] as f64)).collect() });
    }
    let pv = |p: [f32; 3]| V::new(p[0] as f64, p[1] as f64, p[2] as f64);
    let mut face_tris = vec![vec![]; m.faces.len()];
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        face_tris[*f as usize].push([pv(m.positions[t[0] as usize]), pv(m.positions[t[1] as usize]), pv(m.positions[t[2] as usize])]);
    }
    let geo = Geo { surfs, curves, by_faces, face_tris };

    // --- welded sample mesh
    let mut weld: HashMap<[i64; 3], u32> = HashMap::new();
    let mut mesh = Mesh { x: vec![], tris: vec![], tf: vec![], kind: vec![] };
    let mut remap = vec![0u32; m.positions.len()];
    for (i, p) in m.positions.iter().enumerate() {
        let k = [(p[0] as f64 * 2000.0).round() as i64, (p[1] as f64 * 2000.0).round() as i64, (p[2] as f64 * 2000.0).round() as i64];
        remap[i] = *weld.entry(k).or_insert_with(|| {
            mesh.x.push(pv(*p));
            (mesh.x.len() - 1) as u32
        });
    }
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        let t = [remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]];
        if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
            mesh.tris.push(t);
            mesh.tf.push(*f);
        }
    }
    let area: f64 = mesh.tris.iter().map(|t| mesh.tri_normal(*t).length() * 0.5).sum();
    // extraction lattice: twice the final quad (each polygon is cut in 4)
    let h = (area / (s.target_quads.max(4) as f64 / 4.0)).sqrt();
    mesh.classify(&geo);
    mesh.refine(&geo, h / 3.0);
    mesh.flip();
    mesh.relax(&geo, 4);
    mesh.flip();
    let n = mesh.x.len();
    let nb = mesh.neighbours();
    let normal = mesh.normals();
    let x = &mesh.x;
    let kind = &mesh.kind;

    // --- orientation field
    let mut o = vec![V::ZERO; n];
    let mut fixed = vec![false; n];
    for v in 0..n {
        let nv = normal[v];
        let tangent = |d: V| (d - nv * nv.dot(d)).normalize_or_zero();
        let mut dir = V::ZERO;
        match kind[v] {
            Kind::Edge(c) => {
                let along: Vec<V> = nb[v].iter().filter(|j| matches!(kind[**j as usize], Kind::Corner) || kind[**j as usize] == Kind::Edge(c)).map(|j| x[*j as usize]).collect();
                if along.len() == 2 {
                    dir = tangent(along[1] - along[0]);
                }
                fixed[v] = dir != V::ZERO;
            }
            Kind::Face(f) => {
                if let Some(d) = geo.surfs[f as usize].direction(x[v]) {
                    dir = tangent(d);
                    fixed[v] = dir != V::ZERO;
                }
            }
            Kind::Corner => {}
        }
        if dir == V::ZERO {
            // the world axis lying most in the tangent plane
            let ax = [V::X, V::Y, V::Z].into_iter().min_by(|a, b| nv.dot(*a).abs().total_cmp(&nv.dot(*b).abs())).unwrap();
            dir = tangent(ax);
            if dir == V::ZERO {
                dir = tangent(V::new(0.3, 0.5, 0.8));
            }
        }
        o[v] = dir;
    }
    for _ in 0..s.iterations {
        for v in 0..n {
            if fixed[v] {
                continue;
            }
            let nv = normal[v];
            let mut sum = o[v];
            let mut ws = 0.0;
            for j in &nb[v] {
                let (a, b) = compat_o(sum, nv, o[*j as usize], normal[*j as usize]);
                sum = a * ws + b;
                ws += 1.0;
                sum -= nv * nv.dot(sum);
                sum = sum.normalize_or_zero();
            }
            if sum != V::ZERO {
                o[v] = sum;
            }
        }
    }

    // --- position field
    // A first lattice carried outwards from the corners (nearest first,
    // along the B-rep edges before across faces): smoothing alone moves a
    // lattice's offset far too slowly (Instant Meshes uses a hierarchy).
    let mut p: Vec<V> = x.clone();
    {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let mut dist = vec![f64::MAX; n];
        let mut heap = BinaryHeap::new();
        let mut seeds: Vec<usize> = (0..n).filter(|v| kind[*v] == Kind::Corner).collect();
        if seeds.is_empty() {
            seeds.push(0);
        }
        for s in seeds {
            dist[s] = 0.0;
            heap.push(Reverse((0u64, s)));
        }
        let mut done = vec![false; n];
        while let Some(Reverse((_, i))) = heap.pop() {
            if done[i] {
                continue;
            }
            done[i] = true;
            for j in &nb[i] {
                let j = *j as usize;
                if done[j] {
                    continue;
                }
                let along = !matches!(kind[i], Kind::Face(_)) && !matches!(kind[j], Kind::Face(_));
                let d = dist[i] + (x[j] - x[i]).length() * if along { 0.3 } else { 1.0 };
                if d < dist[j] {
                    dist[j] = d;
                    let (qi, qj) = compat_o(o[i], normal[i], o[j], normal[j]);
                    let mut c = lattice_round(p[i], qi, normal[i].cross(qi), x[j], h);
                    c -= normal[j] * normal[j].dot(c - x[j]);
                    if kind[j] == Kind::Corner {
                        c = x[j];
                    } else if matches!(kind[j], Kind::Edge(_)) && fixed[j] {
                        c = x[j] + o[j] * o[j].dot(c - x[j]);
                    }
                    p[j] = lattice_round(c, qj, normal[j].cross(qj), x[j], h);
                    heap.push(Reverse((d.to_bits(), j)));
                }
            }
        }
    }
    for _ in 0..s.iterations * 2 {
        for v in 0..n {
            if kind[v] == Kind::Corner {
                p[v] = x[v];
                continue;
            }
            let (nv, xv) = (normal[v], x[v]);
            let mut sum = p[v];
            let mut ws = 0.0;
            for j in &nb[v] {
                let j = *j as usize;
                let (_, qj) = compat_o(o[v], nv, o[j], normal[j]);
                let ((a, _), (b, _)) = compat_p(xv, nv, o[v], sum, x[j], normal[j], qj, p[j], h);
                sum = (a * ws + b) / (ws + 1.0);
                ws += 1.0;
                sum -= nv * nv.dot(sum - xv);
            }
            if let Kind::Edge(_) = kind[v] {
                if fixed[v] {
                    sum = xv + o[v] * o[v].dot(sum - xv);
                }
            }
            p[v] = lattice_round(sum, o[v], nv.cross(o[v]), xv, h);
        }
    }

    if std::env::var("FORME_REMESH_DEBUG").is_ok() {
        let distinct: HashSet<[i64; 3]> = p.iter().map(|q| [(q.x * 100.0).round() as i64, (q.y * 100.0).round() as i64, (q.z * 100.0).round() as i64]).collect();
        let far = (0..n).map(|v| (p[v] - x[v]).length()).fold(0.0f64, f64::max);
        eprintln!("distinct lattice points {}, max |p - x| {far:.2}", distinct.len());
    }
    // --- extraction: merge samples on the same lattice point
    let mut uf: Vec<u32> = (0..n as u32).collect();
    fn find(uf: &mut [u32], mut a: u32) -> u32 {
        while uf[a as usize] != a {
            uf[a as usize] = uf[uf[a as usize] as usize];
            a = uf[a as usize];
        }
        a
    }
    let mut steps = vec![];
    let (mut dbg_hist, mut dbg_gap, mut dbg_ori) = ([0usize; 4], 0.0f64, 0.0f64);
    // a cluster holds at most one corner
    let mut has_corner: Vec<bool> = (0..n).map(|v| kind[v] == Kind::Corner).collect();
    for v in 0..n {
        for j in &nb[v] {
            let j = *j as usize;
            if j <= v {
                continue;
            }
            let (qi, qj) = compat_o(o[v], normal[v], o[j], normal[j]);
            let ((pa, ii), (pb, ij)) = compat_p(x[v], normal[v], qi, p[v], x[j], normal[j], qj, p[j], h);
            let d = [ii[0] - ij[0], ii[1] - ij[1]];
            dbg_hist[((d[0].abs() + d[1].abs()) as usize).min(3)] += 1;
            if d == [0, 0] { dbg_gap += (pa - pb).length(); }
            dbg_ori += 1.0 - qi.dot(qj).abs();
            if d == [0, 0] {
                let (a, b) = (find(&mut uf, v as u32), find(&mut uf, j as u32));
                if a == b {
                    continue;
                }
                if has_corner[a as usize] && has_corner[b as usize] {
                    steps.push((v as u32, j as u32));
                    continue;
                }
                let (r, o2) = (a.min(b), a.max(b));
                uf[o2 as usize] = r;
                has_corner[r as usize] |= has_corner[o2 as usize];
            } else if d[0].abs() + d[1].abs() == 1 {
                steps.push((v as u32, j as u32));
            }
        }
    }
    if std::env::var("FORME_REMESH_DEBUG").is_ok() {
        eprintln!("steps by size {dbg_hist:?}, gap on merges {:.3}, orientation mismatch {:.4}", dbg_gap / dbg_hist[0].max(1) as f64, dbg_ori / dbg_hist.iter().sum::<usize>() as f64);
    }
    // clusters
    let mut cid: HashMap<u32, u32> = HashMap::new();
    let mut members: Vec<Vec<u32>> = vec![];
    let mut of = vec![0u32; n];
    for v in 0..n as u32 {
        let r = find(&mut uf, v);
        let c = *cid.entry(r).or_insert_with(|| {
            members.push(vec![]);
            (members.len() - 1) as u32
        });
        members[c as usize].push(v);
        of[v as usize] = c;
    }
    let nc = members.len();
    let mut cpos = vec![V::ZERO; nc];
    let mut cnorm = vec![V::ZERO; nc];
    let mut ckind = vec![Kind::Corner; nc];
    for (c, ms) in members.iter().enumerate() {
        let corner = ms.iter().find(|v| kind[**v as usize] == Kind::Corner);
        let avg = ms.iter().map(|v| p[*v as usize]).sum::<V>() / ms.len() as f64;
        cnorm[c] = ms.iter().map(|v| normal[*v as usize]).sum::<V>().normalize_or_zero();
        if let Some(k) = corner {
            ckind[c] = Kind::Corner;
            cpos[c] = x[*k as usize];
            continue;
        }
        // an edge sample wins over face samples; the most common one
        let mut count: HashMap<Kind, usize> = HashMap::new();
        for v in ms {
            *count.entry(kind[*v as usize]).or_default() += 1;
        }
        let k = count
            .iter()
            .max_by_key(|(k, c)| (matches!(k, Kind::Edge(_)) as usize, **c))
            .map(|(k, _)| *k)
            .unwrap();
        ckind[c] = k;
        cpos[c] = geo.place(k, avg);
    }
    // graph
    let mut adj: Vec<Vec<u32>> = vec![vec![]; nc];
    for (a, b) in steps {
        let (ca, cb) = (of[a as usize], of[b as usize]);
        if ca != cb && !adj[ca as usize].contains(&cb) {
            adj[ca as usize].push(cb);
            adj[cb as usize].push(ca);
        }
    }
    // neighbours in counter-clockwise order around each cluster's normal
    for c in 0..nc {
        let nrm = cnorm[c];
        let u = (if nrm.x.abs() < 0.9 { V::X } else { V::Y }).cross(nrm).normalize_or_zero();
        let w = nrm.cross(u);
        let at = cpos[c];
        let ang = |d: &u32| {
            let e = cpos[*d as usize] - at;
            e.dot(w).atan2(e.dot(u))
        };
        adj[c].sort_by(|a, b| ang(a).total_cmp(&ang(b)));
    }
    if std::env::var("FORME_REMESH_DEBUG").is_ok() {
        let mut deg = [0usize; 8];
        for a in &adj {
            deg[a.len().min(7)] += 1;
        }
        let sizes = members.iter().map(|m| m.len()).sum::<usize>() as f64 / nc as f64;
        eprintln!("clusters {nc} (avg {sizes:.1} samples), degrees {deg:?}, h {h:.2}");
    }
    // faces of the graph
    let mut used: HashSet<(u32, u32)> = HashSet::new();
    let mut polys: Vec<Vec<u32>> = vec![];
    let mut report = Report { samples: n, quad_size: h / 2.0, ..Default::default() };
    for a in 0..nc as u32 {
        for &b in &adj[a as usize].clone() {
            if used.contains(&(a, b)) {
                continue;
            }
            let mut poly = vec![a];
            let (mut u, mut v) = (a, b);
            let mut ok = false;
            for _ in 0..64 {
                used.insert((u, v));
                let ring = &adj[v as usize];
                let i = ring.iter().position(|w| *w == u).unwrap();
                let next = ring[(i + ring.len() - 1) % ring.len()];
                if v == a {
                    ok = true;
                    break;
                }
                poly.push(v);
                u = v;
                v = next;
                if used.contains(&(u, v)) && v != a {
                    break;
                }
            }
            if !ok || poly.len() < 3 {
                report.holes += 1;
                continue;
            }
            if poly.len() > 12 {
                report.holes += 1;
                continue;
            }
            report.polygons[(poly.len() - 3).min(3)] += 1;
            polys.push(poly);
        }
    }

    // --- every polygon cut into quads
    let mut out_p: Vec<V> = cpos.clone();
    let mut out_k: Vec<Kind> = ckind.clone();
    let mut mids: HashMap<(u32, u32), u32> = HashMap::new();
    let mut quads = vec![];
    let mut centres = vec![];
    // nearest face sample (for points with no face yet)
    let cell = h / 2.0;
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let gkey = |q: V| [(q.x / cell).floor() as i64, (q.y / cell).floor() as i64, (q.z / cell).floor() as i64];
    for v in 0..n {
        if let Kind::Face(_) = kind[v] {
            grid.entry(gkey(x[v])).or_default().push(v as u32);
        }
    }
    let face_at = |q: V| -> Option<u32> {
        let k = gkey(q);
        let mut best = (f64::MAX, None);
        for r in 1..6i64 {
            for dx in -r..=r {
                for dy in -r..=r {
                    for dz in -r..=r {
                        if let Some(vs) = grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
                            for v in vs {
                                let d = (x[*v as usize] - q).length_squared();
                                if d < best.0 {
                                    best = (d, Some(*v));
                                }
                            }
                        }
                    }
                }
            }
            if best.1.is_some() {
                break;
            }
        }
        best.1.map(|v| match kind[v as usize] {
            Kind::Face(f) => f,
            _ => 0,
        })
    };
    let near_curve = |q: V| -> Option<u32> {
        let mut best = (f64::MAX, 0u32);
        for (i, c) in geo.curves.iter().enumerate() {
            let d = c.project(q).1;
            if d < best.0 {
                best = (d, i as u32);
            }
        }
        (best.0 < h * 0.15).then_some(best.1)
    };
    for poly in &polys {
        let k = poly.len();
        let ctr = poly.iter().map(|c| cpos[*c as usize]).sum::<V>() / k as f64;
        let f = face_at(ctr).unwrap_or(0);
        let ci = out_p.len() as u32;
        out_p.push(geo.place(Kind::Face(f), ctr));
        out_k.push(Kind::Face(f));
        centres.push(f);
        let mut mid = |a: u32, b: u32, out_p: &mut Vec<V>, out_k: &mut Vec<Kind>| -> u32 {
            *mids.entry(ekey(a, b)).or_insert_with(|| {
                let q = (cpos[a as usize] + cpos[b as usize]) * 0.5;
                let feat = |c: u32| !matches!(ckind[c as usize], Kind::Face(_));
                let kk = if feat(a) && feat(b) { near_curve(q).map(Kind::Edge) } else { None }.unwrap_or_else(|| Kind::Face(face_at(q).unwrap_or(f)));
                out_p.push(geo.place(kk, q));
                out_k.push(kk);
                (out_p.len() - 1) as u32
            })
        };
        for i in 0..k {
            let (prev, cur, next) = (poly[(i + k - 1) % k], poly[i], poly[(i + 1) % k]);
            let m0 = mid(cur, next, &mut out_p, &mut out_k);
            let m1 = mid(prev, cur, &mut out_p, &mut out_k);
            quads.push([cur, m0, ci, m1]);
        }
    }

    // --- relax the quads on the exact geometry
    let mut qnb: Vec<Vec<u32>> = vec![vec![]; out_p.len()];
    for q in &quads {
        for i in 0..4 {
            let (a, b) = (q[i], q[(i + 1) % 4]);
            if !qnb[a as usize].contains(&b) {
                qnb[a as usize].push(b);
                qnb[b as usize].push(a);
            }
        }
    }
    for _ in 0..8 {
        for v in 0..out_p.len() {
            let k = out_k[v];
            let ns: Vec<V> = match k {
                Kind::Corner => continue,
                Kind::Face(_) => qnb[v].iter().map(|j| out_p[*j as usize]).collect(),
                Kind::Edge(c) => qnb[v].iter().filter(|j| matches!(out_k[**j as usize], Kind::Corner) || out_k[**j as usize] == Kind::Edge(c)).map(|j| out_p[*j as usize]).collect(),
            };
            if ns.is_empty() || (matches!(k, Kind::Edge(_)) && ns.len() != 2) {
                continue;
            }
            let c = ns.iter().sum::<V>() / ns.len() as f64;
            let target = out_p[v] + (c - out_p[v]) * 0.5;
            let kk = if let Kind::Face(_) = k { Kind::Face(face_at(target).unwrap_or(0)) } else { k };
            out_k[v] = kk;
            out_p[v] = geo.place(kk, target);
        }
    }

    let quad_face: Vec<u32> = polys.iter().zip(&centres).flat_map(|(poly, f)| std::iter::repeat(*f).take(poly.len())).collect();
    Ok((
        Quads {
            positions: out_p.iter().map(|q| [q.x as f32, q.y as f32, q.z as f32]).collect(),
            quads,
            quad_face,
            skipped: vec![],
        },
        report,
    ))
}

/// Wavefront OBJ of remeshed quads.
pub fn obj(q: &Quads, face_name: impl Fn(u32) -> String) -> String {
    let mut s = String::from("# Forme NURBS: export converter (field-aligned quads)\n");
    for p in &q.positions {
        s.push_str(&format!("v {:.5} {:.5} {:.5}\n", p[0], p[1], p[2]));
    }
    let mut last = u32::MAX;
    for (f, face) in q.quads.iter().zip(&q.quad_face) {
        if *face != last {
            s.push_str(&format!("g {}\n", face_name(*face)));
            last = *face;
        }
        s.push_str(&format!("f {} {} {} {}\n", f[0] + 1, f[1] + 1, f[2] + 1, f[3] + 1));
    }
    s
}

/// Every quad edge used by exactly two quads, each way once.
pub fn is_closed(q: &Quads) -> bool {
    let mut uses: HashMap<(u32, u32), i32> = HashMap::new();
    for f in &q.quads {
        for k in 0..4 {
            *uses.entry((f[k], f[(k + 1) % 4])).or_default() += 1;
        }
    }
    uses.iter().all(|((a, b), n)| *n == 1 && uses.get(&(*b, *a)) == Some(&1))
}

#[cfg(all(test, feature = "occ"))]
mod tests {
    use super::*;
    use crate::model::{Edge, EdgeKind, Face};
    use forme_occt::Shape;

    pub(crate) fn model_of(s: &Shape) -> Model {
        let t = s.topo(0.02, 0.2).unwrap();
        let surf = |k: forme_occt::SurfKind| match k {
            forme_occt::SurfKind::Plane => SurfKind::Plane,
            forme_occt::SurfKind::Cylinder => SurfKind::Cylinder,
            forme_occt::SurfKind::Cone => SurfKind::Cone,
            forme_occt::SurfKind::Sphere => SurfKind::Sphere,
            forme_occt::SurfKind::Torus => SurfKind::Torus,
            forme_occt::SurfKind::Other => SurfKind::Other,
        };
        Model {
            positions: t.positions,
            normals: t.normals,
            triangles: t.triangles,
            tri_face: t.tri_face,
            faces: t.faces.iter().map(|f| Face { tag: None, kind: surf(f.info.kind), point: f.info.point, normal: f.info.normal, axis_origin: f.info.axis_origin, axis_dir: f.info.axis_dir, radius: f.info.radius }).collect(),
            edges: t.edges.iter().map(|e| Edge { index: e.index, faces: e.faces, kind: EdgeKind::Line, seam: e.seam, pts: e.pts.iter().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect() }).collect(),
            ..Default::default()
        }
    }

    fn edge_between(s: &Shape, a: [f64; 3], b: [f64; 3]) -> i32 {
        let m = V::from(a).lerp(V::from(b), 0.5);
        let d = |e: &forme_occt::Edge| (V::from(e.pts[e.pts.len() / 2]) - m).length();
        s.edges(9).iter().min_by(|x, y| d(x).total_cmp(&d(y))).unwrap().index
    }

    fn run(name: &str, s: &Shape, target: usize) -> (Quads, Report) {
        let m = model_of(s);
        eprintln!("kinds {:?}", m.faces.iter().map(|f| f.kind).collect::<Vec<_>>());
        let target = std::env::var("FORME_REMESH_TARGET").ok().and_then(|s| s.parse().ok()).unwrap_or(target);
        let (q, r) = remesh(&m, Settings { target_quads: target, ..Default::default() }).unwrap();
        let dir = std::env::var("FORME_REMESH_OUT").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        std::fs::write(dir.join(format!("forme-remesh-{name}.obj")), obj(&q, |f| if m.faces[f as usize].kind == SurfKind::Plane { format!("face{f}") } else { format!("fillet{f}") })).unwrap();
        eprintln!("{name}: {} quads, {} samples, quad {:.2} mm, polygons {:?}, holes {}, closed {}", q.quads.len(), r.samples, r.quad_size, r.polygons, r.holes, is_closed(&q));
        // distance of the quad points to the B-rep's own triangles
        let pv = |p: [f32; 3]| V::new(p[0] as f64, p[1] as f64, p[2] as f64);
        let tris: Vec<[V; 3]> = m.triangles.iter().map(|t| [pv(m.positions[t[0] as usize]), pv(m.positions[t[1] as usize]), pv(m.positions[t[2] as usize])]).collect();
        let dev: Vec<f64> = q.positions.iter().map(|p| tris.iter().map(|t| (closest_on_tri(pv(*p), t[0], t[1], t[2]) - pv(*p)).length()).fold(f64::MAX, f64::min)).collect();
        let worst = dev.iter().cloned().fold(0.0, f64::max);
        eprintln!("{name}: off the surface: worst {worst:.3} mm, {} points over 0.05 mm", dev.iter().filter(|d| **d > 0.05).count());
        (q, r)
    }

    #[test]
    fn test_part() {
        // the capture harness's part: block, hole, boss, slot
        use crate::doc::{Doc, Op, Prim};
        use glam::{DQuat, DVec3};
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-10.0, -5.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Add, Prim::Cylinder { radius: 7.0, height: 10.0 }, DVec3::new(10.0, 20.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Box { x: 8.0, y: 8.0, z: 40.0 }, DVec3::new(0.0, 14.0, 0.0), DQuat::IDENTITY);
        let m = crate::build::build(&d, true, false, &[], &mut crate::build::Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let target = std::env::var("FORME_REMESH_TARGET").ok().and_then(|s| s.parse().ok()).unwrap_or(3000);
        let (q, r) = remesh(&m, Settings { target_quads: target, ..Default::default() }).unwrap();
        eprintln!("part: {} quads, polygons {:?}, holes {}, closed {}", q.quads.len(), r.polygons, r.holes, is_closed(&q));
        let dir = std::env::var("FORME_REMESH_OUT").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        std::fs::write(dir.join("forme-remesh-part.obj"), obj(&q, |f| if m.faces[f as usize].kind == SurfKind::Plane { format!("face{f}") } else { format!("fillet{f}") })).unwrap();
        assert!(q.quads.len() > target / 2);
    }

    #[test]
    fn cube_with_fillet() {
        let cube = Shape::cuboid(20.0, 20.0, 20.0).unwrap();
        let e = edge_between(&cube, [-10.0, 10.0, 10.0], [10.0, 10.0, 10.0]);
        let one = cube.fillet(&[e], 4.0, 99).unwrap();
        let (q, _) = run("one", &one, 600);
        assert!(q.quads.len() > 200);
        let ups: Vec<i32> = [(-10.0, -10.0), (10.0, -10.0), (10.0, 10.0), (-10.0, 10.0)].iter().map(|(x, z)| edge_between(&cube, [*x, -10.0, *z], [*x, 10.0, *z])).collect();
        let four = cube.fillet(&ups, 4.0, 99).unwrap();
        run("four", &four, 800);
        let holed = cube.cut(&Shape::cylinder(4.0, 30.0).unwrap()).unwrap();
        run("holed", &holed, 800);
    }
}
