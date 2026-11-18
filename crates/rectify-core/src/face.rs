//! The Face tool (Romain, 18 nov. 2026, 18 h 56: "si tu as une idée pour
//! continuer à identifier toi-même comme pour les primitives"): a click on
//! a flat face finds all of it and its outline, made a closed path on its
//! plane (`outline_path`), and fills it with a surface over a rectangle of
//! its plane, laid on the face, cut by that path (`fill`: a trimmed
//! surface; one four-sided patch twisted on faces of a dozen corners).
//!
//! The face's plane comes from the flat faces near the click
//! (`path::planes_near`); then the triangles joined to the one clicked are
//! taken while they face the same way (within 12°) and lie within half a
//! millimetre of the plane (an AI's flat faces bow a little); the plane is
//! fitted again to them and they are taken again; thin tongues along a
//! fillet are worn away (`open`). The outline is the longest loop of their
//! border (holes, a button inside the face, are left: the surface passes
//! over them), put on the plane and smoothed (the triangles' zigzag gone).

use crate::mesh::{fit_plane, Mesh, Topo, NONE};
use crate::nurbs::{uniform_knots, Curve, Surface};
use crate::patch::{fit, PatchParams, Patched};
use crate::path::{planes_near, stroke_keys};
use glam::{DVec2, DVec3};
use std::collections::{HashMap, VecDeque};

/// A flat face found under a click.
#[derive(Clone, Debug)]
pub struct Face {
    /// A point of its plane and its unit normal (out of the material).
    pub c: DVec3,
    pub n: DVec3,
    /// Its triangles and their area (mm²).
    pub tris: Vec<u32>,
    pub area: f64,
    /// Its outer outline on the plane, closed (the first point not
    /// repeated), how many holes it has and their area (mm², on the plane).
    pub outline: Vec<DVec3>,
    pub holes: usize,
    pub holes_area: f64,
}

/// The triangles joined to `t0` that face along `n` (within `cos`) and lie
/// within `tol` of the plane through `c`.
fn grow(m: &Mesh, topo: &Topo, t0: u32, c: DVec3, n: DVec3, cos: f64, tol: f64) -> Vec<u32> {
    let ok = |t: u32| m.tri_normal(t as usize).dot(n) >= cos && n.dot(m.tri_centre(t as usize) - c).abs() <= tol;
    if !ok(t0) {
        return vec![];
    }
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::from([t0]);
    let mut out = vec![t0];
    let mut q = VecDeque::from([t0]);
    while let Some(t) = q.pop_front() {
        if out.len() > 2_000_000 {
            break;
        }
        for &u in &topo.adj[t as usize] {
            if u != NONE && seen.insert(u) && ok(u) {
                out.push(u);
                q.push_back(u);
            }
        }
    }
    out
}

/// A set of triangles without its thin tongues (narrower than about
/// `r` × 2 triangles, along a fillet): worn away `r` rings, grown back as
/// far within the set, the part joined to `t0` kept (or all of it, when
/// wearing it leaves nothing there).
fn open(topo: &Topo, tris: &[u32], t0: u32, r: usize) -> Vec<u32> {
    use std::collections::HashSet;
    let all: HashSet<u32> = tris.iter().copied().collect();
    let mut cur = all.clone();
    for _ in 0..r {
        cur = cur.iter().copied().filter(|&t| topo.adj[t as usize].iter().all(|&u| u != NONE && cur.contains(&u))).collect();
    }
    for _ in 0..r {
        let add: Vec<u32> = cur.iter().flat_map(|&t| topo.adj[t as usize].iter().copied()).filter(|&u| u != NONE && all.contains(&u) && !cur.contains(&u)).collect();
        cur.extend(add);
    }
    // the part joined to the click (the triangle nearest it when the click
    // itself was worn away)
    let seed = if cur.contains(&t0) { Some(t0) } else { None };
    let Some(seed) = seed.or_else(|| {
        let mut q = VecDeque::from([t0]);
        let mut seen: HashSet<u32> = HashSet::from([t0]);
        while let Some(t) = q.pop_front() {
            if cur.contains(&t) {
                return Some(t);
            }
            for &u in &topo.adj[t as usize] {
                if u != NONE && all.contains(&u) && seen.insert(u) {
                    q.push_back(u);
                }
            }
        }
        None
    }) else {
        return tris.to_vec();
    };
    let mut out = vec![seed];
    let mut seen: HashSet<u32> = HashSet::from([seed]);
    let mut q = VecDeque::from([seed]);
    while let Some(t) = q.pop_front() {
        for &u in &topo.adj[t as usize] {
            if u != NONE && cur.contains(&u) && seen.insert(u) {
                out.push(u);
                q.push_back(u);
            }
        }
    }
    out
}

/// The border loops of a set of triangles (each a list of vertices, in
/// order), longest first.
fn borders(m: &Mesh, topo: &Topo, tris: &[u32]) -> Vec<Vec<u32>> {
    let inside: std::collections::HashSet<u32> = tris.iter().copied().collect();
    // the sides with nothing of the set across: from a vertex to the next
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut count = 0usize;
    for &t in tris {
        let v = m.tris[t as usize];
        for k in 0..3 {
            let u = topo.adj[t as usize][k];
            if u == NONE || !inside.contains(&u) {
                next.entry(v[k]).or_default().push(v[(k + 1) % 3]);
                count += 1;
            }
        }
    }
    let mut loops: Vec<Vec<u32>> = vec![];
    let starts: Vec<u32> = next.keys().copied().collect();
    for s in starts {
        while next.get(&s).map(|v| !v.is_empty()).unwrap_or(false) {
            let mut lp = vec![s];
            let mut cur = s;
            for _ in 0..=count {
                let Some(nx) = next.get_mut(&cur).and_then(|v| v.pop()) else { break };
                if nx == s {
                    break;
                }
                lp.push(nx);
                cur = nx;
            }
            if lp.len() >= 3 {
                loops.push(lp);
            }
        }
    }
    let len = |l: &Vec<u32>| (0..l.len()).map(|i| (m.pos[l[i] as usize] - m.pos[l[(i + 1) % l.len()] as usize]).length()).sum::<f64>();
    loops.sort_by(|a, b| len(b).partial_cmp(&len(a)).unwrap());
    loops
}

/// The flat face under a click: the triangle `t0`, the point `p` on it;
/// `h`: the scan's edge length.
pub fn find(m: &Mesh, topo: &Topo, t0: u32, p: DVec3, h: f64) -> Result<Face, String> {
    let n0 = m.tri_normal(t0 as usize);
    let reach = (h * 25.0).max(2.0);
    let planes = planes_near(m, topo, t0, p, reach, h);
    let pl = planes
        .iter()
        .filter(|pl| pl.n.dot(n0) >= 15f64.to_radians().cos() && pl.dist(p).abs() <= pl.rms * 3.0 + h * 0.5)
        .min_by(|a, b| a.dist(p).abs().partial_cmp(&b.dist(p).abs()).unwrap())
        .ok_or_else(|| "No flat face here: click on a flat part".to_string())?;
    let cos = 12f64.to_radians().cos();
    let tol = (pl.rms * 4.0).max(h * 3.0).max(0.5);
    let (mut c, mut n) = (pl.c, pl.n);
    let mut tris = grow(m, topo, t0, c, n, cos, tol);
    // the plane again from all of it, and the face again
    let pts: Vec<(DVec3, f64)> = tris.iter().map(|&t| (m.tri_centre(t as usize), m.tri_area(t as usize))).collect();
    if let Some((c2, mut n2)) = fit_plane(&pts) {
        if n2.dot(n) < 0.0 {
            n2 = -n2;
        }
        if n2.dot(n) >= 3f64.to_radians().cos() {
            c = c2;
            n = n2;
            let again = grow(m, topo, t0, c, n, cos, tol);
            if again.len() >= tris.len() / 2 {
                tris = again;
            }
        }
    }
    let tris = open(topo, &tris, t0, 3);
    if tris.len() < 8 {
        return Err("No flat face here: click on a flat part".into());
    }
    let area: f64 = tris.iter().map(|&t| m.tri_area(t as usize)).sum();
    let loops = borders(m, topo, &tris);
    let Some(outer) = loops.first() else { return Err("This face has no outline".into()) };
    // on the plane, in its own frame, smoothed: the zigzag of the triangles'
    // sides gone (a moving average over about two edges), then back
    let u = n.any_orthonormal_vector();
    let v = n.cross(u);
    let flat: Vec<DVec2> = outer.iter().map(|&i| {
        let d = m.pos[i as usize] - c;
        DVec2::new(d.dot(u), d.dot(v))
    }).collect();
    let k = flat.len();
    let mut sm = flat.clone();
    for _ in 0..4 {
        let prev = sm.clone();
        for i in 0..k {
            sm[i] = (prev[(i + k - 1) % k] + prev[i] * 2.0 + prev[(i + 1) % k]) * 0.25;
        }
    }
    let outline = sm.iter().map(|q| c + u * q.x + v * q.y).collect();
    let holes_area = loops[1..]
        .iter()
        .map(|l| {
            let q: Vec<DVec2> = l.iter().map(|&i| {
                let d = m.pos[i as usize] - c;
                DVec2::new(d.dot(u), d.dot(v))
            }).collect();
            signed_area(&q).abs()
        })
        .sum();
    Ok(Face { c, n, tris, area, outline, holes: loops.len() - 1, holes_area })
}

/// The outline as a closed pencil stroke on the plane, starting at its
/// sharpest turn and ending there again (`n`: the plane's normal), in
/// units that make it 1000 long: for `path::stroke_keys` (corners over 12
/// units). Also the outline turned to start there, and whether that turn
/// is a corner (more than 45° over 12 units each side): a round outline
/// has none, its start is a point the curve passes through.
pub fn as_stroke(outline: &[DVec3], n: DVec3) -> (Vec<DVec2>, Vec<DVec3>, bool) {
    let k = outline.len();
    let u = n.any_orthonormal_vector();
    let v = n.cross(u);
    let len: f64 = (0..k).map(|i| (outline[(i + 1) % k] - outline[i]).length()).sum::<f64>().max(1e-9);
    let s = 1000.0 / len;
    // the sharpest turn over 2 % of the outline each side
    let w = (k / 50).max(1);
    let turn = |i: usize| {
        let (a, b, c) = (outline[(i + k - w) % k], outline[i], outline[(i + w) % k]);
        (b - a).normalize_or_zero().dot((c - b).normalize_or_zero())
    };
    let start = (0..k).min_by(|&a, &b| turn(a).partial_cmp(&turn(b)).unwrap()).unwrap_or(0);
    let mut pts: Vec<DVec3> = (0..k).map(|i| outline[(start + i) % k]).collect();
    pts.push(pts[0]);
    let scr: Vec<DVec2> = pts.iter().map(|p| DVec2::new(p.dot(u), p.dot(v)) * s).collect();
    // the turn at the start as stroke_keys sees a corner: 12 units each way
    let along = |back: bool| {
        let mut d = 0.0;
        let mut i = 0usize;
        while d < 12.0 && i < k {
            let (a, b) = if back { (scr[(k - i) % k], scr[(2 * k - i - 1) % k]) } else { (scr[i], scr[i + 1]) };
            d += (b - a).length();
            i += 1;
        }
        if back { scr[(2 * k - i) % k] } else { scr[i.min(k)] }
    };
    let (a, c) = (along(true), along(false));
    let (d1, d2) = (scr[0] - a, c - scr[0]);
    let sharp = d1.length() > 1e-9 && d2.length() > 1e-9 && d1.normalize().dot(d2.normalize()).clamp(-1.0, 1.0).acos().to_degrees() >= 45.0;
    (scr, pts, sharp)
}

/// The outline made a closed path on the face's plane, straight from it
/// (not walked again over the scan): cut at its corners (`path::stroke_keys`
/// on `as_stroke`), each corner put where the sides on either side of it
/// meet (the smoothing rounds it off), a straight side a line, a bent one a
/// smooth cubic through the outline; no corner, one smooth closed curve.
/// Also how many corners and how long it is (mm).
pub fn outline_path(f: &Face, h: f64) -> Result<(Curve, usize, f64), String> {
    let k = f.outline.len();
    if k < 8 {
        return Err("This face's outline is too short".into());
    }
    let (scr, pts, sharp) = as_stroke(&f.outline, f.n);
    let mut keys = stroke_keys(&scr, 45.0, 12.0, 40.0);
    keys.pop();
    let mut corners: Vec<usize> = keys.iter().filter(|q| !q.1).map(|q| q.0).filter(|&i| i < k).collect();
    if !sharp {
        corners.retain(|&i| i != 0);
    }
    let (c, n) = (f.c, f.n);
    let u = n.any_orthonormal_vector();
    let v = n.cross(u);
    let q: Vec<DVec2> = pts[..k].iter().map(|p| DVec2::new((*p - c).dot(u), (*p - c).dot(v))).collect();
    let to3 = |p: DVec2| c + u * p.x + v * p.y;
    let seg = |i: usize| (q[(i + 1) % k] - q[i]).length();
    let total: f64 = (0..k).map(seg).sum();
    // the points from i going forwards (or backwards) between `a` and `b`
    // mm of outline
    let window = |i: usize, fwd: bool, a: f64, b: f64| -> Vec<DVec2> {
        let mut out = vec![];
        let (mut d, mut j) = (0.0, i);
        for _ in 0..k {
            let nj = if fwd { (j + 1) % k } else { (j + k - 1) % k };
            d += (q[nj] - q[j]).length();
            j = nj;
            if d > b {
                break;
            }
            if d >= a {
                out.push(q[j]);
            }
        }
        out
    };
    // a line through points, when they lie along one (a side bent near
    // the corner gives none)
    let line_fit = |p: &[DVec2]| -> Option<(DVec2, DVec2)> {
        if p.len() < 3 {
            return None;
        }
        let m = p.iter().copied().sum::<DVec2>() / p.len() as f64;
        let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
        for x in p {
            let d = *x - m;
            sxx += d.x * d.x;
            syy += d.y * d.y;
            sxy += d.x * d.y;
        }
        let a = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        let d = DVec2::new(a.cos(), a.sin());
        let off = p.iter().map(|x| (*x - m).perp_dot(d).abs()).fold(0.0, f64::max);
        (off <= (h * 1.5).max(0.08)).then_some((m, d))
    };
    let skip = (h * 6.0).max(0.4);
    let reach = (h * 8.0).max(1.0);
    // each corner where its two sides meet (each side's line fitted past the
    // rounding, over a few mm), when they do near it
    let mut at: Vec<DVec2> = vec![];
    let nc = corners.len();
    for (ci, &i) in corners.iter().enumerate() {
        let gap = |a: usize, b: usize| {
            let (mut d, mut j) = (0.0, a);
            while j != b {
                d += seg(j);
                j = (j + 1) % k;
            }
            d
        };
        let (prev, next) = (corners[(ci + nc - 1) % nc], corners[(ci + 1) % nc]);
        let before = if nc == 1 { total } else { gap(prev, i) };
        let after = if nc == 1 { total } else { gap(i, next) };
        let side = |len: f64, fwd: bool| {
            let a = skip.min(len * 0.25);
            let b = (a + 4.0).min(len * 0.6).max(a + h);
            line_fit(&window(i, fwd, a, b))
        };
        let mut p = q[i];
        if let (Some((m1, d1)), Some((m2, d2))) = (side(before, false), side(after, true)) {
            let cr = d1.perp_dot(d2);
            if cr.abs() >= 25f64.to_radians().sin() {
                let x = m1 + d1 * ((m2 - m1).perp_dot(d2) / cr);
                if (x - q[i]).length() <= reach {
                    p = x;
                }
                if std::env::var("RECTIFY_FACE_DEBUG").is_ok() {
                    eprintln!("  corner {ci}: moved {:.3}", (x - q[i]).length());
                }
            }
        }
        at.push(p);
    }
    let fit_run = |run: &[(DVec2, f64)], spans: usize| -> Option<Curve> {
        let mut acc = vec![0.0];
        for w in run.windows(2) {
            acc.push(acc.last().unwrap() + (w[1].0 - w[0].0).length());
        }
        let all = acc.last().copied().unwrap_or(0.0).max(1e-9);
        let data: Vec<(f64, DVec3, f64)> = run.iter().zip(&acc).map(|(&(p, w), t)| (t / all, to3(p), w)).collect();
        let lam = 1e-2 * data.len() as f64 / (spans + 3) as f64;
        Curve::fit_3d(&data, 3, spans, lam)
    };
    let spans_for = |len: f64, lo: usize| ((len / (h * 12.0).max(1.8)).round() as usize).clamp(lo, 48);
    if nc == 0 {
        // one smooth closed curve: fitted with a sixth more at each end,
        // then cut where it passes the start twice
        let w = k / 6;
        let mut run: Vec<(DVec2, f64)> = (k - w..k).map(|i| (q[i], 1.0)).collect();
        let a = run.len();
        run.extend((0..k).map(|i| (q[i], 1.0)));
        let b = run.len();
        run.push((q[0], 1.0));
        run.extend((1..=w).map(|i| (q[i % k], 1.0)));
        let mut acc = vec![0.0];
        for w in run.windows(2) {
            acc.push(acc.last().unwrap() + (w[1].0 - w[0].0).length());
        }
        let all = acc.last().copied().unwrap().max(1e-9);
        let cv = fit_run(&run, spans_for(all, 6)).ok_or("No outline could be made for this face")?;
        let (ta, tb) = (acc[a] / all, acc[b] / all);
        let (_, right) = cv.split(ta).ok_or("No outline could be made for this face")?;
        let (mut mid, _) = right.split(tb).ok_or("No outline could be made for this face")?;
        let m = mid.pts.len();
        let s0 = (mid.pts[0] + mid.pts[m - 1]) * 0.5;
        mid.pts[0] = s0;
        mid.pts[m - 1] = s0;
        return Ok((mid, 0, total));
    }
    // from corner to corner: a line when the outline is straight there, else
    // a cubic through it (its ends on the corners; the rounded bits next to
    // a corner moved left out)
    let mut pieces = vec![];
    let mut lens = vec![];
    for ci in 0..nc {
        let (i, j) = (corners[ci], corners[(ci + 1) % nc]);
        let (a, b) = (at[ci], at[(ci + 1) % nc]);
        let mut inner: Vec<DVec2> = vec![];
        let mut x = (i + 1) % k;
        while x != j {
            inner.push(q[x]);
            x = (x + 1) % k;
        }
        let len: f64 = {
            let mut l = (inner.first().copied().unwrap_or(b) - a).length() + (b - inner.last().copied().unwrap_or(a)).length();
            for w in inner.windows(2) {
                l += (w[1] - w[0]).length();
            }
            l
        };
        let ab = b - a;
        let l2 = ab.length_squared().max(1e-12);
        let far = |p: DVec2| (p - a).length() > skip && (p - b).length() > skip;
        let dev = inner
            .iter()
            .filter(|p| far(**p))
            .map(|p| (*p - (a + ab * ((*p - a).dot(ab) / l2).clamp(0.0, 1.0))).length())
            .fold(0.0, f64::max);
        if dev <= (ab.length() * 0.025).max(h * 2.0).max(0.1) {
            pieces.push(Curve::line(to3(a), to3(b)));
            lens.push(ab.length());
            continue;
        }
        let mut run: Vec<(DVec2, f64)> = vec![(a, 1e6)];
        run.extend(inner.iter().filter(|p| far(**p)).map(|p| (*p, 1.0)));
        run.push((b, 1e6));
        let cv = fit_run(&run, spans_for(len, 1)).unwrap_or_else(|| Curve::line(to3(a), to3(b)));
        pieces.push(cv);
        lens.push(len);
    }
    let (curve, _) = Curve::join(&pieces, &lens);
    Ok((curve, nc, lens.iter().sum()))
}

/// A polygon's signed area (counter-clockwise > 0).
pub fn signed_area(p: &[DVec2]) -> f64 {
    let k = p.len();
    (0..k).map(|i| p[i].perp_dot(p[(i + 1) % k])).sum::<f64>() * 0.5
}

/// Whether `q` is inside the closed polygon `p` (even-odd).
pub fn inside(p: &[DVec2], q: DVec2) -> bool {
    let mut c = false;
    let k = p.len();
    for i in 0..k {
        let (a, b) = (p[i], p[(i + 1) % k]);
        if (a.y > q.y) != (b.y > q.y) && q.x < a.x + (q.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            c = !c;
        }
    }
    c
}

/// The direction the outline's straight sides mostly run along, modulo a
/// quarter turn (radians): the sides weighed by their length, in 2° bins.
fn main_dir(p: &[DVec2]) -> f64 {
    let q = std::f64::consts::FRAC_PI_2;
    let k = p.len();
    let mut bins = [0.0f64; 45];
    let segs: Vec<(f64, f64)> = (0..k)
        .filter_map(|i| {
            let d = p[(i + 1) % k] - p[i];
            let l = d.length();
            (l > 0.0).then(|| (d.y.atan2(d.x).rem_euclid(q), l))
        })
        .collect();
    for &(a, l) in &segs {
        let b = ((a / q * 45.0) as usize).min(44);
        bins[b] += l;
        bins[(b + 1) % 45] += l * 0.5;
        bins[(b + 44) % 45] += l * 0.5;
    }
    let peak = (0..45).max_by(|&a, &b| bins[a].partial_cmp(&bins[b]).unwrap()).unwrap_or(0);
    let a0 = (peak as f64 + 0.5) / 45.0 * q;
    // the mean of the sides near the peak (a quarter turn wraps round)
    let (mut s, mut w) = (0.0, 0.0);
    for &(a, l) in &segs {
        let d = (a - a0 + q * 0.5).rem_euclid(q) - q * 0.5;
        if d.abs() < 3f64.to_radians() {
            s += d * l;
            w += l;
        }
    }
    a0 + if w > 0.0 { s / w } else { 0.0 }
}

/// A flat face filled: a surface over a rectangle of its plane (lined up
/// with its sides), laid on the face's triangles (the parts raised or sunk
/// in it are not in them: passed over) with the fewest spans that follow
/// them, or the plane itself when it does not follow the scan; cut by
/// `outline` (the path made round it, a closed polyline).
pub fn fill(m: &Mesh, f: &Face, outline: &[DVec3], p: PatchParams, h: f64) -> Result<Patched, String> {
    if outline.len() < 3 {
        return Err("This face has no outline".into());
    }
    let (c, n) = (f.c, f.n);
    let u0 = n.any_orthonormal_vector();
    let v0 = n.cross(u0);
    let to2 = |x: DVec3, u: DVec3, v: DVec3| DVec2::new((x - c).dot(u), (x - c).dot(v));
    let a = main_dir(&outline.iter().map(|&x| to2(x, u0, v0)).collect::<Vec<_>>());
    let ex = u0 * a.cos() + v0 * a.sin();
    let ey = n.cross(ex);
    let flat: Vec<DVec2> = outline.iter().map(|&x| to2(x, ex, ey)).collect();
    let (lo, hi) = flat.iter().fold((DVec2::splat(f64::MAX), DVec2::splat(f64::MIN)), |(l, g), q| (l.min(*q), g.max(*q)));
    let size = (hi - lo).max_element();
    if size < h * 2.0 {
        return Err("This face is too small".into());
    }
    let pad = size * 0.02 + h;
    let (lo, hi) = (lo - DVec2::splat(pad), hi + DVec2::splat(pad));
    let wh = hi - lo;
    let to_uv = |q: DVec2| (q - lo) / wh;
    let mut trim: Vec<DVec2> = flat.iter().map(|&q| to_uv(q)).collect();
    if signed_area(&trim) < 0.0 {
        trim.reverse();
    }
    // the surface from its heights over the rectangle (control points over
    // their Greville points: the rectangle's lines stay straight and even)
    let make = |d: usize, su: usize, sv: usize, z: &[f64]| -> Surface {
        let (du, dv) = (d, d);
        let (ku, kv) = (uniform_knots(du, su), uniform_knots(dv, sv));
        let (nu, nv) = (su + du, sv + dv);
        let gu: Vec<f64> = (0..nu).map(|i| ku[i + 1..=i + du].iter().sum::<f64>() / du as f64).collect();
        let gv: Vec<f64> = (0..nv).map(|j| kv[j + 1..=j + dv].iter().sum::<f64>() / dv as f64).collect();
        let mut pts = vec![];
        for j in 0..nv {
            for i in 0..nu {
                pts.push(c + ex * (lo.x + wh.x * gu[i]) + ey * (lo.y + wh.y * gv[j]) + n * z[j * nu + i]);
            }
        }
        Surface { du, dv, ku, kv, nu, nv, pts, w: vec![1.0; nu * nv] }
    };
    // the face's triangles: where each is over the rectangle, how high, how much
    let step = (f.tris.len() / 40_000).max(1);
    let data: Vec<(f64, f64, f64, f64)> = f
        .tris
        .iter()
        .step_by(step)
        .map(|&t| {
            let q = m.tri_centre(t as usize);
            let uv = to_uv(to2(q, ex, ey));
            (uv.x, uv.y, (q - c).dot(n), m.tri_area(t as usize))
        })
        .collect();
    let mean_w = data.iter().map(|d| d.3).sum::<f64>() / data.len().max(1) as f64;
    let height = |s: &Surface, u: f64, v: f64| (s.eval(u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)) - c).dot(n);
    let sigma = (h * 1.5).max(0.1);
    let flat_corners = |s: &Surface| [s.eval(0.0, 0.0), s.eval(1.0, 0.0), s.eval(1.0, 1.0), s.eval(0.0, 1.0)];
    let passed = (f.holes_area / (signed_area(&trim).abs() * wh.x * wh.y).max(1e-9)).clamp(0.0, 1.0);
    let rms = |s: &Surface| {
        let (mut sq, mut w) = (0.0, 0.0);
        for &(u, v, z, a) in &data {
            sq += (height(s, u, v) - z).powi(2) * a;
            w += a;
        }
        (sq / w.max(1e-12)).sqrt()
    };
    if !p.follow {
        let s = make(1, 1, 1, &[0.0; 4]);
        let off = rms(&s);
        return Ok(Patched { corners: flat_corners(&s), surface: s, spans: [1, 1], off, gap: 0.0, passed, trim });
    }
    // one fit (Cauchy: the triangles far off weigh less), judged on its
    // capped distances
    let spans_for = |k: usize| -> (usize, usize) {
        let o = |r: f64| ((k as f64 * r).round() as usize).clamp(1, 12);
        if wh.x >= wh.y {
            (k, o(wh.y / wh.x))
        } else {
            (o(wh.x / wh.y), k)
        }
    };
    let fit_k = |su: usize, sv: usize| -> Option<(Surface, f64)> {
        let mut w: Vec<f64> = data.iter().map(|d| d.3 / mean_w).collect();
        let mut s = None;
        for it in 0..3 {
            let pts: Vec<(f64, f64, DVec3, f64)> = data.iter().zip(&w).map(|(&(u, v, z, _), &w)| (u, v, DVec3::new(0.0, 0.0, z), w)).collect();
            let lam = 0.001 * data.len() as f64 / ((su + 3) * (sv + 3)) as f64;
            let g = fit(&pts, su, sv, lam)?;
            let z: Vec<f64> = g.pts.iter().map(|q| q.z).collect();
            let sf = make(3, su, sv, &z);
            if it < 2 {
                for (k, &(u, v, zz, a)) in data.iter().enumerate() {
                    let r = (height(&sf, u, v) - zz) / sigma;
                    w[k] = a / mean_w / (1.0 + r * r);
                }
            }
            s = Some(sf);
        }
        let s = s?;
        let (mut sq, mut ws) = (0.0, 0.0);
        for &(u, v, z, a) in &data {
            sq += (height(&s, u, v) - z).abs().min(sigma * 1.5).powi(2) * a;
            ws += a;
        }
        Some((s, (sq / ws.max(1e-12)).sqrt()))
    };
    let ks: Vec<usize> = if p.spans > 0 { vec![p.spans.min(12)] } else { (1..=8).collect() };
    let mut tried: Vec<(Surface, f64)> = vec![];
    for k in ks {
        let (su, sv) = spans_for(k);
        if let Some(t) = fit_k(su, sv) {
            if std::env::var("RECTIFY_PATCH_DEBUG").is_ok() {
                eprintln!("face k {k} spans {su}x{sv} robust {:.4}", t.1);
            }
            tried.push(t);
        }
    }
    let best = tried.iter().map(|t| t.1).fold(f64::INFINITY, f64::min);
    let Some((s, _)) = tried.into_iter().find(|t| t.1 <= best * 1.2 + (h * 0.1).max(0.01)) else {
        return Err("No surface fits this face".into());
    };
    let off = rms(&s);
    let spans = [s.nu - s.du, s.nv - s.dv];
    Ok(Patched { corners: flat_corners(&s), surface: s, spans, off, gap: 0.0, passed, trim })
}

/// A polygon clipped to the box `lo`..`hi` (Sutherland–Hodgman).
fn clip_box(p: &[DVec2], lo: DVec2, hi: DVec2) -> Vec<DVec2> {
    let mut out = p.to_vec();
    for side in 0..4 {
        let keep = |q: DVec2| match side {
            0 => q.x >= lo.x,
            1 => q.x <= hi.x,
            2 => q.y >= lo.y,
            _ => q.y <= hi.y,
        };
        let cut = |a: DVec2, b: DVec2| {
            let t = match side {
                0 => (lo.x - a.x) / (b.x - a.x),
                1 => (hi.x - a.x) / (b.x - a.x),
                2 => (lo.y - a.y) / (b.y - a.y),
                _ => (hi.y - a.y) / (b.y - a.y),
            };
            a.lerp(b, t)
        };
        let inp = std::mem::take(&mut out);
        let k = inp.len();
        for i in 0..k {
            let (a, b) = (inp[(i + k - 1) % k], inp[i]);
            match (keep(a), keep(b)) {
                (true, true) => out.push(b),
                (true, false) => out.push(cut(a, b)),
                (false, true) => {
                    out.push(cut(a, b));
                    out.push(b);
                }
                _ => {}
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

/// A simple polygon (counter-clockwise) in triangles, ear by ear.
fn ears(p: &[DVec2]) -> Vec<[DVec2; 3]> {
    let mut v: Vec<DVec2> = vec![];
    for &q in p {
        if v.last().map(|l: &DVec2| (*l - q).length_squared() > 1e-18).unwrap_or(true) {
            v.push(q);
        }
    }
    while v.len() > 1 && (v[0] - v[v.len() - 1]).length_squared() <= 1e-18 {
        v.pop();
    }
    let mut out = vec![];
    let cross = |a: DVec2, b: DVec2, c: DVec2| (b - a).perp_dot(c - a);
    let mut guard = 0;
    while v.len() > 3 && guard < 10_000 {
        guard += 1;
        let k = v.len();
        let ear = (0..k).find(|&i| {
            let (a, b, c) = (v[(i + k - 1) % k], v[i], v[(i + 1) % k]);
            if cross(a, b, c) <= 1e-14 {
                return false;
            }
            !(0..k).any(|j| {
                let q = v[j];
                j != i && j != (i + 1) % k && j != (i + k - 1) % k && cross(a, b, q) >= 0.0 && cross(b, c, q) >= 0.0 && cross(c, a, q) >= 0.0 && q != a && q != b && q != c
            })
        });
        match ear {
            Some(i) => {
                out.push([v[(i + k - 1) % k], v[i], v[(i + 1) % k]]);
                v.remove(i);
            }
            None => {
                // what is left is degenerate: a flat sliver, or a bridge
                // along the box's side
                let i = (0..k).find(|&i| cross(v[(i + k - 1) % k], v[i], v[(i + 1) % k]).abs() <= 1e-14);
                match i {
                    Some(i) => {
                        v.remove(i);
                    }
                    None => break,
                }
            }
        }
    }
    if v.len() == 3 && cross(v[0], v[1], v[2]) > 0.0 {
        out.push([v[0], v[1], v[2]]);
    }
    out
}

/// The inside of a closed outline in (u, v) (counter-clockwise, in the unit
/// square) as triangles: an even grid of `cells` × `cells`, the cells the
/// outline crosses cut along it.
pub fn trim_triangles(trim: &[DVec2], cells: usize) -> Vec<[DVec2; 3]> {
    let n = cells.max(1);
    let mut crossed = vec![false; n * n];
    let cell = |x: f64| ((x * n as f64).floor() as isize).clamp(0, n as isize - 1) as usize;
    let k = trim.len();
    for i in 0..k {
        let (a, b) = (trim[i], trim[(i + 1) % k]);
        let (x0, x1) = (cell(a.x.min(b.x)), cell(a.x.max(b.x)));
        let (y0, y1) = (cell(a.y.min(b.y)), cell(a.y.max(b.y)));
        for y in y0..=y1 {
            for x in x0..=x1 {
                crossed[y * n + x] = true;
            }
        }
    }
    let mut out = vec![];
    let s = 1.0 / n as f64;
    for y in 0..n {
        for x in 0..n {
            let lo = DVec2::new(x as f64 * s, y as f64 * s);
            let hi = lo + DVec2::splat(s);
            if !crossed[y * n + x] {
                if inside(trim, (lo + hi) * 0.5) {
                    let (b, d) = (DVec2::new(hi.x, lo.y), DVec2::new(lo.x, hi.y));
                    out.push([lo, b, hi]);
                    out.push([lo, hi, d]);
                }
                continue;
            }
            let piece = clip_box(trim, lo, hi);
            if piece.len() >= 3 {
                out.extend(ears(&piece));
            }
        }
    }
    out
}

/// A line across (u, v) (`at(t)`, t from 0 to 1, `samples` steps) kept
/// where it is inside the outline: its runs.
pub fn trim_runs(trim: &[DVec2], at: impl Fn(f64) -> DVec2, samples: usize) -> Vec<Vec<DVec2>> {
    let mut out: Vec<Vec<DVec2>> = vec![];
    let mut cur: Vec<DVec2> = vec![];
    for k in 0..=samples {
        let q = at(k as f64 / samples as f64);
        if inside(trim, q) {
            cur.push(q);
        } else if !cur.is_empty() {
            if cur.len() >= 2 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat plate 20 × 12 on z = 0 with a raised square (a button) in it
    /// and walls going down round it.
    fn plate() -> Mesh {
        let (nx, ny) = (80, 48);
        let mut pos = vec![];
        let hgt = |x: f64, y: f64| {
            let edge = (x.abs() - 10.0).max(y.abs() - 6.0);
            let button = (x - 3.0).abs().max((y - 1.0).abs()) < 2.0;
            if edge > 0.0 {
                -edge * 3.0
            } else if button {
                1.5
            } else {
                0.0
            }
        };
        for j in 0..=ny {
            for i in 0..=nx {
                let (x, y) = (-12.0 + 24.0 * i as f64 / nx as f64, -8.0 + 16.0 * j as f64 / ny as f64);
                pos.push(DVec3::new(x, y, hgt(x, y)));
            }
        }
        let id = |i: usize, j: usize| (j * (nx + 1) + i) as u32;
        let mut tris = vec![];
        for j in 0..ny {
            for i in 0..nx {
                tris.push([id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
                tris.push([id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
            }
        }
        Mesh { pos, tris, unit_mm: 1.0, ..Default::default() }
    }

    #[test]
    fn the_plate_and_its_outline() {
        let m = plate();
        let topo = Topo::new(&m);
        let p = DVec3::new(-5.0, -2.0, 0.0);
        let t0 = (0..m.tris.len()).find(|&t| {
            let c = m.tri_centre(t);
            (c - p).length() < 0.3
        }).unwrap() as u32;
        let f = find(&m, &topo, t0, p, 0.3).unwrap();
        assert!(f.n.z > 0.99, "{:?}", f.n);
        assert_eq!(f.holes, 1);
        // about 20 × 12 less the button
        assert!(f.area > 200.0 && f.area < 240.0, "{}", f.area);
        let (x0, x1) = f.outline.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.x), a.1.max(p.x)));
        assert!((x0 + 10.0).abs() < 0.5 && (x1 - 10.0).abs() < 0.5, "{x0} {x1}");
        let (scr, pts, sharp) = as_stroke(&f.outline, f.n);
        assert_eq!(scr.len(), pts.len());
        assert!(sharp);
        let keys = crate::path::stroke_keys(&scr, 45.0, 12.0, 40.0);
        let corners = keys.iter().filter(|k| !k.1).count();
        // 4 corners, the start counted twice (it closes)
        assert_eq!(corners, 5, "{keys:?}");
        // the path: 4 corners, about 20 + 12 twice round
        let (curve, nc, len) = outline_path(&f, 0.3).unwrap();
        assert_eq!(nc, 4);
        assert!((len - 64.0).abs() < 3.0, "{len}");
        assert!((curve.eval(0.0) - curve.eval(1.0)).length() < 1e-9);
        // filled: flat, cut to the plate, the button passed over
        let line = crate::patch::polyline(&curve);
        let pt = fill(&m, &f, &line, PatchParams::default(), 0.3).unwrap();
        assert!(pt.off < 0.05, "{}", pt.off);
        assert!(pt.passed > 0.03 && pt.passed < 0.1, "{}", pt.passed);
        let s = &pt.surface;
        let (u0, u1, v0, v1) = (s.ku[s.du], s.ku[s.nu], s.kv[s.dv], s.kv[s.nv]);
        let area: f64 = trim_triangles(&pt.trim, 48)
            .iter()
            .map(|t| {
                let e = |q: DVec2| s.eval(u0 + (u1 - u0) * q.x, v0 + (v1 - v0) * q.y);
                (e(t[1]) - e(t[0])).cross(e(t[2]) - e(t[0])).length() * 0.5
            })
            .sum();
        assert!((area - 240.0).abs() < 12.0, "{area}");
    }

    #[test]
    fn a_square_in_triangles() {
        // an L: the unit square less its top right quarter
        let l = [DVec2::new(0.1, 0.1), DVec2::new(0.9, 0.1), DVec2::new(0.9, 0.5), DVec2::new(0.5, 0.5), DVec2::new(0.5, 0.9), DVec2::new(0.1, 0.9)];
        let a: f64 = trim_triangles(&l, 7).iter().map(|t| (t[1] - t[0]).perp_dot(t[2] - t[0]) * 0.5).sum();
        assert!((a - 0.48).abs() < 1e-9, "{a}");
        assert_eq!(trim_runs(&l, |t| DVec2::new(t, 0.7), 100).len(), 1);
    }
}
