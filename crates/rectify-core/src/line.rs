//! The Line tool (Romain, 18 nov. 2026: "un tool qui trace un segment que
//! l'user tire et hop ça fix le bord"): a segment dragged along an edge of the
//! mesh; there the edge becomes straight and sharp.
//!
//! Around the segment, a band of the mesh. Its vertices are told apart by
//! their normals (two groups); each side is the part of its group that
//! touches the segment (a parallel face further away is not taken), fitted
//! with a plane, and the edge is where the planes meet. A chain of the mesh's
//! own edges is laid along it (the shortest path that stays close) and put on
//! it, straight; from the chain outwards, the vertices that lie on a side
//! (close to its plane, its normal) go onto that plane, never past the edge.
//! The move fades out at the band's border and past the segment's ends.

use crate::mesh::{fit_plane, Mesh, Topo};
use crate::moves::Moves;
use glam::DVec3;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineParams {
    /// How far from the edge the sides are fixed (mm).
    pub width: f64,
    /// Below this angle between the two sides there is no edge (degrees).
    pub min_angle: f64,
    /// The part of the band put fully on the planes (0..1); the rest fades.
    pub hold: f64,
}

impl Default for LineParams {
    fn default() -> Self {
        LineParams { width: 1.5, min_angle: 12.0, hold: 0.6 }
    }
}

#[derive(Clone, Debug)]
pub struct LineFit {
    /// The edge as fixed, under the segment's two ends.
    pub a: DVec3,
    pub b: DVec3,
    /// The angle between the two sides' normals (degrees: 90 for a box).
    pub angle: f64,
    /// How far from the edge it was fixed (mm): the width asked, or more when
    /// the edge is rounded wider than that.
    pub width: f64,
    /// How far the sides' points lie from their planes (mm, RMS).
    pub rms: f64,
    pub moves: Moves,
    /// The vertices put on the edge, in order.
    pub edge: Vec<u32>,
}

fn smooth(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The vertices reached from `seeds` through vertices that pass `keep`.
fn flood(topo: &Topo, m: &Mesh, seeds: &[u32], keep: impl Fn(DVec3) -> bool) -> Vec<u32> {
    let mut seen: HashMap<u32, ()> = HashMap::new();
    let mut out = vec![];
    let mut q = VecDeque::new();
    for &s in seeds {
        if keep(m.pos[s as usize]) && seen.insert(s, ()).is_none() {
            q.push_back(s);
        }
    }
    while let Some(v) = q.pop_front() {
        out.push(v);
        for &w in topo.verts_of(v as usize) {
            if !seen.contains_key(&w) && keep(m.pos[w as usize]) {
                seen.insert(w, ());
                q.push_back(w);
            }
        }
    }
    out
}

/// The line where two planes meet (a point, the unit direction), its point
/// nearest to `near`.
pub(crate) fn meet(p1: (DVec3, DVec3), p2: (DVec3, DVec3), near: DVec3) -> Option<(DVec3, DVec3)> {
    let (c1, n1) = p1;
    let (c2, n2) = p2;
    let e = n1.cross(n2);
    if e.length() < 1e-6 {
        return None;
    }
    let e = e.normalize();
    let (a11, a12, a22) = (n1.dot(n1), n1.dot(n2), n2.dot(n2));
    let (b1, b2) = (n1.dot(c1 - near), n2.dot(c2 - near));
    let det = a11 * a22 - a12 * a12;
    if det.abs() < 1e-12 {
        return None;
    }
    let al = (b1 * a22 - b2 * a12) / det;
    let be = (a11 * b2 - a12 * b1) / det;
    Some((near + n1 * al + n2 * be, e))
}

/// In a side's plane, the direction away from the edge (toward its points).
fn side_dir(pl: (DVec3, DVec3), (p0, e): (DVec3, DVec3)) -> DVec3 {
    let u = e.cross(pl.1).normalize_or_zero();
    if (pl.0 - p0).dot(u) < 0.0 {
        -u
    } else {
        u
    }
}

fn debug() -> bool {
    std::env::var("RECTIFY_DEBUG").is_ok()
}

/// The densest direction of a set of unit normals (mean shift with a
/// narrow window, from `from`): a flat face's normal, whatever is round
/// around it.
fn mode(ns: &[(DVec3, f64)], from: DVec3, window_deg: f64) -> DVec3 {
    let cos = window_deg.to_radians().cos();
    let mut m = from.normalize_or_zero();
    for _ in 0..20 {
        let s: DVec3 = ns.iter().filter(|(n, _)| n.dot(m) >= cos).map(|(n, w)| *n * *w).sum();
        let next = s.normalize_or_zero();
        if next == DVec3::ZERO || next.dot(m) > 1.0 - 1e-10 {
            break;
        }
        m = next;
    }
    m
}

/// A face's plane from its flat vertices: normal `n` refined on them, offset
/// their median.
fn flat_plane(pts: &[(DVec3, DVec3, f64)], n0: DVec3, window_deg: f64) -> Option<(DVec3, DVec3)> {
    let cos = window_deg.to_radians().cos();
    let flat: Vec<&(DVec3, DVec3, f64)> = pts.iter().filter(|(_, n, _)| n.dot(n0) >= cos).collect();
    if flat.len() < 6 {
        return None;
    }
    let mut offs: Vec<f64> = flat.iter().map(|(x, _, _)| n0.dot(*x)).collect();
    offs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let d0 = offs[offs.len() / 2];
    // a plane through those close to it (a parallel face nearby left out)
    let spread = (offs[offs.len() * 3 / 4] - offs[offs.len() / 4]).abs().max(1e-3);
    let near: Vec<(DVec3, f64)> = flat.iter().filter(|(x, _, _)| (n0.dot(*x) - d0).abs() <= spread * 2.0).map(|(x, _, w)| (*x, *w)).collect();
    let (c, n) = fit_plane(&near)?;
    let n = if n.dot(n0) < 0.0 { -n } else { n };
    // never far from the normals' own direction
    if n.dot(n0) < 8f64.to_radians().cos() {
        return Some((n0 * d0, n0));
    }
    Some((c, n))
}

/// Fixes the edge under the segment `pa`–`pb` (points on the mesh, `seeds`
/// the vertices of the triangles they lie on; `h` the mesh's edge length).
pub fn fix(m: &Mesh, topo: &Topo, seeds: &[u32], pa: DVec3, pb: DVec3, h: f64, p: &LineParams) -> Result<LineFit, String> {
    let len = (pb - pa).length();
    if len < h.max(0.05) * 2.0 {
        return Err("Drag a longer segment along the edge".into());
    }
    let d = (pb - pa) / len;
    let w0 = p.width.max(h * 3.0);
    // the faces are looked for this far (a rounded edge is wider than it looks)
    let reach = w0 * 2.5;
    let mid = (pa + pb) * 0.5;
    let seg_dist = |x: DVec3| {
        let t = (x - pa).dot(d).clamp(0.0, len);
        (x - (pa + d * t)).length()
    };
    let normal_of = |v: u32| -> (DVec3, f64) {
        let s: DVec3 = topo.tris_of(v as usize).iter().map(|&t| m.tri_cross(t as usize)).sum();
        let a: f64 = topo.tris_of(v as usize).iter().map(|&t| m.tri_cross(t as usize).length()).sum::<f64>() / 6.0;
        (s.normalize_or_zero(), a)
    };

    // 1. the band around the segment as drawn (the middle of it: its ends
    // may stop on another face)
    let (s0, s1) = (len * 0.1, len * 0.9);
    let band0 = flood(topo, m, seeds, |x| {
        let t = (x - pa).dot(d);
        let perp = ((x - pa) - d * t).length();
        t >= -w0 && t <= len + w0 && perp <= reach
    });
    if band0.len() < 12 {
        return Err("Too few vertices there: drag over the mesh, along an edge".into());
    }
    let idx0: HashMap<u32, usize> = band0.iter().enumerate().map(|(i, &v)| (v, i)).collect();
    let nrm: Vec<(DVec3, f64)> = band0.iter().map(|&v| normal_of(v)).collect();
    let mid_part: Vec<bool> = band0.iter().map(|&v| {
        let t = (m.pos[v as usize] - pa).dot(d);
        t >= s0 && t <= s1
    }).collect();

    // 2. two groups by their normals (across the segment): 2-means
    let np: Vec<DVec3> = nrm.iter().map(|(n, _)| (*n - d * n.dot(d)).normalize_or_zero()).collect();
    let mean = np.iter().zip(&nrm).map(|(n, (_, a))| *n * *a).sum::<DVec3>().normalize_or_zero();
    let far = |from: DVec3| -> DVec3 { np.iter().copied().filter(|n| *n != DVec3::ZERO).min_by(|a, b| a.dot(from).partial_cmp(&b.dot(from)).unwrap()).unwrap_or(from) };
    let mut c1 = far(mean);
    let mut c2 = far(c1);
    for _ in 0..12 {
        let (mut s1, mut s2) = (DVec3::ZERO, DVec3::ZERO);
        for (i, n) in np.iter().enumerate() {
            if n.dot(c1) >= n.dot(c2) {
                s1 += *n * nrm[i].1;
            } else {
                s2 += *n * nrm[i].1;
            }
        }
        let (n1, n2) = (s1.normalize_or_zero(), s2.normalize_or_zero());
        if n1 == DVec3::ZERO || n2 == DVec3::ZERO {
            break;
        }
        c1 = n1;
        c2 = n2;
    }
    let turn = c1.dot(c2).clamp(-1.0, 1.0).acos().to_degrees();
    if turn < p.min_angle {
        return Err(format!("No edge there: the surface turns {turn:.0}° only"));
    }
    let class: Vec<u8> = np.iter().map(|n| if n.dot(c1) >= n.dot(c2) { 1 } else { 2 }).collect();
    // each side: its group's part that touches the segment
    let near_seg = (w0 * 0.4).max(h * 3.0);
    let side_part = |k: u8| -> Vec<usize> {
        let mut inside = vec![false; band0.len()];
        let mut q: VecDeque<usize> = (0..band0.len()).filter(|&i| class[i] == k && seg_dist(m.pos[band0[i] as usize]) <= near_seg).collect();
        for &i in &q {
            inside[i] = true;
        }
        let mut out = vec![];
        while let Some(i) = q.pop_front() {
            out.push(i);
            for &w in topo.verts_of(band0[i] as usize) {
                if let Some(&j) = idx0.get(&w) {
                    if !inside[j] && class[j] == k {
                        inside[j] = true;
                        q.push_back(j);
                    }
                }
            }
        }
        out
    };
    let part1 = side_part(1);
    let part2 = side_part(2);
    // each side's face: the densest normal of its part (away from the
    // segment first), its flat vertices, their plane
    let face = |part: &[usize], c: DVec3| -> Option<(DVec3, DVec3)> {
        let ns: Vec<(DVec3, f64)> = part.iter().filter(|&&i| mid_part[i]).map(|&i| nrm[i]).collect();
        let far_n: DVec3 = part.iter().filter(|&&i| mid_part[i] && seg_dist(m.pos[band0[i] as usize]) > reach * 0.5).map(|&i| nrm[i].0 * nrm[i].1).sum();
        let start = if far_n.length() > 1e-12 { far_n.normalize() } else { c };
        let n0 = mode(&ns, start, 10.0);
        let pts: Vec<(DVec3, DVec3, f64)> = part.iter().filter(|&&i| mid_part[i]).map(|&i| (m.pos[band0[i] as usize], nrm[i].0, nrm[i].1)).collect();
        flat_plane(&pts, n0, 10.0)
    };
    let (Some(pl1), Some(pl2)) = (face(&part1, c1), face(&part2, c2)) else {
        return Err("No flat face on one side: make the width larger".into());
    };
    if debug() {
        eprintln!("line: band {} · groups {:.0}° · sides {} + {} vertices", band0.len(), turn, part1.len(), part2.len());
    }

    // 3. the edge where the faces meet
    let (p0, mut e) = meet(pl1, pl2, mid).ok_or("The two sides are parallel: no edge there")?;
    if e.dot(d) < 0.0 {
        e = -e;
    }
    let turn = pl1.1.dot(pl2.1).clamp(-1.0, 1.0).acos().to_degrees();
    if turn < p.min_angle {
        return Err(format!("No edge there: the faces turn {turn:.0}° only"));
    }
    let dist_line = |x: DVec3| ((x - p0) - e * (x - p0).dot(e)).length();
    // how wide the rounded edge is: across it, how far from the edge the
    // surface comes back onto each face (the median gap per step of one
    // edge length, so that a bump further along does not count)
    let extent = {
        let u = [side_dir(pl1, (p0, e)), side_dir(pl2, (p0, e))];
        let split = u[0] - u[1];
        let bins = ((reach / h).ceil() as usize).clamp(4, 400);
        let mut gaps: Vec<Vec<Vec<f64>>> = vec![vec![vec![]; bins]; 2];
        for (i, &v) in band0.iter().enumerate() {
            if !mid_part[i] {
                continue;
            }
            let x = m.pos[v as usize];
            let k = if (x - p0).dot(split) >= 0.0 { 0 } else { 1 };
            let (c, n) = if k == 0 { pl1 } else { pl2 };
            let rho = dist_line(x);
            let b = (rho / h) as usize;
            if b < bins {
                gaps[k][b].push(n.dot(x - c).abs());
            }
        }
        let flat = (h * 0.3).max(0.03);
        let mut ext: f64 = 0.0;
        for g in &mut gaps {
            let med: Vec<Option<f64>> = g
                .iter_mut()
                .map(|v| {
                    if v.len() < 3 {
                        return None;
                    }
                    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    Some(v[v.len() / 2])
                })
                .collect();
            // the first two steps in a row close to the face
            let mut at = None;
            for b in 0..bins.saturating_sub(1) {
                if let (Some(a), Some(c)) = (med[b], med[b + 1]) {
                    if a < flat && c < flat {
                        at = Some(b);
                        break;
                    }
                }
            }
            ext = ext.max(at.map(|b| b as f64 * h).unwrap_or(reach * 0.6));
        }
        ext
    };
    let r = (extent * 1.25 + h * 2.0).clamp(w0, reach);
    let fade = (r * 0.6).min(len * 0.3);
    if debug() {
        eprintln!("line: faces {:.1}° · edge {:.3} / {:.3} mm from the ends · round {:.2} mm → width {:.2}", turn, dist_line(pa), dist_line(pb), extent, r);
    }
    if dist_line(pa) > r * 1.5 || dist_line(pb) > r * 1.5 {
        return Err("The edge found is far from the segment: drag closer along it".into());
    }
    let (ta, tb) = ((pa - p0).dot(e), (pb - p0).dot(e));
    let sides = [(pl1, side_dir(pl1, (p0, e))), (pl2, side_dir(pl2, (p0, e)))];

    // 4. the band around the edge found
    let (t0, t1) = (ta - fade, tb + fade);
    let band = flood(topo, m, &band0, |x| {
        let t = (x - p0).dot(e);
        t >= t0 && t <= t1 && dist_line(x) <= r
    });
    let index: HashMap<u32, usize> = band.iter().enumerate().map(|(i, &v)| (v, i)).collect();
    let pos = |i: usize| m.pos[band[i] as usize];

    // 5. a chain of the mesh's edges along it, from end to end
    let nearest = |at: DVec3| -> Option<usize> { (0..band.len()).min_by(|&i, &j| (pos(i) - at).length_squared().partial_cmp(&(pos(j) - at).length_squared()).unwrap()) };
    let (Some(start), Some(goal)) = (nearest(p0 + e * t0), nearest(p0 + e * t1)) else {
        return Err("No mesh along the edge".into());
    };
    let mut cost = vec![f64::INFINITY; band.len()];
    let mut prev = vec![usize::MAX; band.len()];
    let mut heap = BinaryHeap::new();
    cost[start] = 0.0;
    heap.push((Reverse((0.0f64).to_bits()), start));
    while let Some((Reverse(cb), i)) = heap.pop() {
        let c = f64::from_bits(cb);
        if c > cost[i] {
            continue;
        }
        if i == goal {
            break;
        }
        let xi = pos(i);
        for &w in topo.verts_of(band[i] as usize) {
            let Some(&j) = index.get(&w) else { continue };
            let xj = pos(j);
            let l = (xj - xi).length();
            let dm = dist_line((xi + xj) * 0.5) / h.max(1e-6);
            let back = (-(xj - xi).dot(e)).max(0.0);
            let c2 = c + l * (1.0 + 2.0 * dm * dm) + back * 4.0;
            if c2 < cost[j] {
                cost[j] = c2;
                prev[j] = i;
                heap.push((Reverse(c2.to_bits()), j));
            }
        }
    }
    if !cost[goal].is_finite() {
        return Err("The mesh is broken along the edge".into());
    }
    let mut chain = vec![goal];
    while *chain.last().unwrap() != start {
        chain.push(prev[*chain.last().unwrap()]);
    }
    chain.reverse();
    let mut on_edge = vec![false; band.len()];
    for &i in &chain {
        on_edge[i] = true;
    }

    // 6. the sides, from the chain outwards: next to each of its edges, the
    // two triangles' third corners (the one more on side 1 is side 1); then
    // through the vertices that lie on their side: on the rounded edge, or
    // near the face (close to its plane, its normal); how much they belong
    // fades out
    let crease = (extent * 1.15).max(h * 2.0);
    let tol = (w0 * 0.2).max(h * 3.0);
    let belong = |i: usize, s: usize| -> f64 {
        let x = pos(i);
        if dist_line(x) < crease {
            return 1.0;
        }
        let ((c, n), _) = sides[s];
        let (nv, _) = normal_of(band[i]);
        let off = smooth((tol * 2.0 - n.dot(x - c).abs()) / tol);
        let ang = smooth((nv.dot(n) - 50f64.to_radians().cos()) / (30f64.to_radians().cos() - 50f64.to_radians().cos()));
        off * ang
    };
    let mut side = vec![u8::MAX; band.len()];
    let mut part = vec![0.0f64; band.len()];
    let mut q = VecDeque::new();
    let split = sides[0].1 - sides[1].1;
    for w in chain.windows(2) {
        let (u, v) = (band[w[0]], band[w[1]]);
        let thirds: Vec<usize> = topo
            .tris_of(u as usize)
            .iter()
            .filter(|&&t| m.tris[t as usize].contains(&v))
            .filter_map(|&t| m.tris[t as usize].iter().find(|&&x| x != u && x != v).and_then(|x| index.get(x).copied()))
            .collect();
        if let [a, b] = thirds[..] {
            let (s1, s2) = if (pos(a) - p0).dot(split) >= (pos(b) - p0).dot(split) { (a, b) } else { (b, a) };
            for (i, s) in [(s1, 0u8), (s2, 1u8)] {
                if !on_edge[i] && side[i] == u8::MAX {
                    let b = belong(i, s as usize);
                    if b > 0.0 {
                        side[i] = s;
                        part[i] = b;
                        q.push_back(i);
                    }
                }
            }
        }
    }
    while let Some(i) = q.pop_front() {
        let s = side[i];
        for &w in topo.verts_of(band[i] as usize) {
            let Some(&j) = index.get(&w) else { continue };
            if !on_edge[j] && side[j] == u8::MAX {
                // belonging never grows away from the edge
                let b = belong(j, s as usize).min(part[i]);
                if b > 0.0 {
                    side[j] = s;
                    part[j] = b;
                    q.push_back(j);
                }
            }
        }
    }

    // how much each vertex belongs, smoothed over its neighbours (no ragged
    // border where the moved part ends)
    for _ in 0..6 {
        let next: Vec<f64> = (0..band.len())
            .map(|i| {
                if side[i] == u8::MAX || on_edge[i] {
                    return part[i];
                }
                let (mut s, mut n) = (0.0, 0.0);
                for &w in topo.verts_of(band[i] as usize) {
                    n += 1.0;
                    if let Some(&j) = index.get(&w) {
                        s += if on_edge[j] { 1.0 } else if side[j] == side[i] { part[j] } else { part[i] };
                    }
                }
                if n == 0.0 {
                    part[i]
                } else {
                    0.5 * part[i] + 0.5 * s / n
                }
            })
            .collect();
        part = next;
    }

    // 7. where each vertex goes
    let along = |t: f64| -> f64 {
        if fade <= 1e-9 {
            return 1.0;
        }
        if t < ta {
            smooth((t - t0) / fade)
        } else if t > tb {
            smooth((t1 - t) / fade)
        } else {
            1.0
        }
    };
    // the whole rounded edge is moved fully, then it fades to the border
    let full = (r * p.hold.clamp(0.0, 0.95)).max(crease).min(r * 0.95);
    let across = |rho: f64| -> f64 {
        if rho <= full {
            1.0
        } else {
            smooth((r - rho) / (r - full))
        }
    };
    let mut targets: Vec<(u32, DVec3)> = Vec::with_capacity(band.len());
    // the chain, straight on the edge and in order along it
    let mut last = f64::NEG_INFINITY;
    for &i in &chain {
        let x = pos(i);
        let t = (x - p0).dot(e).max(last).clamp(t0, t1);
        last = t;
        let y = p0 + e * t;
        targets.push((band[i], x + (y - x) * along(t)));
    }
    let mut sq = 0.0;
    let mut n_sq = 0usize;
    for i in 0..band.len() {
        if on_edge[i] || side[i] == u8::MAX {
            continue;
        }
        let x = pos(i);
        let ((c, n), u) = sides[side[i] as usize];
        let off = n.dot(x - c);
        let mut y = x - n * off;
        // never past the edge
        let past = (y - p0).dot(u);
        if past < 0.0 {
            y -= u * past;
        }
        let rho = dist_line(x);
        if rho >= r * 0.3 {
            sq += off * off;
            n_sq += 1;
        }
        let w = along((x - p0).dot(e)) * across(rho) * part[i];
        targets.push((band[i], x + (y - x) * w));
    }
    if debug() {
        let n1 = side.iter().filter(|s| **s == 0).count();
        let n2 = side.iter().filter(|s| **s == 1).count();
        eprintln!("line: band {} · chain {} · sides {} + {} · left alone {}", band.len(), chain.len(), n1, n2, band.len() - chain.len() - n1 - n2);
    }
    let moves = Moves::new(m, targets);
    let edge = chain.iter().map(|&i| band[i]).collect();
    Ok(LineFit { a: p0 + e * ta, b: p0 + e * tb, angle: turn, width: r, rms: (sq / n_sq.max(1) as f64).sqrt(), moves, edge })
}
