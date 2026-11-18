//! The Patch tool (Romain, 18 nov. 2026, 18 h 51: the gun's body drawn with
//! Lines; 18 h 56: "développe les 3"): a loop of paths filled with one NURBS
//! surface laid on the scan.
//!
//! The paths picked are cut where another one's end lands on them, the
//! pieces left hanging are dropped, and what is left must close one loop.
//! The loop is cut into 4 sides: at the points where its paths meet (the
//! sharpest kept when there are more than 4), else at its sharpest turns,
//! else in the middle of its longest side. The sides make a Coons grid, its
//! inner points are put on the scan (straight across, along the grid's
//! normal, so that it stays even), and a bicubic surface is fitted to them
//! by least squares, its
//! border held on the sides; points far off weigh less (Cauchy). The fewest
//! spans that follow the scan almost as well as the most are kept.

use crate::mesh::{Mesh, Surface as Closest};
use crate::nurbs::{basis, find_span, solve, uniform_knots, Curve, Surface};
use glam::{DVec2, DVec3};

/// The patch's settings, changed live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PatchParams {
    /// Spans along the longer sides (the other way in proportion); 0: the
    /// fewest that follow the scan.
    pub spans: usize,
    /// The inside laid on the scan (else stretched between the sides only).
    pub follow: bool,
}

impl Default for PatchParams {
    fn default() -> Self {
        PatchParams { spans: 0, follow: true }
    }
}

/// A patch made.
#[derive(Clone, Debug)]
pub struct Patched {
    pub surface: Surface,
    /// Spans in u and in v.
    pub spans: [usize; 2],
    /// How far the scan is from the surface's inside (mm, RMS), and the
    /// largest gap between its border and the sides.
    pub off: f64,
    pub gap: f64,
    /// The share of the inside passed over: far from a calm surface (a
    /// button, a hole inside the loop), left out.
    pub passed: f64,
    /// The loop's four corners, in order.
    pub corners: [DVec3; 4],
}

/// A curve as a polyline, its corners kept (every knot span sampled).
pub fn polyline(c: &Curve) -> Vec<DVec3> {
    let mut ks: Vec<f64> = c.knots[c.degree..=c.pts.len()].to_vec();
    ks.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
    let spans = ks.len().saturating_sub(1).max(1);
    let per = (192 / spans).clamp(4, 24);
    let mut out = vec![c.eval(ks[0])];
    for w in ks.windows(2) {
        for k in 1..=per {
            let p = c.eval(w[0] + (w[1] - w[0]) * k as f64 / per as f64);
            if (p - *out.last().unwrap()).length() > 1e-9 {
                out.push(p);
            }
        }
    }
    out
}

fn arc(p: &[DVec3]) -> Vec<f64> {
    let mut s = vec![0.0];
    for w in p.windows(2) {
        s.push(s.last().unwrap() + (w[1] - w[0]).length());
    }
    s
}

/// The point at `t` (0..1) of the polyline's length.
fn at(p: &[DVec3], s: &[f64], t: f64) -> DVec3 {
    let l = *s.last().unwrap();
    if l <= 0.0 {
        return p[0];
    }
    let x = t.clamp(0.0, 1.0) * l;
    let i = s.partition_point(|&v| v < x).clamp(1, p.len() - 1);
    let f = ((x - s[i - 1]) / (s[i] - s[i - 1]).max(1e-12)).clamp(0.0, 1.0);
    p[i - 1].lerp(p[i], f)
}

/// The nearest point of a polyline to `x`: its distance, the segment and
/// where on it (0..1).
fn nearest_on(p: &[DVec3], x: DVec3) -> (f64, usize, f64) {
    let mut best = (f64::INFINITY, 0, 0.0);
    for i in 0..p.len().saturating_sub(1) {
        let (a, b) = (p[i], p[i + 1]);
        let ab = b - a;
        let t = ((x - a).dot(ab) / ab.length_squared().max(1e-24)).clamp(0.0, 1.0);
        let d = (a + ab * t - x).length();
        if d < best.0 {
            best = (d, i, t);
        }
    }
    best
}

/// One piece of a path between two meeting points.
struct Piece {
    pts: Vec<DVec3>,
    /// The path it comes from.
    from: usize,
    ends: [usize; 2],
}

/// The paths cut where another one's end lands inside them (within
/// `tol`), into pieces between meeting points (ends closer than `tol` are
/// one point); also how many meeting points.
fn graph(lines: &[Vec<DVec3>], tol: f64) -> (Vec<Piece>, usize) {
    let mut pieces: Vec<(Vec<DVec3>, usize)> = vec![];
    for (i, li) in lines.iter().enumerate() {
        if li.len() < 2 {
            continue;
        }
        let s = arc(li);
        let len = *s.last().unwrap();
        let mut cuts: Vec<(f64, DVec3)> = vec![];
        for (j, lj) in lines.iter().enumerate() {
            if j == i || lj.len() < 2 {
                continue;
            }
            for e in [lj[0], *lj.last().unwrap()] {
                let (d, k, t) = nearest_on(li, e);
                let x = s[k] + (s[k + 1] - s[k]) * t;
                if d <= tol && x > tol && x < len - tol && cuts.iter().all(|c| (c.0 - x).abs() > tol) {
                    cuts.push((x, e));
                }
            }
        }
        cuts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut cur: Vec<DVec3> = vec![li[0]];
        let mut c = 0;
        for k in 1..li.len() {
            while c < cuts.len() && cuts[c].0 <= s[k] {
                cur.push(cuts[c].1);
                pieces.push((std::mem::take(&mut cur), i));
                cur.push(cuts[c].1);
                c += 1;
            }
            if (li[k] - *cur.last().unwrap()).length() > 1e-9 {
                cur.push(li[k]);
            }
        }
        if cur.len() >= 2 {
            pieces.push((cur, i));
        }
    }
    // the meeting points: piece ends closer than `tol` are one
    let mut nodes: Vec<(DVec3, usize)> = vec![];
    let mut node_of = |x: DVec3| -> usize {
        if let Some(k) = nodes.iter().position(|(c, n)| (*c / *n as f64 - x).length() <= tol) {
            nodes[k].0 += x;
            nodes[k].1 += 1;
            return k;
        }
        nodes.push((x, 1));
        nodes.len() - 1
    };
    let mut ps: Vec<Piece> = pieces
        .into_iter()
        .map(|(pts, from)| {
            let ends = [node_of(pts[0]), node_of(*pts.last().unwrap())];
            Piece { pts, from, ends }
        })
        .collect();
    let centre: Vec<DVec3> = nodes.iter().map(|(c, n)| *c / *n as f64).collect();
    for p in &mut ps {
        p.pts[0] = centre[p.ends[0]];
        let n = p.pts.len();
        p.pts[n - 1] = centre[p.ends[1]];
    }
    // tiny pieces between two meeting points are dropped, their ends one
    ps.retain(|p| !(p.ends[0] == p.ends[1] && arc(&p.pts).last().unwrap() < &(tol * 4.0)));
    (ps, centre.len())
}

/// The pieces left once what hangs is dropped (again and again), and how
/// many pieces meet at each point then.
fn prune(ps: &[Piece], nodes: usize) -> (Vec<bool>, Vec<usize>) {
    let mut alive = vec![true; ps.len()];
    loop {
        let mut deg = vec![0usize; nodes];
        for (k, p) in ps.iter().enumerate() {
            if alive[k] {
                deg[p.ends[0]] += 1;
                deg[p.ends[1]] += 1;
            }
        }
        let mut changed = false;
        for (k, p) in ps.iter().enumerate() {
            if alive[k] && (deg[p.ends[0]] < 2 || deg[p.ends[1]] < 2) {
                alive[k] = false;
                changed = true;
            }
        }
        if !changed {
            return (alive, deg);
        }
    }
}

/// The loop the polylines close: its points (the first not repeated at the
/// end) and where two paths meet on it. Paths are cut where another one's
/// end lands on them (within `tol`); pieces left hanging are dropped.
pub fn chain(lines: &[Vec<DVec3>], tol: f64) -> Result<(Vec<DVec3>, Vec<usize>), String> {
    if lines.iter().all(|l| l.len() < 2) {
        return Err("Pick the paths around the patch".into());
    }
    let (ps, nodes) = graph(lines, tol);
    let (alive, deg) = prune(&ps, nodes);
    if deg.iter().any(|&d| d > 2) {
        return Err("The paths make more than one loop: pick only those around the patch".into());
    }
    let live: Vec<usize> = (0..ps.len()).filter(|&k| alive[k]).collect();
    if live.is_empty() {
        return Err("The paths do not close a loop".into());
    }
    // walk round
    let mut used = vec![false; ps.len()];
    let mut out: Vec<DVec3> = vec![];
    let mut joints: Vec<usize> = vec![];
    let mut k = live[0];
    let mut node = ps[k].ends[0];
    let start = node;
    let mut prev_from = usize::MAX;
    loop {
        used[k] = true;
        let p = &ps[k];
        let fwd = p.ends[0] == node;
        let mut pts = p.pts.clone();
        if !fwd {
            pts.reverse();
        }
        if prev_from != usize::MAX && prev_from != p.from {
            joints.push(out.len());
        }
        out.extend_from_slice(&pts[..pts.len() - 1]);
        prev_from = p.from;
        node = if fwd { p.ends[1] } else { p.ends[0] };
        if node == start {
            // the start is a meeting point when the last path is not the first
            if p.from != ps[live[0]].from {
                joints.insert(0, 0);
            }
            break;
        }
        match live.iter().copied().find(|&j| !used[j] && (ps[j].ends[0] == node || ps[j].ends[1] == node)) {
            Some(j) => k = j,
            None => return Err("The paths do not close a loop".into()),
        }
    }
    if live.iter().any(|&j| !used[j]) {
        return Err("The paths make more than one loop: pick only those around the patch".into());
    }
    if out.len() < 4 {
        return Err("The loop is too small".into());
    }
    Ok((out, joints))
}

/// The smallest loop of the paths around a point seen on screen (Romain
/// draws a network of paths, then clicks in each of its cells): the paths
/// cut where they meet, what hangs dropped, the pieces seen on screen
/// (`scr`: a point's place on screen, None behind the eye), the cells
/// traced round, and the smallest one around `at` whose box (in space)
/// holds `near` (the scan under the click: a cell on the far side is not
/// it). The loop as runs of one path each, in order, and the paths used.
pub fn loop_around(lines: &[Vec<DVec3>], tol: f64, scr: impl Fn(DVec3) -> Option<DVec2>, at: DVec2, near: DVec3) -> Result<(Vec<Vec<DVec3>>, Vec<usize>), String> {
    let (ps, nodes) = graph(lines, tol);
    let (alive, _) = prune(&ps, nodes);
    let sp: Vec<Option<Vec<DVec2>>> = ps.iter().enumerate().map(|(k, p)| if alive[k] { p.pts.iter().map(|x| scr(*x)).collect() } else { None }).collect();
    // the half-edges leaving each point, by angle on screen
    let leave = |pts: &mut dyn Iterator<Item = DVec2>| -> f64 {
        let a = pts.next().unwrap();
        let mut b = a;
        for q in pts {
            b = q;
            if (q - a).length() > 6.0 {
                break;
            }
        }
        let d = b - a;
        d.y.atan2(d.x)
    };
    let mut out: Vec<Vec<(f64, usize, bool)>> = vec![vec![]; nodes];
    for (k, s) in sp.iter().enumerate() {
        let Some(s) = s else { continue };
        out[ps[k].ends[0]].push((leave(&mut s.iter().copied()), k, true));
        out[ps[k].ends[1]].push((leave(&mut s.iter().rev().copied()), k, false));
    }
    for o in &mut out {
        o.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    }
    let end_of = |k: usize, fwd: bool| if fwd { ps[k].ends[1] } else { ps[k].ends[0] };
    // each cell: from a half-edge, at its end turn to the next one round
    // after the way back
    let mut seen: std::collections::HashSet<(usize, bool)> = std::collections::HashSet::new();
    let total: usize = out.iter().map(|o| o.len()).sum();
    let mut best: Option<(f64, Vec<(usize, bool)>)> = None;
    for o in &out {
        for &(_, k0, f0) in o {
            if seen.contains(&(k0, f0)) {
                continue;
            }
            let mut face = vec![];
            let (mut k, mut f) = (k0, f0);
            let mut ok = false;
            for _ in 0..=total {
                seen.insert((k, f));
                face.push((k, f));
                let v = end_of(k, f);
                let o = &out[v];
                let Some(i) = o.iter().position(|e| e.1 == k && e.2 != f) else { break };
                let next = o[(i + 1) % o.len()];
                (k, f) = (next.1, next.2);
                if (k, f) == (k0, f0) {
                    ok = true;
                    break;
                }
            }
            if !ok {
                continue;
            }
            // its outline on screen and in space
            let mut poly: Vec<DVec2> = vec![];
            let mut pts3: Vec<DVec3> = vec![];
            for &(k, f) in &face {
                let s = sp[k].as_ref().unwrap();
                if f {
                    poly.extend_from_slice(&s[..s.len() - 1]);
                    pts3.extend_from_slice(&ps[k].pts);
                } else {
                    poly.extend(s.iter().rev().take(s.len() - 1));
                    pts3.extend(ps[k].pts.iter().rev());
                }
            }
            if poly.len() < 3 {
                continue;
            }
            let area = poly.iter().zip(poly.iter().cycle().skip(1)).map(|(a, b)| a.x * b.y - a.y * b.x).sum::<f64>().abs() * 0.5;
            let mut inside = false;
            for i in 0..poly.len() {
                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                if (a.y > at.y) != (b.y > at.y) && at.x < a.x + (at.y - a.y) / (b.y - a.y) * (b.x - a.x) {
                    inside = !inside;
                }
            }
            let (lo, hi) = pts3.iter().fold((DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)), |(l, h), p| (l.min(*p), h.max(*p)));
            let pad = ((hi - lo).length() * 0.3).max(tol);
            let holds = near.cmpge(lo - pad).all() && near.cmple(hi + pad).all();
            if std::env::var("RECTIFY_PATCH_DEBUG").is_ok() {
                eprintln!("cell: {} pieces from {:?} · area {area:.0} · inside {inside} · holds {holds} · near {near:.2} box {lo:.2} {hi:.2}", face.len(), face.iter().map(|f| ps[f.0].from).collect::<Vec<_>>());
            }
            if inside && holds && best.as_ref().map(|b| area < b.0).unwrap_or(true) {
                best = Some((area, face));
            }
        }
    }
    let Some((_, face)) = best else {
        return Err("No loop of paths around this point: draw the paths round it first (Line), their ends on each other".into());
    };
    // runs of one path each; the last joined to the first when the same
    let mut runs: Vec<(usize, Vec<DVec3>)> = vec![];
    for &(k, f) in &face {
        let mut pts = ps[k].pts.clone();
        if !f {
            pts.reverse();
        }
        match runs.last_mut() {
            Some((from, r)) if *from == ps[k].from => r.extend_from_slice(&pts[1..]),
            _ => runs.push((ps[k].from, pts)),
        }
    }
    if runs.len() > 1 && runs[0].0 == runs[runs.len() - 1].0 {
        let (_, last) = runs.pop().unwrap();
        let mut joined = last;
        joined.extend_from_slice(&runs[0].1[1..]);
        runs[0].1 = joined;
    }
    let mut used: Vec<usize> = runs.iter().map(|r| r.0).collect();
    used.sort_unstable();
    used.dedup();
    Ok((runs.into_iter().map(|r| r.1).collect(), used))
}

/// How much a closed polyline turns at point `i` (radians), seen over
/// `w` of its length on each side.
fn turn(p: &[DVec3], s: &[f64], i: usize, w: f64) -> f64 {
    let n = p.len();
    let l = s[n];
    let point = |x: f64| -> DVec3 {
        let x = x.rem_euclid(l);
        let k = s.partition_point(|&v| v < x).clamp(1, n);
        let f = ((x - s[k - 1]) / (s[k] - s[k - 1]).max(1e-12)).clamp(0.0, 1.0);
        p[k - 1].lerp(p[k % n], f)
    };
    let (a, b) = (point(s[i] - w), point(s[i] + w));
    let (d0, d1) = ((p[i] - a).normalize_or_zero(), (b - p[i]).normalize_or_zero());
    d0.dot(d1).clamp(-1.0, 1.0).acos()
}

/// Four corners on the loop, in order: its meeting points (the sharpest
/// when more than 4), then its sharpest turns (35° and more), then the
/// middle of its longest side.
fn corners(p: &[DVec3], joints: &[usize]) -> [usize; 4] {
    let n = p.len();
    let mut s = arc(p);
    s.push(s[n - 1] + (p[0] - p[n - 1]).length());
    let l = s[n];
    let w = l * 0.03;
    let sep = l * 0.08;
    let tn: Vec<f64> = (0..n).map(|i| turn(p, &s, i, w)).collect();
    let mut c: Vec<usize> = joints.to_vec();
    while c.len() > 4 {
        let k = (0..c.len()).min_by(|&a, &b| tn[c[a]].partial_cmp(&tn[c[b]]).unwrap()).unwrap();
        c.remove(k);
    }
    let gap = |a: usize, b: usize| {
        let d = (s[a] - s[b]).abs();
        d.min(l - d)
    };
    while c.len() < 4 {
        let best = (0..n).filter(|&i| c.iter().all(|&k| gap(i, k) > sep)).max_by(|&a, &b| tn[a].partial_cmp(&tn[b]).unwrap());
        match best {
            Some(i) if tn[i] >= 35f64.to_radians() || c.is_empty() => c.push(i),
            _ => {
                // the middle of the longest side
                c.sort_unstable();
                let m = c.len();
                let (mut bi, mut bl) = (0, -1.0);
                for k in 0..m {
                    let (a, b) = (c[k], c[(k + 1) % m]);
                    let len = if m == 1 { l } else { (s[b] - s[a]).rem_euclid(l) };
                    if len > bl {
                        bl = len;
                        bi = k;
                    }
                }
                let mid = (s[c[bi]] + bl * 0.5).rem_euclid(l);
                let i = (0..n).min_by(|&a, &b| (s[a] - mid).abs().partial_cmp(&(s[b] - mid).abs()).unwrap()).unwrap();
                if c.contains(&i) {
                    break;
                }
                c.push(i);
            }
        }
    }
    c.sort_unstable();
    while c.len() < 4 {
        c.push(c[c.len() - 1]);
    }
    [c[0], c[1], c[2], c[3]]
}

/// The four sides of the loop the paths close, each from its corner to the
/// next.
pub fn loop_sides(lines: &[Vec<DVec3>], tol: f64) -> Result<[Vec<DVec3>; 4], String> {
    let (p, joints) = chain(lines, tol)?;
    let c = corners(&p, &joints);
    if c.windows(2).any(|w| w[0] == w[1]) {
        return Err("The loop is too small".into());
    }
    let n = p.len();
    let side = |a: usize, b: usize| -> Vec<DVec3> {
        let mut v = vec![];
        let mut i = a;
        loop {
            v.push(p[i]);
            if i == b {
                break;
            }
            i = (i + 1) % n;
        }
        v
    };
    Ok([side(c[0], c[1]), side(c[1], c[2]), side(c[2], c[3]), side(c[3], c[0])])
}

/// The bicubic surface with `su` × `sv` equal spans nearest the weighted
/// points `(u, v, point, weight)`; `lam` keeps its control net calm.
fn fit(data: &[(f64, f64, DVec3, f64)], su: usize, sv: usize, lam: f64) -> Option<Surface> {
    let (ku, kv) = (uniform_knots(3, su), uniform_knots(3, sv));
    let (nu, nv) = (su + 3, sv + 3);
    let nn = nu * nv;
    let mut ata = vec![vec![0.0f64; nn]; nn];
    let mut atb = vec![DVec3::ZERO; nn];
    for &(u, v, p, w) in data {
        let (u, v) = (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
        let (spu, spv) = (find_span(&ku, 3, nu, u), find_span(&kv, 3, nv, v));
        let (bu, bv) = (basis(&ku, 3, spu, u), basis(&kv, 3, spv, v));
        let mut idx = [(0usize, 0.0f64); 16];
        for b in 0..4 {
            for a in 0..4 {
                idx[b * 4 + a] = ((spv - 3 + b) * nu + spu - 3 + a, bu[a] * bv[b]);
            }
        }
        for &(i, ci) in &idx {
            atb[i] += p * (w * ci);
            for &(j, cj) in &idx {
                ata[i][j] += w * ci * cj;
            }
        }
    }
    // the control net's second differences along u and along v
    let mut bend = |ks: [usize; 3]| {
        let c = [1.0, -2.0, 1.0];
        for a in 0..3 {
            for b in 0..3 {
                ata[ks[a]][ks[b]] += lam * c[a] * c[b];
            }
        }
    };
    for j in 0..nv {
        for i in 1..nu - 1 {
            bend([j * nu + i - 1, j * nu + i, j * nu + i + 1]);
        }
    }
    for i in 0..nu {
        for j in 1..nv - 1 {
            bend([(j - 1) * nu + i, j * nu + i, (j + 1) * nu + i]);
        }
    }
    let xs = solve(ata.clone(), atb.iter().map(|p| p.x).collect())?;
    let ys = solve(ata.clone(), atb.iter().map(|p| p.y).collect())?;
    let zs = solve(ata, atb.iter().map(|p| p.z).collect())?;
    let pts = (0..nn).map(|i| DVec3::new(xs[i], ys[i], zs[i])).collect();
    Some(Surface { du: 3, dv: 3, ku, kv, nu, nv, pts, w: vec![1.0; nn] })
}

/// The Coons surface of four sides: bottom c0 → c1, right c1 → c2, top
/// c2 → c3, left c3 → c0.
struct Coons<'a> {
    sides: &'a [Vec<DVec3>; 4],
    arcs: [Vec<f64>; 4],
    c: [DVec3; 4],
}

impl<'a> Coons<'a> {
    fn new(sides: &'a [Vec<DVec3>; 4]) -> Coons<'a> {
        let arcs = [arc(&sides[0]), arc(&sides[1]), arc(&sides[2]), arc(&sides[3])];
        Coons { sides, arcs, c: [sides[0][0], sides[1][0], sides[2][0], sides[3][0]] }
    }

    fn side(&self, k: usize, t: f64) -> DVec3 {
        at(&self.sides[k], &self.arcs[k], t)
    }

    fn eval(&self, u: f64, v: f64) -> DVec3 {
        let b = self.side(0, u);
        let r = self.side(1, v);
        let t = self.side(2, 1.0 - u);
        let l = self.side(3, 1.0 - v);
        let c = &self.c;
        b * (1.0 - v) + t * v + l * (1.0 - u) + r * u - (c[0] * ((1.0 - u) * (1.0 - v)) + c[1] * (u * (1.0 - v)) + c[2] * (u * v) + c[3] * ((1.0 - u) * v))
    }

    fn normal(&self, u: f64, v: f64) -> DVec3 {
        let e = 0.02;
        let du = self.eval((u + e).min(1.0), v) - self.eval((u - e).max(0.0), v);
        let dv = self.eval(u, (v + e).min(1.0)) - self.eval(u, (v - e).max(0.0));
        du.cross(dv).normalize_or_zero()
    }

    /// The border at (u, v) on it (one of them 0 or 1).
    fn border(&self, u: f64, v: f64) -> DVec3 {
        if v <= 0.0 {
            self.side(0, u)
        } else if u >= 1.0 {
            self.side(1, v)
        } else if v >= 1.0 {
            self.side(2, 1.0 - u)
        } else {
            self.side(3, 1.0 - v)
        }
    }
}

/// Points round the border: (u, v), `n` per side.
fn border_uv(n: usize) -> Vec<(f64, f64)> {
    let mut out = vec![];
    for k in 0..n {
        let t = k as f64 / n as f64;
        out.push((t, 0.0));
        out.push((1.0, t));
        out.push((1.0 - t, 1.0));
        out.push((0.0, 1.0 - t));
    }
    out
}

/// The patch filling the four sides (each from its corner to the next),
/// laid on the scan. `h`: the scan's edge length.
pub fn patch(m: &Mesh, scan: &Closest, sides: &[Vec<DVec3>; 4], p: PatchParams, h: f64) -> Result<Patched, String> {
    let mut sides = sides.clone();
    // the surface faces the way the scan does under the sides
    let newell = {
        let pts: Vec<DVec3> = sides.iter().flat_map(|s| s[..s.len() - 1].iter().copied()).collect();
        let mut n = DVec3::ZERO;
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            n += DVec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
        }
        n
    };
    let reach = (h * 8.0).max(0.5);
    let facing: DVec3 = sides.iter().flat_map(|s| s.iter().step_by((s.len() / 8).max(1))).filter_map(|x| scan.closest(m, *x, reach)).map(|(t, _)| m.tri_normal(t as usize)).sum();
    if facing.dot(newell) < 0.0 {
        sides = [sides[3].clone(), sides[2].clone(), sides[1].clone(), sides[0].clone()];
        for s in &mut sides {
            s.reverse();
        }
    }
    let co = Coons::new(&sides);
    let len = |k: usize| *co.arcs[k].last().unwrap();
    let (lu, lv) = ((len(0) + len(2)) * 0.5, (len(1) + len(3)) * 0.5);
    if lu < h || lv < h {
        return Err("The loop is too small".into());
    }
    let size = lu.max(lv);
    let far = (h * 40.0).min(size * 0.25).max(reach);
    // the scan straight across from the point (along the normal: the grid
    // stays as even as it is), facing the same way; else its closest point
    let onto = |x: DVec3, n: DVec3| -> Option<DVec3> {
        let keep = |t: u32| m.tri_normal(t as usize).dot(n) > 0.2;
        if n != DVec3::ZERO {
            if let Some((_, s)) = scan.hit_along(m, x, n, far, keep) {
                return Some(x + n * s);
            }
        }
        scan.closest_where(m, x, reach, keep).or_else(|| scan.closest_where(m, x, far, keep)).map(|r| r.1)
    };
    const G: usize = 28;
    let grid: Vec<(f64, f64)> = (1..G).flat_map(|j| (1..G).map(move |i| (i as f64 / G as f64, j as f64 / G as f64))).collect();
    let border: Vec<(f64, f64, DVec3, f64)> = border_uv(G * 3).into_iter().map(|(u, v)| (u, v, co.border(u, v), 1e3)).collect();
    // the scan's own roughness: farther than this weighs less, and counts as
    // off the scan when the spans are chosen (a button inside the loop is
    // passed over, not followed)
    let sigma = (h * 1.5).max(0.1);
    let lam_for = |n_data: usize, su: usize, sv: usize| 0.001 * n_data as f64 / ((su + 3) * (sv + 3)) as f64;
    let spans_for = |k: usize| -> (usize, usize) {
        let o = |r: f64| ((k as f64 * r).round() as usize).clamp(1, 12);
        if lu >= lv {
            (k, o(lv / lu))
        } else {
            (o(lu / lv), k)
        }
    };
    // the inside on the scan (or the Coons surface itself)
    let mut inner: Vec<(f64, f64, DVec3)> = grid
        .iter()
        .filter_map(|&(u, v)| {
            let x = co.eval(u, v);
            if p.follow {
                onto(x, co.normal(u, v)).map(|q| (u, v, q))
            } else {
                Some((u, v, x))
            }
        })
        .collect();
    if p.follow && inner.len() < grid.len() / 3 {
        return Err("The scan is not under this loop".into());
    }
    // the first weights: points far from the Coons surface of the sides
    // weigh less, against how far most are (on a flat part a button stands
    // out; on a bent one every point is off a little)
    let prior = |inner: &[(f64, f64, DVec3)]| -> Vec<f64> {
        let res: Vec<f64> = inner.iter().map(|&(u, v, q)| (co.eval(u, v) - q).length()).collect();
        let mut sorted = res.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let s0 = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0).max(sigma);
        res.iter().map(|r| 1.0 / (1.0 + (r / s0).powi(2))).collect()
    };
    // one fit (the points left out weigh nothing), points far off weigh
    // less (Cauchy)
    let fit_robust = |inner: &[(f64, f64, DVec3)], keep: &[bool], su: usize, sv: usize| -> Option<Surface> {
        let mut w: Vec<f64> = if p.follow { prior(inner) } else { vec![1.0; inner.len()] };
        for (w, &k) in w.iter_mut().zip(keep) {
            if !k {
                *w = 0.0;
            }
        }
        let mut s = None;
        let iters = if p.follow { 5 } else { 1 };
        for it in 0..iters {
            let mut data = border.clone();
            data.extend(inner.iter().zip(&w).filter(|(_, &w)| w > 0.0).map(|(&(u, v, q), &w)| (u, v, q, w)));
            let f = fit(&data, su, sv, lam_for(inner.len(), su, sv))?;
            if it + 1 < iters {
                for (k, &(u, v, q)) in inner.iter().enumerate() {
                    if keep[k] {
                        let r = (f.eval(u, v) - q).length() / sigma;
                        w[k] = 1.0 / (1.0 + r * r);
                    }
                }
            }
            s = Some(f);
        }
        s
    };
    let all = vec![true; inner.len()];
    let mut keep = all.clone();
    if p.follow {
        // the inside put on the scan again, from a first fit
        let (su, sv) = spans_for(4);
        if let Some(f) = fit_robust(&inner, &all, su, sv) {
            let again: Vec<(f64, f64, DVec3)> = grid
                .iter()
                .filter_map(|&(u, v)| {
                    let x = f.eval(u, v);
                    let e = 0.01;
                    let n = (f.eval((u + e).min(1.0), v) - f.eval((u - e).max(0.0), v)).cross(f.eval(u, (v + e).min(1.0)) - f.eval(u, (v - e).max(0.0))).normalize_or_zero();
                    onto(x, n).map(|q| (u, v, q))
                })
                .collect();
            if again.len() >= inner.len() * 2 / 3 {
                inner = again;
            }
        }
        // what stands far from a calm surface (2 spans the long way) is a
        // detail inside the loop (a button, a hole): passed over, left out
        // (twice: the second calm surface no longer lifted by it)
        let (su, sv) = spans_for(2);
        keep = vec![true; inner.len()];
        for round in 0..2 {
            let Some(f) = fit_robust(&inner, &keep, su, sv) else { break };
            let res: Vec<f64> = inner.iter().map(|&(u, v, q)| (f.eval(u, v) - q).length()).collect();
            let mut sorted: Vec<f64> = res.iter().zip(&keep).filter(|(_, &k)| k).map(|(r, _)| *r).collect();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = sorted[sorted.len() / 2];
            let lim = (sigma * 2.5).max(med * 4.0);
            keep = res.iter().map(|&r| r <= lim).collect();
            if std::env::var("RECTIFY_PATCH_DEBUG").is_ok() {
                eprintln!("patch calm {round} {su}x{sv}: median {med:.3} max {:.3} lim {lim:.3} kept {}", sorted[sorted.len() - 1], keep.iter().filter(|&&k| k).count());
            }
            if keep.iter().filter(|&&k| k).count() < inner.len() / 3 {
                keep = vec![true; inner.len()];
                break;
            }
        }
    }
    let kept = keep.iter().filter(|&&k| k).count().max(1);
    // how well a surface does: the points kept from it (capped: a little
    // off is off), its border from the sides
    let judge = |f: &Surface| -> (f64, f64, f64, f64) {
        let (mut robust, mut sq) = (0.0, 0.0);
        for (k, &(u, v, q)) in inner.iter().enumerate() {
            if keep[k] {
                let d = (f.eval(u, v) - q).length();
                robust += d.min(sigma * 1.5).powi(2);
                sq += d * d;
            }
        }
        let (mut gap, mut gap_sq) = (0.0f64, 0.0);
        let bs = border_uv(48);
        for &(u, v) in &bs {
            let d = (f.eval(u, v) - co.border(u, v)).length();
            gap = gap.max(d);
            gap_sq += d * d;
        }
        ((robust / kept as f64).sqrt(), (sq / kept as f64).sqrt(), gap, (gap_sq / bs.len() as f64).sqrt())
    };
    let ks: Vec<usize> = if p.spans > 0 { vec![p.spans.min(12)] } else { (1..=8).collect() };
    let mut tried: Vec<(usize, Surface, f64, f64, f64)> = vec![];
    for k in ks {
        let (su, sv) = spans_for(k);
        let Some(f) = fit_robust(&inner, &keep, su, sv) else { continue };
        let (robust, off, gap, gap_rms) = judge(&f);
        if std::env::var("RECTIFY_PATCH_DEBUG").is_ok() {
            eprintln!("patch k {k} spans {su}x{sv} robust {robust:.4} off {off:.4} gap {gap:.4} gap_rms {gap_rms:.4} kept {kept}/{}", inner.len());
        }
        tried.push((k, f, robust + gap_rms, off, gap));
    }
    let best = tried.iter().map(|t| t.2).fold(f64::INFINITY, f64::min);
    let Some((_, f, _, off, gap)) = tried.into_iter().find(|t| t.2 <= best * 1.2 + (h * 0.1).max(0.01)) else {
        return Err("No surface fits this loop".into());
    };
    let spans = [f.nu - 3, f.nv - 3];
    let passed = 1.0 - kept as f64 / inner.len().max(1) as f64;
    Ok(Patched { surface: f, spans, off, gap, passed, corners: co.c })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(a: DVec3, b: DVec3) -> Vec<DVec3> {
        (0..=20).map(|k| a.lerp(b, k as f64 / 20.0)).collect()
    }

    fn v(x: f64, y: f64) -> DVec3 {
        DVec3::new(x, y, 0.0)
    }

    #[test]
    fn four_lines_make_four_sides() {
        let ls = vec![line(v(0.0, 0.0), v(10.0, 0.0)), line(v(10.0, 6.0), v(10.0, 0.0)), line(v(10.0, 6.0), v(0.0, 6.0)), line(v(0.0, 6.0), v(0.0, 0.0))];
        let s = loop_sides(&ls, 0.1).unwrap();
        let lens: Vec<f64> = s.iter().map(|x| *arc(x).last().unwrap()).collect();
        let mut sorted = lens.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!((sorted[0] - 6.0).abs() < 1e-6 && (sorted[3] - 10.0).abs() < 1e-6, "{lens:?}");
    }

    #[test]
    fn hanging_ends_are_dropped() {
        // the bottom runs past both corners, the sides land on it
        let ls = vec![line(v(-3.0, 0.0), v(13.0, 0.0)), line(v(10.0, 0.0), v(10.0, 6.0)), line(v(10.0, 6.0), v(0.0, 6.0)), line(v(0.0, 6.0), v(0.0, 0.0))];
        let s = loop_sides(&ls, 0.1).unwrap();
        let total: f64 = s.iter().map(|x| *arc(x).last().unwrap()).sum();
        assert!((total - 32.0).abs() < 1e-6, "{total}");
    }

    #[test]
    fn a_click_finds_its_cell() {
        // two cells side by side, the middle line's ends on the long ones
        let ls = vec![line(v(0.0, 0.0), v(20.0, 0.0)), line(v(20.0, 0.0), v(20.0, 10.0)), line(v(20.0, 10.0), v(0.0, 10.0)), line(v(0.0, 10.0), v(0.0, 0.0)), line(v(10.0, 0.0), v(10.0, 10.0))];
        let scr = |x: DVec3| Some(DVec2::new(x.x, x.y));
        for (x, other) in [(5.0, 15.0), (15.0, 5.0)] {
            let (runs, used) = loop_around(&ls, 0.1, scr, DVec2::new(x, 5.0), v(x, 5.0)).unwrap();
            assert_eq!(used.len(), 4);
            assert!(used.contains(&4));
            let total: f64 = runs.iter().map(|r| *arc(r).last().unwrap()).sum();
            assert!((total - 40.0).abs() < 1e-6, "{total}");
            // every point within the cell
            assert!(runs.iter().flatten().all(|p| (p.x - x).abs() <= 5.0 + 1e-9 && (p.x - other).abs() >= 5.0 - 1e-9));
            let sides = loop_sides(&runs, 0.1).unwrap();
            let lens: Vec<f64> = sides.iter().map(|x| *arc(x).last().unwrap()).collect();
            assert!(lens.iter().all(|l| (l - 10.0).abs() < 1e-6), "{lens:?}");
        }
        // outside every cell
        assert!(loop_around(&ls, 0.1, scr, DVec2::new(25.0, 5.0), v(25.0, 5.0)).is_err());
    }

    #[test]
    fn three_lines_get_a_fourth_corner() {
        let ls = vec![line(v(0.0, 0.0), v(10.0, 0.0)), line(v(10.0, 0.0), v(10.0, 8.0)), line(v(10.0, 8.0), v(0.0, 0.0))];
        let s = loop_sides(&ls, 0.1).unwrap();
        assert!(s.iter().all(|x| *arc(x).last().unwrap() > 1.0));
    }

    #[test]
    fn a_circle_is_cut_in_quarters() {
        let c = Curve::circle(DVec3::ZERO, DVec3::X, DVec3::Y, 5.0);
        let s = loop_sides(&[polyline(&c)], 0.1).unwrap();
        for x in &s {
            let l = *arc(x).last().unwrap();
            assert!((l - std::f64::consts::PI * 2.5).abs() < 0.3, "{l}");
        }
    }

    fn sheet(f: impl Fn(f64, f64) -> f64) -> Mesh {
        let n = 60;
        let mut pos = vec![];
        for j in 0..=n {
            for i in 0..=n {
                let (x, y) = (-15.0 + 30.0 * i as f64 / n as f64, -15.0 + 30.0 * j as f64 / n as f64);
                pos.push(DVec3::new(x, y, f(x, y)));
            }
        }
        let mut tris = vec![];
        let id = |i: usize, j: usize| (j * (n + 1) + i) as u32;
        for j in 0..n {
            for i in 0..n {
                tris.push([id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
                tris.push([id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
            }
        }
        Mesh { pos, tris, unit_mm: 1.0, ..Default::default() }
    }

    #[test]
    fn a_patch_follows_a_bump() {
        let f = |x: f64, y: f64| 3.0 * (-(x * x + y * y) / 40.0).exp();
        let m = sheet(f);
        let scan = Closest::new(&m);
        let on = |a: (f64, f64), b: (f64, f64)| -> Vec<DVec3> {
            (0..=40).map(|k| {
                let t = k as f64 / 40.0;
                let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                DVec3::new(x, y, f(x, y))
            }).collect()
        };
        let ls = vec![on((-8.0, -6.0), (8.0, -6.0)), on((8.0, -6.0), (8.0, 6.0)), on((8.0, 6.0), (-8.0, 6.0)), on((-8.0, 6.0), (-8.0, -6.0))];
        let sides = loop_sides(&ls, 0.1).unwrap();
        let p = patch(&m, &scan, &sides, PatchParams::default(), 0.2).unwrap();
        assert!(p.off < 0.05, "off {}", p.off);
        assert!(p.gap < 0.1, "gap {}", p.gap);
        // the surface faces up, as the scan
        let s = &p.surface;
        let n = (s.eval(0.6, 0.5) - s.eval(0.4, 0.5)).cross(s.eval(0.5, 0.6) - s.eval(0.5, 0.4));
        assert!(n.z > 0.0);
        // the middle on the bump's top
        assert!((s.eval(0.5, 0.5).z - 3.0).abs() < 0.1, "{}", s.eval(0.5, 0.5).z);
        // a flat sheet: one span
        let m2 = sheet(|_, _| 0.0);
        let scan2 = Closest::new(&m2);
        let flat = vec![line(v(-8.0, -6.0), v(8.0, -6.0)), line(v(8.0, -6.0), v(8.0, 6.0)), line(v(8.0, 6.0), v(-8.0, 6.0)), line(v(-8.0, 6.0), v(-8.0, -6.0))];
        let p2 = patch(&m2, &scan2, &loop_sides(&flat, 0.1).unwrap(), PatchParams::default(), 0.5).unwrap();
        assert_eq!(p2.spans, [1, 1]);
        assert!(p2.off < 0.01);
    }
}
