//! Putting the mesh back on its shapes.
//!
//! - Inside a region, a vertex goes onto the region's shape.
//! - Where two planes meet (directly, or across a strip of free faces, the
//!   melted edge of an AI mesh), the vertices go onto a clean edge: a
//!   fillet of constant radius along the straight line where the planes
//!   meet, the radius fitted on the mesh (or zero: a sharp edge).
//! - Elsewhere near a border the move fades out, and the free vertices
//!   follow their neighbours smoothly: no steps, no tears.

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
    /// Left as they are (only the faces are snapped).
    Soft,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapParams {
    pub edges: Edges,
    /// Widest strip between two planes still taken for one edge (mm).
    pub gap: f64,
    /// Width over which the move fades near a border (mm).
    pub feather: f64,
    /// Smallest angle between two planes that makes an edge (degrees).
    pub edge_angle: f64,
    /// A vertex never moves further than this (mm).
    pub max_move: f64,
    /// Smoothing passes that carry the moves into the free areas.
    pub relax: usize,
}

impl Default for SnapParams {
    fn default() -> Self {
        SnapParams { edges: Edges::Fillet, gap: 2.5, feather: 0.6, edge_angle: 25.0, max_move: 0.8, relax: 120 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SnapReport {
    /// Vertices put exactly on a shape (inside a region).
    pub on_face: usize,
    /// Vertices put on a rebuilt edge.
    pub on_edge: usize,
    /// Pairs of planes rebuilt as an edge, and their radii (mm).
    pub edges: usize,
    pub radii: Vec<f64>,
    pub faded: usize,
    pub free: usize,
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

/// For each vertex, the two nearest regions (geodesic distance along the
/// mesh, from the vertices inside each region), up to `cap` mm.
fn two_nearest(m: &Mesh, topo: &Topo, inner: &[u32], cap: f64) -> Vec<[(f64, u32); 2]> {
    let nv = m.pos.len();
    let mut best = vec![[(f64::INFINITY, NONE); 2]; nv];
    let mut heap = BinaryHeap::new();
    for v in 0..nv {
        if inner[v] != NONE {
            heap.push(Item { d: 0.0, v: v as u32, l: inner[v] });
        }
    }
    while let Some(Item { d, v, l }) = heap.pop() {
        let b = &mut best[v as usize];
        if b[0].1 == l || b[1].1 == l || b[1].0 <= d {
            continue;
        }
        if d < b[0].0 {
            b[1] = b[0];
            b[0] = (d, l);
        } else {
            b[1] = (d, l);
        }
        for &w in topo.verts_of(v as usize) {
            let nd = d + (m.pos[w as usize] - m.pos[v as usize]).length();
            if nd > cap {
                continue;
            }
            let bw = &best[w as usize];
            if bw[0].1 == l || bw[1].1 == l || bw[1].0 <= nd {
                continue;
            }
            heap.push(Item { d: nd, v: w, l });
        }
    }
    best
}

/// Geodesic distance to the nearest vertex for which `src` is true, up to `cap`.
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

/// Two planes meeting along a line, rounded by a fillet of radius `r`.
#[derive(Clone, Copy, Debug)]
pub struct Corner {
    na: DVec3,
    da: f64,
    nb: DVec3,
    db: f64,
    /// Matter inside both half-spaces (an outer edge) or in either (inner).
    convex: bool,
    pub r: f64,
}

impl Corner {
    /// None when the planes are too close to parallel.
    fn new(a: &Prim, b: &Prim, min_angle: f64, samples: &[DVec3]) -> Option<Corner> {
        let (Prim::Plane { n: na, d: da }, Prim::Plane { n: nb, d: db }) = (*a, *b) else { return None };
        if na.dot(nb) > min_angle.to_radians().cos() {
            return None;
        }
        // an outer edge: the points of the strip lie inside both half-spaces
        let mut inside = 0;
        for p in samples {
            if na.dot(*p) - da < 0.0 && nb.dot(*p) - db < 0.0 {
                inside += 1;
            }
        }
        let convex = inside * 2 >= samples.len();
        Some(Corner { na, da, nb, db, convex, r: 0.0 })
    }

    /// The point of the rounded corner nearest to `x`.
    fn closest(&self, x: DVec3) -> DVec3 {
        let (na, nb) = (self.na, self.nb);
        let l = na.cross(nb).normalize();
        // the line's point nearest to x
        let o = {
            // solve na·p = da, nb·p = db, l·p = l·x
            let m = [[na.x, na.y, na.z], [nb.x, nb.y, nb.z], [l.x, l.y, l.z]];
            let b = [self.da, self.db, l.dot(x)];
            match crate::prim::solve3(m, b) {
                Some(s) => DVec3::new(s[0], s[1], s[2]),
                None => return x,
            }
        };
        let s = if self.convex { -1.0 } else { 1.0 };
        // each face goes away from the line inside the other's matter
        // (outer edge) or outside it (inner edge)
        let mut ua = l.cross(na);
        if s * nb.dot(ua) < 0.0 {
            ua = -ua;
        }
        let mut ub = l.cross(nb);
        if s * na.dot(ub) < 0.0 {
            ub = -ub;
        }
        let pa = x - na * (na.dot(x) - self.da);
        let pb = x - nb * (nb.dot(x) - self.db);
        if self.r <= 1e-9 {
            // sharp: the nearest of the two faces (within their half-planes)
            let a_ok = (pa - o).dot(ua) >= 0.0;
            let b_ok = (pb - o).dot(ub) >= 0.0;
            return match (a_ok, b_ok) {
                (true, true) => {
                    if (pa - x).length_squared() < (pb - x).length_squared() {
                        pa
                    } else {
                        pb
                    }
                }
                (true, false) => pa,
                (false, true) => pb,
                _ => o + l * l.dot(x - o),
            };
        }
        // the fillet's centre: r from both planes, on the matter side
        // (outer) or the empty side (inner)
        let r = self.r;
        let mm = [[na.x, na.y, na.z], [nb.x, nb.y, nb.z], [l.x, l.y, l.z]];
        let c = match crate::prim::solve3(mm, [self.da + s * r, self.db + s * r, l.dot(x)]) {
            Some(q) => DVec3::new(q[0], q[1], q[2]),
            None => return x,
        };
        let ta = c - na * (s * r);
        let tb = c - nb * (s * r);
        if (pa - ta).dot(ua) >= 0.0 && (pa - x).length_squared() <= (pb - x).length_squared() + 1e-12 {
            return pa;
        }
        if (pb - tb).dot(ub) >= 0.0 {
            return pb;
        }
        if (pa - ta).dot(ua) >= 0.0 {
            return pa;
        }
        let v = x - c;
        let v = v - l * l.dot(v);
        let len = v.length();
        if len < 1e-12 {
            return x;
        }
        c + l * l.dot(x - c) + v * (r / len)
    }

    fn err(&self, pts: &[DVec3]) -> f64 {
        pts.iter().map(|p| (self.closest(*p) - *p).length_squared()).sum::<f64>() / pts.len().max(1) as f64
    }

    /// The radius that fits the points best (golden section on [0, hi]).
    fn fit_radius(&mut self, pts: &[DVec3], hi: f64) {
        let g = 0.618_034;
        let (mut a, mut b) = (0.0, hi);
        for _ in 0..40 {
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

pub fn snap(m: &mut Mesh, topo: &Topo, seg: &Segmentation, p: &SnapParams) -> SnapReport {
    let nv = m.pos.len();
    let old = m.pos.clone();
    // each vertex: the region all its faces share (inner), or none
    let inner: Vec<u32> = (0..nv)
        .into_par_iter()
        .map(|v| {
            let ts = topo.tris_of(v);
            let l = ts.first().map(|&t| seg.label[t as usize]).unwrap_or(NONE);
            if l != NONE && ts.iter().all(|&t| seg.label[t as usize] == l) {
                l
            } else {
                NONE
            }
        })
        .collect();
    let near = two_nearest(m, topo, &inner, p.gap.max(p.feather) * 1.5);
    // how far from the free faces and from other regions' faces
    let border: Vec<bool> = (0..nv).map(|v| inner[v] == NONE).collect();
    let to_border = distance_to(m, topo, &border, p.feather * 1.5);

    // ---------------------------------------------------- edges between planes
    let mut corners: HashMap<(u32, u32), Corner> = HashMap::new();
    if p.edges != Edges::Soft {
        let mut strips: HashMap<(u32, u32), Vec<DVec3>> = HashMap::new();
        for v in 0..nv {
            let [(d1, l1), (d2, l2)] = near[v];
            if l1 == NONE || l2 == NONE || d1 + d2 > p.gap {
                continue;
            }
            strips.entry((l1.min(l2), l1.max(l2))).or_default().push(old[v]);
        }
        let list: Vec<((u32, u32), Vec<DVec3>)> = strips.into_iter().filter(|(_, pts)| pts.len() >= 8).collect();
        let found: Vec<Option<((u32, u32), Corner)>> = list
            .par_iter()
            .map(|(k, pts)| {
                let (a, b) = (&seg.regions[k.0 as usize].prim, &seg.regions[k.1 as usize].prim);
                let mut c = Corner::new(a, b, p.edge_angle, pts)?;
                if p.edges == Edges::Fillet {
                    c.fit_radius(pts, p.gap);
                }
                // the strip must look like this edge
                let rms = c.err(pts).sqrt();
                (rms < p.max_move * 0.35).then_some((*k, c))
            })
            .collect();
        corners = found.into_iter().flatten().collect();
    }

    // ---------------------------------------------------- each vertex
    let res: Vec<(DVec3, f64, u8)> = (0..nv)
        .into_par_iter()
        .map(|v| {
            let x = old[v];
            let [(d1, l1), (d2, l2)] = near[v];
            // on a rebuilt edge
            if l1 != NONE && l2 != NONE && d1 + d2 <= p.gap {
                if let Some(c) = corners.get(&(l1.min(l2), l1.max(l2))) {
                    let q = c.closest(x);
                    if (q - x).length() <= p.max_move {
                        return (q, 1.0, 2);
                    }
                }
            }
            // inside a region: on its shape, fading near its border
            if inner[v] != NONE {
                let reg = &seg.regions[inner[v] as usize];
                let q = reg.prim.project(x);
                // a move near the limit fades out instead of stopping dead
                let far = ((p.max_move - (q - x).length()) / (p.max_move * 0.5)).clamp(0.0, 1.0);
                if far <= 0.0 {
                    return (x, 0.0, 0);
                }
                let t = (to_border[v] / p.feather.max(1e-6)).clamp(0.0, 1.0) * far;
                // a region that fits its shape poorly is only nudged
                let fit = (1.0 - ((reg.rms - 0.06) / 0.1).max(0.0)).clamp(0.3, 1.0);
                let w = t * t * (3.0 - 2.0 * t) * fit;
                return (q, w, if w >= 1.0 { 1 } else { 3 });
            }
            (x, 0.0, 0)
        })
        .collect();
    let mut rep = SnapReport { edges: corners.len(), ..Default::default() };
    let mut radii: Vec<f64> = corners.values().map(|c| c.r).collect();
    radii.sort_by(|a, b| a.partial_cmp(b).unwrap());
    rep.radii = radii;
    let mut delta = vec![DVec3::ZERO; nv];
    let mut target = vec![DVec3::ZERO; nv];
    let mut weight = vec![0.0f64; nv];
    for (v, (q, w, k)) in res.iter().enumerate() {
        target[v] = *q - old[v];
        delta[v] = target[v] * *w;
        weight[v] = *w;
        match k {
            1 => rep.on_face += 1,
            2 => rep.on_edge += 1,
            3 => rep.faded += 1,
            _ => rep.free += 1,
        }
    }
    // the vertices not fully placed take the moves of their neighbours,
    // smoothly (a vertex placed at w keeps that share of its own move)
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
