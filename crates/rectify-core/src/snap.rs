//! Putting the edges back (Romain, 18 nov.: "je me fiche du micro bruit,
//! c'est plus les lignes droites, fillets"): only the vertices near the
//! edges move, the inside of the faces is left as it is.
//!
//! - Each pair of regions that meet (directly, or across a strip of free
//!   faces: the melted edge of an AI mesh) is an edge between two surfaces
//!   (plane, cylinder or sphere): rounded by a rolling-ball fillet of
//!   constant radius (fitted on the mesh, then the radii of the whole part
//!   grouped and rounded), or sharp. Two surfaces meeting almost tangent are
//!   each other's continuation (a fillet found as a narrow cylinder).
//! - Where three regions meet, their edges are blended by distance, so the
//!   corner is one smooth piece instead of a tear.
//! - Near an edge the faces are put on their surface, fading out over a
//!   short distance (Fade); further in nothing moves, unless Flatten asks.
//! - The moves are then carried smoothly into what is left free.

use crate::mesh::{Mesh, Topo, NONE};
use crate::prim::Prim;
use crate::segment::Segmentation;
use glam::DVec3;
use rayon::prelude::*;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edges {
    /// Fillets of the radius fitted on the mesh.
    Fillet,
    /// Sharp edges (radius 0).
    Sharp,
    /// Left as they are (only the faces near the borders are snapped).
    Soft,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapParams {
    pub edges: Edges,
    /// Widest strip between two regions still taken for one edge (mm).
    pub gap: f64,
    /// Width over which the faces' move fades beyond an edge (mm).
    pub fade: f64,
    /// Smallest angle between two surfaces that makes an edge (degrees);
    /// below, they continue each other.
    pub edge_angle: f64,
    /// A vertex never moves further than this (mm).
    pub max_move: f64,
    /// How far the mesh may be from an edge's model for it to be rebuilt
    /// (mm, RMS).
    pub edge_tol: f64,
    /// The fillet radii of the part grouped (same radius) and rounded to
    /// this step (mm); 0: each its own.
    pub round: f64,
    /// Smallest fillet radius (mm): below about two triangles a fillet
    /// shades as a zigzag. Sharp edges ignore it.
    pub min_radius: f64,
    /// Share of the move given to the inside of the faces (0: untouched).
    pub flatten: f64,
    /// Smoothing passes that carry the moves into the free areas.
    pub relax: usize,
}

impl Default for SnapParams {
    fn default() -> Self {
        SnapParams { edges: Edges::Fillet, gap: 2.5, fade: 2.0, edge_angle: 25.0, max_move: 0.8, edge_tol: 0.15, round: 0.05, min_radius: 0.3, flatten: 0.0, relax: 80 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SnapReport {
    /// Edges rebuilt (fillet or sharp), and their radii (mm).
    pub edges: usize,
    pub radii: Vec<f64>,
    /// Groups of equal radii.
    pub radius_groups: usize,
    /// Pairs of regions that continue each other (no edge between them).
    pub tangent: usize,
    /// Pairs whose strip did not look like an edge (left alone).
    pub rejected: usize,
    /// Vertices put on an edge, on a face near an edge (fading), left free,
    /// and where neighbours disagreed (eased).
    pub on_edge: usize,
    pub faded: usize,
    pub free: usize,
    pub eased: usize,
    pub mean_move: f64,
    pub max_move: f64,
}

#[derive(Clone, Copy, PartialEq)]
struct Item {
    d: f64,
    v: u32,
    l: u32,
}
impl Eq for Item {}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        o.d.partial_cmp(&self.d).unwrap_or(Ordering::Equal)
    }
}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

pub type Near = [(f64, u32); 3];

/// For each vertex, the three nearest regions (geodesic distance along the
/// mesh, from the vertices inside each region), up to `cap` mm.
pub fn three_nearest(m: &Mesh, topo: &Topo, inner: &[u32], cap: f64) -> Vec<Near> {
    let nv = m.pos.len();
    let mut best = vec![[(f64::INFINITY, NONE); 3]; nv];
    let mut heap = BinaryHeap::new();
    for v in 0..nv {
        if inner[v] != NONE {
            heap.push(Item { d: 0.0, v: v as u32, l: inner[v] });
        }
    }
    let has = |b: &Near, l: u32| b.iter().any(|x| x.1 == l);
    while let Some(Item { d, v, l }) = heap.pop() {
        let b = &mut best[v as usize];
        if has(b, l) || b[2].0 <= d {
            continue;
        }
        if d < b[0].0 {
            b[2] = b[1];
            b[1] = b[0];
            b[0] = (d, l);
        } else if d < b[1].0 {
            b[2] = b[1];
            b[1] = (d, l);
        } else {
            b[2] = (d, l);
        }
        for &w in topo.verts_of(v as usize) {
            let nd = d + (m.pos[w as usize] - m.pos[v as usize]).length();
            if nd > cap {
                continue;
            }
            let bw = &best[w as usize];
            if has(bw, l) || bw[2].0 <= nd {
                continue;
            }
            heap.push(Item { d: nd, v: w, l });
        }
    }
    best
}

/// Geodesic distance to the nearest vertex for which `src` is true, up to `cap`.
#[allow(dead_code)]
fn distance_to(m: &Mesh, topo: &Topo, src: &[bool], cap: f64) -> Vec<f64> {
    let nv = m.pos.len();
    let mut dist = vec![f64::INFINITY; nv];
    let mut heap = BinaryHeap::new();
    for v in 0..nv {
        if src[v] {
            dist[v] = 0.0;
            heap.push(Item { d: 0.0, v: v as u32, l: 0 });
        }
    }
    while let Some(Item { d, v, .. }) = heap.pop() {
        if d > dist[v as usize] {
            continue;
        }
        for &w in topo.verts_of(v as usize) {
            let nd = d + (m.pos[w as usize] - m.pos[v as usize]).length();
            if nd < dist[w as usize] && nd <= cap {
                dist[w as usize] = nd;
                heap.push(Item { d: nd, v: w, l: 0 });
            }
        }
    }
    dist
}

/// The vertices whose faces all belong to one region: that region, else NONE.
pub fn inner_vertices(topo: &Topo, label: &[u32], nv: usize) -> Vec<u32> {
    (0..nv)
        .into_par_iter()
        .map(|v| {
            let ts = topo.tris_of(v);
            let l = ts.first().map(|&t| label[t as usize]).unwrap_or(NONE);
            if l != NONE && ts.iter().all(|&t| label[t as usize] == l) {
                l
            } else {
                NONE
            }
        })
        .collect()
}

/// A region's surface with its outside: `o` is +1 when the outward normal is
/// the shape's own (`Prim::normal_at`: the plane's normal, away from a
/// cylinder's axis or a sphere's centre), −1 for a hole or a hollow.
#[derive(Clone, Copy, Debug)]
pub struct Surf {
    pub prim: Prim,
    pub o: f64,
}

impl Surf {
    /// Orients the shape like the region's faces.
    pub fn new(prim: Prim, m: &Mesh, faces: &[u32]) -> Surf {
        let mut s = 0.0;
        for &f in faces.iter().step_by((faces.len() / 400).max(1)) {
            let t = f as usize;
            s += m.tri_cross(t).dot(prim.normal_at(m.tri_centre(t)));
        }
        Surf { prim, o: if s < 0.0 { -1.0 } else { 1.0 } }
    }

    /// Signed distance, positive outside the matter.
    pub fn sd(&self, x: DVec3) -> f64 {
        self.o
            * match self.prim {
                Prim::Plane { n, d } => n.dot(x) - d,
                Prim::Cylinder { p, a, r } => {
                    let v = x - p;
                    (v - a * a.dot(v)).length() - r
                }
                Prim::Sphere { c, r } => (x - c).length() - r,
            }
    }

    /// Outward unit normal.
    pub fn grad(&self, x: DVec3) -> DVec3 {
        self.prim.normal_at(x) * self.o
    }

    pub fn project(&self, x: DVec3) -> DVec3 {
        self.prim.project(x)
    }
}

/// Where a vertex goes on an edge.
#[derive(Clone, Copy, Debug)]
struct Spot {
    q: DVec3,
    /// How far `q` lies on a face beyond the fillet's tangent line (0 on
    /// the fillet).
    s: f64,
    /// 0 on the fillet, 1 on the first surface, 2 on the second.
    side: u8,
    /// The middle of the fillet (the edge line when sharp) at the vertex's
    /// place along the edge, and the vertex's distance to it across.
    ridge: DVec3,
    rd: f64,
}

/// A vertex near a thin edge: which edge, its side, the ridge point.
#[derive(Clone, Copy, Debug)]
struct Ridge {
    key: (u32, u32),
    side: u8,
    at: DVec3,
    d: f64,
}

/// Where two surfaces meet.
#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub a: Surf,
    pub b: Surf,
    /// −1: an outer edge (the matter inside both), +1: an inner one.
    pub s: f64,
    /// Fillet radius (0: sharp).
    pub r: f64,
    /// The two surfaces continue each other: no edge, each point goes to
    /// the nearer one.
    pub tangent: bool,
    /// Strip points the edge was fitted on (its weight when radii are grouped).
    pub n: usize,
    /// The mesh's edge length: a fillet narrower than this cannot be drawn
    /// by the triangles, the vertices nearest to its ridge go onto it.
    pub h: f64,
}

impl Edge {
    /// None when the two surfaces neither cross nor touch along the strip
    /// (parallel faces of a step, with the step's wall missing between).
    fn new(a: Surf, b: Surf, pts: &[DVec3], min_angle: f64, touch: f64, h: f64) -> Option<Edge> {
        // the angle between the surfaces along the strip
        let mut cs: Vec<f64> = pts.iter().map(|&x| a.grad(a.project(x)).dot(b.grad(b.project(x)))).collect();
        cs.sort_by(|x, y| x.partial_cmp(y).unwrap_or(Ordering::Equal));
        let c = cs.get(cs.len() / 2).copied().unwrap_or(1.0);
        let tangent = c > min_angle.to_radians().cos();
        if tangent {
            // continuing each other: they must meet, not run side by side
            let mut gaps: Vec<f64> = pts.iter().map(|&x| (a.project(x) - b.project(x)).length()).collect();
            gaps.sort_by(|x, y| x.partial_cmp(y).unwrap_or(Ordering::Equal));
            let low = gaps.get(gaps.len() / 10).copied().unwrap_or(f64::INFINITY);
            if low > touch {
                return None;
            }
        }
        // an outer edge: the melted strip lies inside both
        let inside = pts.iter().filter(|&&x| a.sd(x) < 0.0 && b.sd(x) < 0.0).count();
        let s = if inside * 2 >= pts.len() { -1.0 } else { 1.0 };
        Some(Edge { a, b, s, r: 0.0, tangent, n: pts.len(), h })
    }

    /// The point of the fillet's spine (the line of the rolling ball's centre;
    /// the edge line itself when sharp) near `x`: Gauss-Newton onto the two
    /// offset surfaces.
    fn spine(&self, x: DVec3) -> Option<DVec3> {
        let off = self.s * self.r;
        let mut c = x;
        for _ in 0..20 {
            let (fa, fb) = (self.a.sd(c) - off, self.b.sd(c) - off);
            let (ga, gb) = (self.a.grad(c), self.b.grad(c));
            let g = ga.dot(gb);
            let det = 1.0 - g * g;
            if det < 1e-6 {
                return None;
            }
            let l0 = (fa - g * fb) / det;
            let l1 = (fb - g * fa) / det;
            let d = ga * l0 + gb * l1;
            c -= d;
            if d.length_squared() < 1e-16 {
                break;
            }
        }
        let ok = c.is_finite() && (self.a.sd(c) - off).abs() < 1e-3 && (self.b.sd(c) - off).abs() < 1e-3;
        ok.then_some(c)
    }

    /// The point of the rebuilt edge nearest to `x`.
    fn locate(&self, x: DVec3) -> Option<Spot> {
        let pa = self.a.project(x);
        let pb = self.b.project(x);
        let a_nearer = (pa - x).length_squared() <= (pb - x).length_squared();
        if self.tangent {
            let (q, side) = if a_nearer { (pa, 1) } else { (pb, 2) };
            return Some(Spot { q, s: 0.0, side, ridge: q, rd: f64::INFINITY });
        }
        let c = self.spine(x)?;
        let (na, nb) = (self.a.grad(c), self.b.grad(c));
        let off = self.s * self.r;
        let (ta, tb) = (c - na * off, c - nb * off);
        // along each face, away from the edge
        let g = na.dot(nb);
        let ua = ((nb - na * g) * self.s).normalize_or_zero();
        let ub = ((na - nb * g) * self.s).normalize_or_zero();
        // the ridge: the middle of the fillet (the edge line when sharp), at
        // x's place along the edge
        let l = na.cross(nb).normalize_or_zero();
        let mid = c - (na + nb).normalize_or_zero() * (self.s * self.r);
        let v = x - mid;
        let ridge = mid + l * l.dot(v);
        let rd = (v - l * l.dot(v)).length();
        let sa = (pa - ta).dot(ua);
        let sb = (pb - tb).dot(ub);
        let (q, s, side) = match (sa >= 0.0, sb >= 0.0) {
            (true, true) => {
                if a_nearer {
                    (pa, sa, 1)
                } else {
                    (pb, sb, 2)
                }
            }
            (true, false) => (pa, sa, 1),
            (false, true) => (pb, sb, 2),
            _ => {
                if self.r <= 1e-9 {
                    (ridge, 0.0, 0)
                } else {
                    let w = x - c;
                    let w = w - l * l.dot(w);
                    let len = w.length();
                    if len < 1e-12 {
                        return None;
                    }
                    (c + w * (self.r / len), 0.0, 0)
                }
            }
        };
        Some(Spot { q, s, side, ridge, rd })
    }

    /// The point of the rebuilt edge nearest to `x`.
    pub fn closest(&self, x: DVec3) -> Option<DVec3> {
        self.locate(x).map(|l| l.q)
    }

    /// Where a vertex goes and how much of the way (1 on the edge, fading to
    /// 0 over `fade` beyond the tangent lines). `da`, `db`: its distances to
    /// the two regions; two surfaces that continue each other are blended
    /// smoothly across their junction rather than creased.
    fn place(&self, x: DVec3, da: f64, db: f64, fade: f64) -> Option<(Spot, f64)> {
        let fade = fade.max(1e-3);
        if self.tangent {
            let (pa, pb) = (self.a.project(x), self.b.project(x));
            let bw = fade.max(0.3);
            let t = smoothstep(0.5 + (da - db) / (2.0 * bw));
            let q = pa * (1.0 - t) + pb * t;
            let w = 1.0 - smoothstep(((da - db).abs() * 0.5 - fade * 0.5) / fade);
            return Some((Spot { q, s: 0.0, side: if t < 0.5 { 1 } else { 2 }, ridge: q, rd: f64::INFINITY }, w));
        }
        let sp = self.locate(x)?;
        let w = 1.0 - smoothstep((sp.s - fade * 0.5) / fade);
        Some((sp, w))
    }

    /// Too thin for the triangles to draw: its ridge is made a line of
    /// vertices instead (see `snap`).
    fn thin(&self) -> bool {
        !self.tangent && self.r < self.h * 1.5
    }

    /// Mean squared distance of the points to the edge.
    fn err(&self, pts: &[DVec3]) -> f64 {
        let mut e = 0.0;
        for p in pts {
            e += match self.closest(*p) {
                Some(q) => (q - *p).length_squared(),
                None => 1.0,
            };
        }
        e / pts.len().max(1) as f64
    }

    /// The radius that fits the points best (golden section on [lo, hi]).
    fn fit_radius(&mut self, pts: &[DVec3], lo: f64, hi: f64) {
        let g = 0.618_034;
        let (mut a, mut b) = (lo, hi.max(lo));
        for _ in 0..30 {
            let x1 = b - g * (b - a);
            let x2 = a + g * (b - a);
            self.r = x1;
            let e1 = self.err(pts);
            self.r = x2;
            let e2 = self.err(pts);
            if e1 < e2 {
                b = x2;
            } else {
                a = x1;
            }
        }
        self.r = (a + b) * 0.5;
    }
}

/// The strip of each pair of regions: the vertices whose two nearest
/// regions they are, both within `gap`.
fn strips(pos: &[DVec3], near: &[Near], gap: f64) -> HashMap<(u32, u32), Vec<DVec3>> {
    let mut out: HashMap<(u32, u32), Vec<DVec3>> = HashMap::new();
    for (v, nb) in near.iter().enumerate() {
        let [(d1, l1), (d2, l2), _] = *nb;
        if l1 == NONE || l2 == NONE || d1 + d2 > gap {
            continue;
        }
        out.entry((l1.min(l2), l1.max(l2))).or_default().push(pos[v]);
    }
    out
}

/// Every edge of the part: one per pair of regions whose strip looks like
/// one. Returns the edges and how many pairs were left alone.
pub fn find_edges(m: &Mesh, seg: &Segmentation, near: &[Near], p: &SnapParams) -> (HashMap<(u32, u32), Edge>, usize) {
    let surfs: Vec<Surf> = seg.regions.par_iter().map(|r| Surf::new(r.prim, m, &r.faces)).collect();
    let h = m.edge_median();
    let list: Vec<((u32, u32), Vec<DVec3>)> = strips(&m.pos, near, p.gap).into_iter().filter(|(_, pts)| pts.len() >= 8).collect();
    let found: Vec<(Option<((u32, u32), Edge)>, bool)> = list
        .par_iter()
        .map(|(k, pts)| {
            // at most 600 points to fit on
            let step = (pts.len() / 600).max(1);
            let fit_pts: Vec<DVec3> = pts.iter().step_by(step).copied().collect();
            let Some(mut e) = Edge::new(surfs[k.0 as usize], surfs[k.1 as usize], &fit_pts, p.edge_angle, p.edge_tol * 2.0, h) else { return (None, true) };
            if !e.tangent && p.edges == Edges::Fillet {
                e.fit_radius(&fit_pts, p.min_radius, p.gap * 0.6);
            }
            e.n = pts.len();
            let rms = e.err(&fit_pts).sqrt();
            if rms < p.edge_tol {
                (Some((*k, e)), false)
            } else {
                (None, true)
            }
        })
        .collect();
    let rejected = found.iter().filter(|x| x.1).count();
    (found.into_iter().filter_map(|x| x.0).collect(), rejected)
}

/// The radii of the part grouped: radii within a few hundredths of a mm (or
/// 12 %) share their mean, then rounded to `step`. An edge the shared radius
/// fits much worse keeps its own (rounded). Returns the number of groups.
fn group_radii(edges: &mut HashMap<(u32, u32), Edge>, strips: &HashMap<(u32, u32), Vec<DVec3>>, step: f64, lo: f64) -> usize {
    let mut keys: Vec<(u32, u32)> = edges.iter().filter(|(_, e)| !e.tangent && e.r > 0.0).map(|(k, _)| *k).collect();
    keys.sort_by(|a, b| edges[a].r.partial_cmp(&edges[b].r).unwrap_or(Ordering::Equal));
    let mut groups: Vec<Vec<(u32, u32)>> = vec![];
    for k in keys {
        let r = edges[&k].r;
        let joins = match groups.last() {
            Some(g) => {
                let first = edges[&g[0]].r;
                r - first <= (0.04f64).max(first * 0.12)
            }
            None => false,
        };
        if joins {
            groups.last_mut().unwrap().push(k);
        } else {
            groups.push(vec![k]);
        }
    }
    let round = |r: f64| if step > 0.0 { ((r / step).round() * step).max(step).max(lo) } else { r.max(lo) };
    let edges_ro = &*edges;
    let updates: Vec<((u32, u32), f64)> = groups
        .par_iter()
        .flat_map_iter(|g| {
            let w: f64 = g.iter().map(|k| edges_ro[k].n as f64).sum();
            let mean = g.iter().map(|k| edges_ro[k].r * edges_ro[k].n as f64).sum::<f64>() / w.max(1.0);
            let shared = round(mean);
            g.iter()
                .map(|k| {
                    let e = edges_ro[k];
                    let r = match strips.get(k) {
                        Some(pts) => {
                            let step_n = (pts.len() / 300).max(1);
                            let pts: Vec<DVec3> = pts.iter().step_by(step_n).copied().collect();
                            let own = e.err(&pts);
                            let mut t = e;
                            t.r = shared;
                            if t.err(&pts) <= own * 1.5 + 1e-4 {
                                shared
                            } else {
                                round(e.r)
                            }
                        }
                        None => round(e.r),
                    };
                    (*k, r)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    for (k, r) in updates {
        edges.get_mut(&k).unwrap().r = r;
    }
    groups.len()
}

fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn snap(m: &mut Mesh, topo: &Topo, seg: &Segmentation, p: &SnapParams) -> SnapReport {
    let nv = m.pos.len();
    let old = m.pos.clone();
    let inner = inner_vertices(topo, &seg.label, nv);
    let near = three_nearest(m, topo, &inner, p.gap * 1.5);
    let mut rep = SnapReport::default();

    // ---------------------------------------------------- the edges
    let mut edges: HashMap<(u32, u32), Edge> = HashMap::new();
    if p.edges != Edges::Soft {
        let (e, rejected) = find_edges(m, seg, &near, p);
        edges = e;
        rep.rejected = rejected;
        if p.edges == Edges::Fillet && p.round > 0.0 {
            let mut st = strips(&old, &near, p.gap);
            st.retain(|k, _| edges.contains_key(k));
            rep.radius_groups = group_radii(&mut edges, &st, p.round, p.min_radius);
        }
    }
    rep.edges = edges.values().filter(|e| !e.tangent).count();
    rep.tangent = edges.len() - rep.edges;
    let mut radii: Vec<f64> = edges.values().filter(|e| !e.tangent).map(|e| e.r).collect();
    radii.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    rep.radii = radii;

    // ---------------------------------------------------- each vertex
    // the pairs among a vertex's three nearest regions that have an edge,
    // blended by how close each pair is (one pair: that edge alone); the
    // move fades away from the edge, so the inside of the faces stays
    let blend = 0.25;
    let res: Vec<(DVec3, f64, u8, Option<Ridge>)> = (0..nv)
        .into_par_iter()
        .map(|v| {
            let x = old[v];
            let nb = near[v];
            let mut dmin = f64::INFINITY;
            let mut cands: Vec<(f64, Spot, f64, (u32, u32), bool)> = vec![];
            for i in 0..3 {
                for j in i + 1..3 {
                    let ((di, li), (dj, lj)) = (nb[i], nb[j]);
                    if li == NONE || lj == NONE || di + dj > p.gap {
                        continue;
                    }
                    let key = (li.min(lj), li.max(lj));
                    let Some(e) = edges.get(&key) else { continue };
                    // the edge's a is the lower region index
                    let (da, db) = if li < lj { (di, dj) } else { (dj, di) };
                    let Some((sp, w)) = e.place(x, da, db, p.fade) else { continue };
                    let mv = (sp.q - x).length();
                    if mv > p.max_move || w <= 0.0 {
                        continue;
                    }
                    // a move near the limit fades out instead of stopping dead
                    let far = ((p.max_move - mv) / (p.max_move * 0.5)).clamp(0.0, 1.0);
                    dmin = dmin.min(di + dj);
                    cands.push((di + dj, sp, w * far, key, e.thin()));
                }
            }
            let mut sum = DVec3::ZERO;
            let mut wsum = 0.0;
            let mut wmove = 0.0;
            let mut ridge: Option<Ridge> = None;
            let mut top = 0.0;
            for (d, sp, w, key, thin) in cands {
                let k = (-(d - dmin) / blend).exp();
                sum += sp.q * k;
                wmove += w * k;
                wsum += k;
                if k > top {
                    top = k;
                    ridge = thin.then_some(Ridge { key, side: sp.side, at: sp.ridge, d: sp.rd });
                }
            }
            let mut out = if wsum > 0.0 { (sum / wsum, wmove / wsum, 2u8, ridge) } else { (x, 0.0, 0u8, None) };
            // Flatten: the inside of the faces too, a share of the way
            if p.flatten > 0.0 && inner[v] != NONE && out.1 < p.flatten {
                let reg = &seg.regions[inner[v] as usize];
                let q = reg.prim.project(x);
                let far = ((p.max_move - (q - x).length()) / (p.max_move * 0.5)).clamp(0.0, 1.0);
                // a region that fits its shape poorly is only nudged
                let fit = (1.0 - ((reg.rms - 0.06) / 0.1).max(0.0)).clamp(0.3, 1.0);
                let w = p.flatten * far * fit;
                if w > out.1 {
                    out = (q, w, 3, None);
                }
            }
            out
        })
        .collect();
    // a sharp edge (or one too thin for the triangles): where a mesh edge
    // crosses it (one end on each face), the end nearer to it goes onto it,
    // so the edge is drawn by a line of vertices, not a zigzag
    let onto_ridge: Vec<Option<DVec3>> = (0..nv)
        .into_par_iter()
        .map(|v| {
            let rv = res[v].3?;
            if rv.side == 0 || (rv.at - old[v]).length() > p.max_move {
                return None;
            }
            for &u in topo.verts_of(v) {
                let Some(ru) = res[u as usize].3 else { continue };
                if ru.key != rv.key || ru.side == rv.side {
                    continue;
                }
                if ru.side == 0 {
                    // the other end is on the ridge already
                    return None;
                }
                if rv.d < ru.d || (rv.d == ru.d && v < u as usize) {
                    return Some(rv.at);
                }
            }
            None
        })
        .collect();
    // RECTIFY_DEBUG_AT="x,y,z,r": what the vertices around a point do
    if let Ok(s) = std::env::var("RECTIFY_DEBUG_AT") {
        let v: Vec<f64> = s.split(',').filter_map(|t| t.trim().parse().ok()).collect();
        if v.len() == 4 {
            let (c, rad) = (DVec3::new(v[0], v[1], v[2]), v[3]);
            let mut pairs: HashMap<(u32, u32), (usize, f64)> = HashMap::new();
            let mut regs: HashMap<u32, usize> = HashMap::new();
            for (i, x) in old.iter().enumerate() {
                if (*x - c).length() > rad {
                    continue;
                }
                *regs.entry(inner[i]).or_default() += 1;
                let nb = near[i];
                for a in 0..3 {
                    for b in a + 1..3 {
                        let ((da, la), (db, lb)) = (nb[a], nb[b]);
                        if la == NONE || lb == NONE || da + db > p.gap {
                            continue;
                        }
                        let e = pairs.entry((la.min(lb), la.max(lb))).or_default();
                        e.0 += 1;
                        e.1 += (res[i].0 - *x).length();
                    }
                }
            }
            eprintln!("DEBUG at {c} r {rad}: inner {:?}", regs);
            for (k, (n, mv)) in pairs {
                let ra = &seg.regions[k.0 as usize];
                let rb = &seg.regions[k.1 as usize];
                let e = edges.get(&k).map(|e| format!("tangent {} s {} r {:.2}", e.tangent, e.s, e.r)).unwrap_or("no edge".into());
                eprintln!("  pair {:?} ({} rms {:.3} | {} rms {:.3}): {} verts, move {:.3}: {}", k, ra.prim.describe(), ra.rms, rb.prim.describe(), rb.rms, n, mv / n as f64, e);
            }
        }
    }
    let mut target = vec![DVec3::ZERO; nv];
    let mut weight = vec![0.0f64; nv];
    let mut kinds = vec![0u8; nv];
    for (v, (q, w, k, _)) in res.into_iter().enumerate() {
        let (q, w) = match onto_ridge[v] {
            Some(r) => (r, 1.0),
            None => (q, w),
        };
        target[v] = q - old[v];
        weight[v] = w;
        kinds[v] = if k == 2 && w >= 0.999 { 2 } else if w > 0.0 { 3 } else { 0 };
    }
    // neighbours sent far apart (two edges disagreeing, a corner) are eased:
    // the relaxation below joins them smoothly
    let eased: Vec<bool> = (0..nv)
        .into_par_iter()
        .map(|v| {
            if weight[v] < 1.0 {
                return false;
            }
            topo.verts_of(v).iter().any(|&u| {
                let u = u as usize;
                if weight[u] <= 0.0 {
                    return false;
                }
                let before = (old[u] - old[v]).length();
                let after = (old[u] + target[u] * weight[u] - old[v] - target[v]).length();
                after > before * 3.0 + 0.08
            })
        })
        .collect();
    for v in 0..nv {
        if eased[v] {
            weight[v] = 0.4;
            rep.eased += 1;
        }
        match kinds[v] {
            2 => rep.on_edge += 1,
            3 => rep.faded += 1,
            _ => rep.free += 1,
        }
    }
    // the vertices not fully placed take the moves of their neighbours,
    // smoothly (a vertex placed at w keeps that share of its own move)
    let mut delta: Vec<DVec3> = (0..nv).map(|v| target[v] * weight[v]).collect();
    for _ in 0..p.relax {
        let cur = &delta;
        delta = (0..nv)
            .into_par_iter()
            .map(|v| {
                let w = weight[v];
                if w >= 1.0 {
                    return cur[v];
                }
                let ns = topo.verts_of(v);
                if ns.is_empty() {
                    return cur[v];
                }
                let avg = ns.iter().map(|&u| cur[u as usize]).sum::<DVec3>() / ns.len() as f64;
                target[v] * w + avg * (1.0 - w)
            })
            .collect();
    }
    let mut sum = 0.0;
    for v in 0..nv {
        m.pos[v] = old[v] + delta[v];
        let l = delta[v].length();
        sum += l;
        rep.max_move = rep.max_move.max(l);
    }
    rep.mean_move = sum / nv.max(1) as f64;
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(n: DVec3, d: f64) -> Surf {
        Surf { prim: Prim::Plane { n, d }, o: 1.0 }
    }

    #[test]
    fn outer_fillet_between_two_planes() {
        // top (y = 0, matter below) and side (x = 0, matter at x < 0)
        let (a, b) = (plane(DVec3::Y, 0.0), plane(DVec3::X, 0.0));
        let mut e = Edge { a, b, s: -1.0, r: 0.3, tangent: false, n: 0, h: 0.0 };
        let q = e.closest(DVec3::new(-0.1, -0.02, 2.0)).unwrap();
        let c = DVec3::new(-0.3, -0.3, 2.0);
        assert!(((q - c).length() - 0.3).abs() < 1e-9, "{q}");
        // a point well on the top goes to the top
        let q = e.closest(DVec3::new(-1.0, 0.05, 0.0)).unwrap();
        assert!((q - DVec3::new(-1.0, 0.0, 0.0)).length() < 1e-9);
        // the radius comes back from points on a 0.3 fillet
        let pts: Vec<DVec3> = (0..40)
            .map(|i| {
                let t = i as f64 / 39.0 * std::f64::consts::FRAC_PI_2;
                c + DVec3::new(t.sin(), t.cos(), 0.0) * 0.3 + DVec3::Z * (i as f64 * 0.1)
            })
            .collect();
        e.r = 0.0;
        e.fit_radius(&pts, 0.0, 1.5);
        assert!((e.r - 0.3).abs() < 1e-3, "{}", e.r);
    }

    #[test]
    fn inner_edge_plane_cylinder() {
        // a boss (cylinder r 2 along Y, matter inside) standing on a floor
        // (y = 0, matter below): the inner edge is a circle
        let floor = plane(DVec3::Y, 0.0);
        let boss = Surf { prim: Prim::Cylinder { p: DVec3::ZERO, a: DVec3::Y, r: 2.0 }, o: 1.0 };
        let e = Edge { a: floor, b: boss, s: 1.0, r: 0.5, tangent: false, n: 0, h: 0.0 };
        // the rolling ball's centre: 0.5 above the floor, 2.5 from the axis
        let x = DVec3::new(2.2, 0.2, 0.0);
        let q = e.closest(x).unwrap();
        let c = DVec3::new(2.5, 0.5, 0.0);
        assert!(((q - c).length() - 0.5).abs() < 1e-6, "{q}");
    }
}

/// Normals for display: each corner averages the faces around it that turn
/// less than `angle` from its own face (smooth on fillets, sharp on edges).
pub fn crease_normals(m: &Mesh, topo: &Topo, angle: f64) -> Vec<[DVec3; 3]> {
    let cos = angle.to_radians().cos();
    let fnorm = m.normals();
    (0..m.tris.len())
        .into_par_iter()
        .map(|t| {
            let n0 = fnorm[t];
            let mut out = [n0; 3];
            for c in 0..3 {
                let v = m.tris[t][c] as usize;
                let mut s = DVec3::ZERO;
                for &o in topo.tris_of(v) {
                    if fnorm[o as usize].dot(n0) >= cos {
                        s += m.tri_cross(o as usize);
                    }
                }
                out[c] = s.normalize_or(n0);
            }
            out
        })
        .collect()
}
