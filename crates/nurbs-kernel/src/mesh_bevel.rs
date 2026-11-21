//! Maya's Bevel on a polygonal mesh (Romain, 20 nov. 2026: "travaille le
//! bevel comme Maya, avec chamfer et bevel, et surtout en travaillant bien
//! les coins").
//!
//! Each edge asked becomes a strip of `segments` rows of quads, `width`
//! from the edge on each of its two faces: flat rows (Chamfer) or rows on a
//! rounded profile (Bevel: a conic tangent to both faces, a quarter circle
//! on a square edge). What happens round each vertex of those edges (its
//! faces in order round it, the "fan") follows Forme SDF's Bake bevel of
//! 10 nov. (its rules from Romain's Correctif.obj) and Maya's results:
//!
//! - between two edges beveled with one face between them, the face's
//!   corner goes where the two offset lines cross (the mitre);
//! - an edge not beveled next to a beveled one takes the new point along it
//!   (the point slides on it, as Maya's); with one such edge between two
//!   beveled ones, both strips end on that one point;
//! - two beveled edges at a corner, with single points between them (the
//!   top edges of a box at its corners, a loop of edges round a cylinder):
//!   the two strips meet on one shared profile (a mitre, no extra face);
//! - three or more (a box's corner): a patch of quads round a centre, the
//!   profiles' halves as its sides, laid on the sphere through them when
//!   they lie on one (a sphere's eighth at a box's corner); an odd count of
//!   segments leaves a small polygon in the middle, as Maya's;
//! - one beveled edge ending at a corner of three faces: its profile goes
//!   into the third face (the end face gets the arc); with more faces there,
//!   the vertex stays and a polygon closes the strip's end;
//! - too wide, the width stops where two new points would cross along an
//!   edge (Maya's Clamp), and says so.

use crate::mesh::{edge_key, poly_normal};
use glam::Vec3;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BevelOptions {
    /// The distance from the edge on each face (mm).
    pub width: f32,
    /// Rows of quads across each strip (1 to 32).
    pub segments: u32,
    /// Flat rows (Maya's Chamfer on); else a rounded profile.
    pub chamfer: bool,
}

#[derive(Clone, Debug, Default)]
pub struct BevelResult {
    /// The new faces (the strips, the corners), by their index.
    pub faces: Vec<u32>,
    /// The width made (below the one asked when clamped).
    pub width: f32,
    pub clamped: bool,
    /// The edges beveled (border and non-manifold edges are left out).
    pub edges: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum It {
    /// A new point of this vertex's plan.
    L(usize),
    /// The vertex itself, kept.
    V,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Where two offset lines cross inside a face.
    Meet,
    /// On an edge not beveled (or square off the beveled one).
    Slide,
    /// Inside a profile or a corner patch.
    Inner,
}

/// What a vertex of the beveled edges becomes, in offsets from it for a
/// width of 1 (everything scales with the width about the vertex).
struct Plan {
    v: u32,
    loc: Vec<Vec3>,
    kind: Vec<Kind>,
    /// Per beveled edge (by the vertex at its other end): its profile here,
    /// from its point on the face that runs other → v (G_A) to its point on
    /// the other face.
    prof: HashMap<u32, Vec<usize>>,
    /// The face's corner at v becomes these.
    rep: Vec<(usize, Vec<It>)>,
    /// New polygons round the vertex.
    caps: Vec<Vec<It>>,
    /// How far along each edge (by its other end) the new points reach.
    reach: Vec<(u32, f32)>,
}

/// The faces round `v` in order: `o[k]` the edges' other ends, `f[k]` the
/// face between edge k and edge k + 1 (its corner o[k] → v → o[k + 1]).
struct Fan {
    o: Vec<u32>,
    f: Vec<usize>,
    closed: bool,
}

fn corner(face: &[u32], v: u32) -> Option<(u32, u32)> {
    let n = face.len();
    let i = face.iter().position(|&x| x == v)?;
    Some((face[(i + n - 1) % n], face[(i + 1) % n]))
}

fn fan_of(v: u32, faces: &[Vec<u32>], dir: &HashMap<(u32, u32), usize>, vf: &[Vec<usize>]) -> Option<Fan> {
    let fs = &vf[v as usize];
    if fs.is_empty() {
        return None;
    }
    // an open fan starts at the face whose first edge has no face beyond it
    let mut start = fs[0];
    for &fi in fs {
        let (p, _) = corner(&faces[fi], v)?;
        if !dir.contains_key(&(v, p)) {
            start = fi;
            break;
        }
    }
    let (p0, _) = corner(&faces[start], v)?;
    let mut o = vec![p0];
    let mut f = vec![];
    let mut cur = start;
    let closed;
    loop {
        let (_, nx) = corner(&faces[cur], v)?;
        f.push(cur);
        o.push(nx);
        match dir.get(&(nx, v)) {
            Some(&g) if g == start => {
                closed = true;
                o.pop();
                break;
            }
            Some(&g) => {
                if f.len() > fs.len() {
                    return None;
                }
                cur = g;
            }
            None => {
                closed = false;
                break;
            }
        }
    }
    // a vertex where two fans touch (not a manifold): left alone
    if f.len() != fs.len() || faces[start].iter().filter(|&&x| x == v).count() != 1 {
        return None;
    }
    Some(Fan { o, f, closed })
}

/// The rational quadratic from `a` to `b` through the control `k`, weight
/// `w`, at `t`.
fn conic(a: Vec3, k: Vec3, b: Vec3, w: f32, t: f32) -> Vec3 {
    let (b0, b1, b2) = ((1.0 - t) * (1.0 - t), 2.0 * t * (1.0 - t) * w, t * t);
    (a * b0 + k * b1 + b * b2) / (b0 + b1 + b2)
}

/// `n` - 1 points dividing the conic into `n` pieces of equal length.
fn conic_points(a: Vec3, k: Vec3, b: Vec3, w: f32, n: usize) -> Vec<Vec3> {
    const S: usize = 96;
    let pts: Vec<Vec3> = (0..=S).map(|i| conic(a, k, b, w, i as f32 / S as f32)).collect();
    let mut acc = vec![0.0f32];
    for i in 1..=S {
        acc.push(acc[i - 1] + (pts[i] - pts[i - 1]).length());
    }
    let total = acc[S].max(1e-9);
    let mut out = vec![];
    let mut j = 0;
    for i in 1..n {
        let want = total * i as f32 / n as f32;
        while j < S - 1 && acc[j + 1] < want {
            j += 1;
        }
        let span = (acc[j + 1] - acc[j]).max(1e-9);
        out.push(pts[j].lerp(pts[j + 1], ((want - acc[j]) / span).clamp(0.0, 1.0)));
    }
    out
}

/// The sphere through points (least squares): centre, radius, RMS error.
fn sphere_fit(p: &[Vec3]) -> Option<(Vec3, f32, f32)> {
    if p.len() < 4 {
        return None;
    }
    let mean = p.iter().sum::<Vec3>() / p.len() as f32;
    let mut m = [[0f64; 5]; 4];
    for q in p {
        let q = (*q - mean).as_dvec3();
        let row = [2.0 * q.x, 2.0 * q.y, 2.0 * q.z, 1.0];
        let y = q.length_squared();
        for i in 0..4 {
            for j in 0..4 {
                m[i][j] += row[i] * row[j];
            }
            m[i][4] += row[i] * y;
        }
    }
    // Gauss-Jordan with partial pivoting
    for c in 0..4 {
        let piv = (c..4).max_by(|a, b| m[*a][c].abs().total_cmp(&m[*b][c].abs()))?;
        if m[piv][c].abs() < 1e-12 {
            return None;
        }
        m.swap(c, piv);
        for r in 0..4 {
            if r != c {
                let k = m[r][c] / m[c][c];
                for j in c..5 {
                    m[r][j] -= k * m[c][j];
                }
            }
        }
    }
    let x: Vec<f64> = (0..4).map(|i| m[i][4] / m[i][i]).collect();
    let c = glam::DVec3::new(x[0], x[1], x[2]);
    let r2 = x[3] + c.length_squared();
    if r2 <= 0.0 {
        return None;
    }
    let r = r2.sqrt();
    let rms = (p.iter().map(|q| ((q.as_dvec3() - mean.as_dvec3() - c).length() - r).powi(2)).sum::<f64>() / p.len() as f64).sqrt();
    Some(((c + mean.as_dvec3()).as_vec3(), r as f32, rms as f32))
}

#[allow(clippy::too_many_arguments)]
fn plan_vertex(v: u32, pts: &[Vec3], faces: &[Vec<u32>], dir: &HashMap<(u32, u32), usize>, vf: &[Vec<usize>], bev: &HashSet<(u32, u32)>, n: usize, chamfer: bool) -> Option<Plan> {
    let fan = fan_of(v, faces, dir, vf)?;
    let m = fan.o.len();
    let pv = pts[v as usize];
    let b: Vec<bool> = fan.o.iter().map(|o| bev.contains(&edge_key(v, *o))).collect();
    let bl: Vec<usize> = (0..m).filter(|k| b[*k]).collect();
    let nb = bl.len();
    if nb == 0 {
        return None;
    }
    let d: Vec<Vec3> = fan.o.iter().map(|o| (pts[*o as usize] - pv).normalize_or_zero()).collect();
    let nf: Vec<Vec3> = fan.f.iter().map(|fi| poly_normal(pts, &faces[*fi]).normalize_or_zero()).collect();
    let nfaces = fan.f.len();
    // the face k's angle at v, inside it (more than π at a notch)
    let alpha = |k: usize| -> f32 {
        let (a, c) = (d[k % m], d[(k + 1) % m]);
        let mut x = c.cross(a).dot(nf[k % nfaces]).atan2(a.dot(c));
        if x < 0.0 {
            x += std::f32::consts::TAU;
        }
        x
    };
    let md = |x: usize| x % m;
    let mut plan = Plan { v, loc: vec![], kind: vec![], prof: HashMap::new(), rep: vec![], caps: vec![], reach: vec![] };
    let mut on_edge: Vec<Option<usize>> = vec![];
    let mut push = |plan: &mut Plan, p: Vec3, kind: Kind, edge: Option<usize>| -> usize {
        plan.loc.push(p);
        plan.kind.push(kind);
        on_edge.push(edge);
        plan.loc.len() - 1
    };
    // face k between beveled edges k and k + 1: where their offsets cross
    let meet = |k: usize| -> (Vec3, Kind) {
        let s = alpha(k).sin();
        if s.abs() > 0.15 {
            ((d[md(k)] + d[md(k + 1)]) / s, Kind::Meet)
        } else {
            (d[md(k)].cross(nf[k % nfaces]).normalize_or_zero(), Kind::Slide)
        }
    };
    // face k after beveled edge k: the offset meets edge k + 1
    let slide_after = |k: usize| -> Vec3 {
        let s = alpha(k).sin();
        if s > 0.15 {
            d[md(k + 1)] / s
        } else {
            d[md(k)].cross(nf[k % nfaces]).normalize_or_zero()
        }
    };
    // face k − 1 before beveled edge k: the offset meets edge k − 1
    let slide_before = |k: usize| -> Vec3 {
        let f = (k + m - 1) % m;
        let s = alpha(f).sin();
        if s > 0.15 {
            d[md(k + m - 1)] / s
        } else {
            nf[f % nfaces].cross(d[md(k)]).normalize_or_zero()
        }
    };
    let mut r_of: HashMap<usize, usize> = HashMap::new();
    let mut l_of: HashMap<usize, usize> = HashMap::new();
    // gaps between consecutive beveled edges: (a, b unwrapped, g)
    let mut gaps: Vec<(usize, usize, usize)> = vec![];
    let count = if fan.closed { nb } else { nb - 1 };
    for i in 0..count {
        let a = bl[i];
        let bb = if i + 1 < nb { bl[i + 1] } else { bl[0] + m };
        gaps.push((a, bb, bb - a));
    }
    for &(a, bb, g) in &gaps {
        match g {
            1 => {
                let (p, kd) = meet(a);
                let id = push(&mut plan, p, kd, None);
                r_of.insert(a, id);
                l_of.insert(md(bb), id);
            }
            2 => {
                let p = (slide_after(a) + slide_before(bb)) * 0.5;
                let id = push(&mut plan, p, Kind::Slide, Some(md(a + 1)));
                r_of.insert(a, id);
                l_of.insert(md(bb), id);
            }
            _ => {
                let r = push(&mut plan, slide_after(a), Kind::Slide, Some(md(a + 1)));
                let l = push(&mut plan, slide_before(bb), Kind::Slide, Some(md(bb + m - 1)));
                r_of.insert(a, r);
                l_of.insert(md(bb), l);
            }
        }
    }
    let (mut kept_before, mut kept_after) = (false, false);
    if !fan.closed {
        let (a0, a1) = (bl[0], bl[nb - 1]);
        // the border edges are never beveled: a0 ≥ 1, a1 ≤ m − 2
        let l = push(&mut plan, slide_before(a0), Kind::Slide, Some(a0 - 1));
        l_of.insert(a0, l);
        kept_before = a0 >= 2;
        let r = push(&mut plan, slide_after(a1), Kind::Slide, Some(a1 + 1));
        r_of.insert(a1, r);
        kept_after = a1 + 2 < m;
    }
    // the profiles: from the point on face k (R) to the one on face k − 1 (L)
    let mut prof: HashMap<usize, Vec<usize>> = HashMap::new();
    for &a in &bl {
        let (ri, li) = (r_of[&a], l_of[&a]);
        let (pa, pb) = (plan.loc[ri], plan.loc[li]);
        let mut ids = vec![ri];
        if n > 1 {
            let inner: Vec<Vec3> = if chamfer {
                (1..n).map(|i| pa.lerp(pb, i as f32 / n as f32)).collect()
            } else {
                let fa = a % nfaces;
                let fb = (a + m - 1) % m % nfaces;
                let cphi = nf[fa].dot(nf[fb]).clamp(-1.0, 1.0);
                let w = ((cphi.acos()) * 0.5).cos().max(0.05);
                let slide = plan.kind[ri] != Kind::Meet || plan.kind[li] != Kind::Meet;
                let t = if slide { 0.0 } else { (pa.dot(d[a]) + pb.dot(d[a])) * 0.5 };
                conic_points(pa, d[a] * t, pb, w, n)
            };
            for p in inner {
                ids.push(push(&mut plan, p, Kind::Inner, None));
            }
        }
        ids.push(li);
        prof.insert(a, ids);
    }
    let simple = fan.closed && gaps.iter().all(|g| g.2 <= 2);
    // two strips at a corner with single points between them: one profile
    if simple && nb == 2 && n > 1 {
        let (a, c) = (bl[0], bl[1]);
        let pa = prof[&a].clone();
        let mut pc = prof[&c].clone();
        for i in 1..n {
            let (ia, ic) = (pa[i], pc[n - i]);
            plan.loc[ia] = (plan.loc[ia] + plan.loc[ic]) * 0.5;
            pc[n - i] = ia;
        }
        prof.insert(c, pc);
    }
    // the faces' corners: the beveled edges before and after a face
    // (unwrapped round a closed fan)
    let mi = m as i64;
    let prev_bev = |k: usize| -> Option<i64> {
        for j in 0..mi {
            let x = k as i64 - j;
            if x < 0 && !fan.closed {
                return None;
            }
            if b[x.rem_euclid(mi) as usize] {
                return Some(x);
            }
        }
        None
    };
    let next_bev = |k: usize| -> Option<i64> {
        for x in (k as i64 + 1)..(k as i64 + 1 + mi) {
            if x >= mi && !fan.closed {
                return None;
            }
            if b[x.rem_euclid(mi) as usize] {
                return Some(x);
            }
        }
        None
    };
    let um = |x: i64| x.rem_euclid(mi) as usize;
    for k in 0..nfaces {
        let (bk, bk1) = (b[k], b[md(k + 1)]);
        let items = if bk {
            vec![It::L(r_of[&k])]
        } else if bk1 {
            vec![It::L(l_of[&md(k + 1)])]
        } else {
            let (a, bb) = (prev_bev(k), next_bev(k));
            let ki = k as i64;
            let mut it = vec![];
            if let Some(a) = a {
                if a + 1 == ki {
                    it.push(It::L(r_of[&um(a)]));
                }
            }
            let g = match (a, bb) {
                (Some(a), Some(bb)) => bb - a,
                _ => 0,
            };
            if nb == 1 && fan.closed && g == 3 {
                // its profile goes into this face (the end face gets the arc)
                for x in &prof[&um(a.unwrap())][1..] {
                    it.push(It::L(*x));
                }
            } else {
                let keep = match (a, bb) {
                    (Some(_), Some(_)) => g >= 4,
                    (None, Some(_)) => kept_before,
                    (Some(_), None) => kept_after,
                    _ => true,
                };
                if keep {
                    it.push(It::V);
                }
                if let Some(bb) = bb {
                    if bb == ki + 2 {
                        it.push(It::L(l_of[&um(bb)]));
                    }
                }
            }
            it
        };
        plan.rep.push((fan.f[k], items));
    }
    // the polygons round the vertex
    if simple && nb >= 3 && n >= 2 {
        corner_patch(&mut plan, &bl, &prof, n, chamfer);
    } else if !(simple && nb == 2) && !(fan.closed && nb == 1 && gaps[0].2 == 3) {
        // the loop round the vertex, in the fan's order, then turned
        let mut lp: Vec<It> = vec![];
        if kept_before {
            lp.push(It::V);
        }
        for (i, &a) in bl.iter().enumerate() {
            for x in prof[&a].iter().rev() {
                lp.push(It::L(*x));
            }
            let g = if i < gaps.len() { gaps[i].2 } else { 0 };
            if g >= 4 {
                lp.push(It::V);
            }
        }
        if kept_after && !kept_before {
            lp.push(It::V);
        }
        lp.dedup();
        if fan.closed && lp.len() > 1 && lp.first() == lp.last() {
            lp.pop();
        }
        let open_single = !fan.closed && nb == 1 && !kept_before && !kept_after;
        let distinct: HashSet<i64> = lp.iter().map(|x| match x { It::L(i) => *i as i64, It::V => -1 }).collect();
        if !open_single && distinct.len() >= 3 {
            lp.reverse();
            plan.caps.push(lp);
        }
    }
    for (k, p) in &prof {
        plan.prof.insert(fan.o[*k], p.clone());
    }
    // how far along each edge the new points reach
    for k in 0..m {
        let mut r = 0.0f32;
        if b[k] {
            r = r.max(plan.loc[r_of[&k]].dot(d[k])).max(plan.loc[l_of[&k]].dot(d[k]));
        }
        for (i, e) in on_edge.iter().enumerate() {
            if *e == Some(k) {
                r = r.max(plan.loc[i].dot(d[k]));
            }
        }
        if r > 0.0 {
            plan.reach.push((fan.o[k], r));
        }
    }
    Some(plan)
}

/// Three beveled edges or more round a vertex with single points between
/// them: quads round a centre (Maya's corner), on the sphere through the
/// profiles when they lie on one.
fn corner_patch(plan: &mut Plan, bl: &[usize], prof: &HashMap<usize, Vec<usize>>, n: usize, chamfer: bool) {
    let k = bl.len();
    // the profiles in the cap's own order (each from its point on face k to
    // the one on face k − 1, the fan taken backwards)
    let p: Vec<Vec<usize>> = (0..k).map(|j| prof[&bl[k - 1 - j]].clone()).collect();
    let h = n / 2;
    let odd = n % 2 == 1;
    let pos = |plan: &Plan, i: usize| plan.loc[i];
    let corners: Vec<Vec3> = (0..k).map(|j| pos(plan, p[j][0])).collect();
    let bound: Vec<Vec3> = p.iter().flat_map(|q| q[..n].iter().map(|i| pos(plan, *i))).collect();
    let sphere = if chamfer {
        None
    } else {
        sphere_fit(&bound).filter(|(_, r, rms)| *rms < 0.03 * *r && *r < 20.0)
    };
    let onto = |q: Vec3| -> Vec3 {
        match sphere {
            Some((c, r, _)) => c + (q - c).normalize_or_zero() * r,
            None => q,
        }
    };
    // the centre
    let mids: Vec<Vec3> = p
        .iter()
        .map(|q| if odd { (pos(plan, q[h]) + pos(plan, q[h + 1])) * 0.5 } else { pos(plan, q[h]) })
        .collect();
    let mmid = mids.iter().sum::<Vec3>() / k as f32;
    let mcor = corners.iter().sum::<Vec3>() / k as f32;
    let c = if chamfer {
        mcor
    } else if let Some((sc, r, _)) = sphere {
        sc + (mmid - sc).normalize_or_zero() * r
    } else {
        mmid + (mmid - mcor) * 0.77
    };
    let add = |plan: &mut Plan, q: Vec3| -> usize {
        plan.loc.push(q);
        plan.kind.push(Kind::Inner);
        plan.loc.len() - 1
    };
    // the inner points Q (all the centre when even)
    let qs: Vec<usize> = if odd {
        (0..k).map(|j| { let q = onto(c + (corners[j] - c) / n as f32); add(plan, q) }).collect()
    } else {
        let ci = add(plan, c);
        vec![ci; k]
    };
    // spokes: A from P_j[h] to Q_j, B from P_j[n − h] to Q_{j+1}
    let spoke = |plan: &mut Plan, from: usize, to: usize| -> Vec<usize> {
        let (a, bq) = (plan.loc[from], plan.loc[to]);
        let mut s = vec![from];
        for t in 1..h {
            let q = onto(a.lerp(bq, t as f32 / h as f32));
            s.push(add(plan, q));
        }
        s.push(to);
        s
    };
    let sa: Vec<Vec<usize>> = (0..k).map(|j| spoke(plan, p[j][h], qs[j])).collect();
    let sb: Vec<Vec<usize>> = if odd { (0..k).map(|j| spoke(plan, p[j][n - h], qs[(j + 1) % k])).collect() } else { sa.clone() };
    for j in 0..k {
        let jp = (j + k - 1) % k;
        // the grid of corner j: (a along P_j, b back along P_{j−1})
        let mut g = vec![vec![usize::MAX; h + 1]; h + 1];
        for a in 0..=h {
            g[a][0] = p[j][a];
            g[a][h] = sb[jp][a];
        }
        for bb in 0..=h {
            g[0][bb] = p[jp][n - bb];
            g[h][bb] = sa[j][bb];
        }
        for a in 1..h {
            for bb in 1..h {
                let (s, t) = (a as f32 / h as f32, bb as f32 / h as f32);
                let l = |i: usize| plan.loc[i];
                let q = l(g[0][bb]) * (1.0 - s) + l(g[h][bb]) * s + l(g[a][0]) * (1.0 - t) + l(g[a][h]) * t
                    - (l(g[0][0]) * (1.0 - s) * (1.0 - t) + l(g[h][0]) * s * (1.0 - t) + l(g[0][h]) * (1.0 - s) * t + l(g[h][h]) * s * t);
                g[a][bb] = add(plan, onto(q));
            }
        }
        for a in 0..h {
            for bb in 0..h {
                plan.caps.push(vec![It::L(g[a][bb]), It::L(g[a + 1][bb]), It::L(g[a + 1][bb + 1]), It::L(g[a][bb + 1])]);
            }
        }
        if odd {
            for bb in 0..h {
                plan.caps.push(vec![It::L(sa[j][bb]), It::L(sb[j][bb]), It::L(sb[j][bb + 1]), It::L(sa[j][bb + 1])]);
            }
        }
    }
    if odd {
        plan.caps.push(qs.iter().map(|q| It::L(*q)).collect());
    }
}

/// Bevels the edges (Maya's Bevel). The points and faces are changed in
/// place; the faces not touched keep their index, the new ones follow.
/// None when no edge can be beveled.
pub fn bevel(pts: &mut Vec<Vec3>, faces: &mut Vec<Vec<u32>>, edges: &[(u32, u32)], o: BevelOptions) -> Option<BevelResult> {
    let n = o.segments.clamp(1, 32) as usize;
    let mut dir: HashMap<(u32, u32), usize> = HashMap::new();
    let mut count: HashMap<(u32, u32), u32> = HashMap::new();
    let mut vf: Vec<Vec<usize>> = vec![vec![]; pts.len()];
    for (fi, f) in faces.iter().enumerate() {
        for i in 0..f.len() {
            let (a, b) = (f[i], f[(i + 1) % f.len()]);
            dir.insert((a, b), fi);
            *count.entry(edge_key(a, b)).or_default() += 1;
            if !vf[a as usize].contains(&fi) {
                vf[a as usize].push(fi);
            }
        }
    }
    // inner edges between two faces only
    let mut bev: HashSet<(u32, u32)> = edges
        .iter()
        .map(|e| edge_key(e.0, e.1))
        .filter(|e| e.0 != e.1 && count.get(e) == Some(&2) && dir.contains_key(&(e.0, e.1)) && dir.contains_key(&(e.1, e.0)))
        .collect();
    if bev.is_empty() || o.width <= 0.0 {
        return None;
    }
    // the plans; a vertex whose faces do not go round it once drops its edges
    let plans: Vec<Plan> = loop {
        let mut verts: Vec<u32> = bev.iter().flat_map(|e| [e.0, e.1]).collect();
        verts.sort();
        verts.dedup();
        let mut plans = vec![];
        let mut bad = vec![];
        for v in verts {
            match plan_vertex(v, pts, faces, &dir, &vf, &bev, n, o.chamfer) {
                Some(p) => plans.push(p),
                None => bad.push(v),
            }
        }
        if bad.is_empty() {
            break plans;
        }
        bev.retain(|e| !bad.contains(&e.0) && !bad.contains(&e.1));
        if bev.is_empty() {
            return None;
        }
    };
    // the clamp: two ends' points never cross along an edge
    let mut reach: HashMap<(u32, u32), f32> = HashMap::new();
    for p in &plans {
        for (other, r) in &p.reach {
            reach.insert((p.v, *other), *r);
        }
    }
    let mut lim = f32::MAX;
    for (&(a, b), &r) in &reach {
        let r2 = r + reach.get(&(b, a)).copied().unwrap_or(0.0);
        if r2 > 1e-6 {
            let len = (pts[a as usize] - pts[b as usize]).length();
            lim = lim.min(len * 0.98 / r2);
        }
    }
    let w = o.width.min(lim);
    if w < 1e-5 {
        return None;
    }
    // the new points
    let mut gid: Vec<Vec<u32>> = vec![];
    for p in &plans {
        let base = pts[p.v as usize];
        let ids: Vec<u32> = p.loc.iter().map(|q| {
            pts.push(base + *q * w);
            pts.len() as u32 - 1
        }).collect();
        gid.push(ids);
    }
    let map = |pi: usize, it: It| -> u32 {
        match it {
            It::L(i) => gid[pi][i],
            It::V => plans[pi].v,
        }
    };
    // the faces' corners replaced
    let mut rep: HashMap<(usize, u32), Vec<u32>> = HashMap::new();
    for (pi, p) in plans.iter().enumerate() {
        for (fi, items) in &p.rep {
            rep.insert((*fi, p.v), items.iter().map(|it| map(pi, *it)).collect());
        }
    }
    let mut out: Vec<(Vec<u32>, bool)> = vec![];
    for (fi, f) in faces.iter().enumerate() {
        let mut nf = vec![];
        for &c in f {
            match rep.get(&(fi, c)) {
                Some(r) => nf.extend_from_slice(r),
                None => nf.push(c),
            }
        }
        out.push((nf, false));
    }
    // the strips
    let pidx: HashMap<u32, usize> = plans.iter().enumerate().map(|(i, p)| (p.v, i)).collect();
    let mut bl: Vec<(u32, u32)> = bev.iter().copied().collect();
    bl.sort();
    for (a, b) in bl {
        // G_A runs a → b: at b its profile as kept (from G_A's side), at a
        // the other way round
        let (Some(&pa), Some(&pb)) = (pidx.get(&a), pidx.get(&b)) else { continue };
        let (Some(at_b), Some(at_a)) = (plans[pb].prof.get(&a), plans[pa].prof.get(&b)) else { continue };
        let pv: Vec<u32> = at_b.iter().map(|i| gid[pb][*i]).collect();
        let pu: Vec<u32> = at_a.iter().rev().map(|i| gid[pa][*i]).collect();
        for i in 0..n {
            out.push((vec![pv[i], pu[i], pu[i + 1], pv[i + 1]], true));
        }
    }
    for (pi, p) in plans.iter().enumerate() {
        for cap in &p.caps {
            out.push((cap.iter().map(|it| map(pi, *it)).collect(), true));
        }
    }
    // tidy: repeated corners out, slivers dropped
    faces.clear();
    let mut made = vec![];
    for (mut f, new) in out {
        f.dedup();
        while f.len() > 1 && f.first() == f.last() {
            f.pop();
        }
        if f.len() < 3 {
            continue;
        }
        if new {
            made.push(faces.len() as u32);
        }
        faces.push(f);
    }
    compact_points(pts, faces);
    Some(BevelResult { faces: made, width: w, clamped: w < o.width - 1e-5, edges: bev.len() })
}

/// Drops the points no face uses (indices renumbered).
pub fn compact_points(pts: &mut Vec<Vec3>, faces: &mut [Vec<u32>]) {
    let mut used = vec![false; pts.len()];
    for f in faces.iter() {
        for &v in f {
            used[v as usize] = true;
        }
    }
    let mut map = vec![u32::MAX; pts.len()];
    let mut np = vec![];
    for (i, u) in used.iter().enumerate() {
        if *u {
            map[i] = np.len() as u32;
            np.push(pts[i]);
        }
    }
    for f in faces.iter_mut() {
        for v in f.iter_mut() {
            *v = map[*v as usize];
        }
    }
    *pts = np;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{extrude_faces, poly_centre, poly_cube, poly_cylinder, poly_plane};

    fn closed(faces: &[Vec<u32>]) -> bool {
        let mut c: HashMap<(u32, u32), i32> = HashMap::new();
        let mut d: HashSet<(u32, u32)> = HashSet::new();
        for f in faces {
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                // each directed edge once (consistent orientation)
                if !d.insert((a, b)) {
                    return false;
                }
                *c.entry(edge_key(a, b)).or_default() += 1;
            }
        }
        c.values().all(|n| *n == 2)
    }

    fn outward(pts: &[Vec3], faces: &[Vec<u32>]) -> bool {
        let c = pts.iter().sum::<Vec3>() / pts.len() as f32;
        faces.iter().all(|f| poly_normal(pts, f).dot(poly_centre(pts, f) - c) > 0.0)
    }

    fn all_edges(faces: &[Vec<u32>]) -> Vec<(u32, u32)> {
        let mut s: Vec<(u32, u32)> = faces.iter().flat_map(|f| (0..f.len()).map(move |i| edge_key(f[i], f[(i + 1) % f.len()]))).collect();
        s.sort();
        s.dedup();
        s
    }

    fn euler(pts: &[Vec3], faces: &[Vec<u32>]) -> i64 {
        pts.len() as i64 - all_edges(faces).len() as i64 + faces.len() as i64
    }

    #[test]
    fn cube_all_edges_corners() {
        for (n, chamfer) in [(1, true), (2, false), (3, false), (4, false), (3, true)] {
            let (mut p, mut f) = poly_cube(20.0);
            let e = all_edges(&f);
            let r = bevel(&mut p, &mut f, &e, BevelOptions { width: 2.0, segments: n, chamfer }).unwrap();
            assert!(closed(&f), "n {n}");
            assert!(outward(&p, &f), "n {n}");
            assert_eq!(euler(&p, &f), 2);
            assert!(!r.clamped);
            // 6 faces, 12 strips of n quads, 8 corners
            let corner = if n == 1 { 1 } else if n % 2 == 0 { 3 * (n / 2) * (n / 2) } else { 3 * (n / 2) * (n / 2) + 3 * (n / 2) + 1 };
            assert_eq!(f.len() as u32, 6 + 12 * n + 8 * corner as u32, "n {n}");
            if !chamfer {
                // the corners on the sphere of radius 2 round (8, 18, 8)
                let c = Vec3::new(8.0, 18.0, 8.0);
                for q in &p {
                    if q.x > 8.0 && q.y > 18.0 && q.z > 8.0 {
                        assert!(((*q - c).length() - 2.0).abs() < 0.02, "{q:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn cube_top_edges_mitred() {
        for n in [1, 2, 3] {
            let (mut p, mut f) = poly_cube(20.0);
            let top = &f[3].clone();
            let e: Vec<(u32, u32)> = (0..4).map(|i| edge_key(top[i], top[(i + 1) % 4])).collect();
            bevel(&mut p, &mut f, &e, BevelOptions { width: 3.0, segments: n, chamfer: false }).unwrap();
            assert!(closed(&f) && outward(&p, &f));
            // no corner faces: 6 + 4 strips
            assert_eq!(f.len() as u32, 6 + 4 * n);
            assert!(f.iter().all(|q| q.len() == 4));
        }
    }

    #[test]
    fn one_edge_and_two() {
        for n in [1, 2, 4] {
            let (mut p, mut f) = poly_cube(20.0);
            // a vertical edge: its profile goes into the top and bottom faces
            bevel(&mut p, &mut f, &[(0, 2)], BevelOptions { width: 4.0, segments: n, chamfer: false }).unwrap();
            assert!(closed(&f) && outward(&p, &f));
            assert_eq!(f.len() as u32, 6 + n);
            // two edges of one face meeting at a corner
            let (mut p, mut f) = poly_cube(20.0);
            bevel(&mut p, &mut f, &[(0, 2), (2, 3)], BevelOptions { width: 4.0, segments: n, chamfer: false }).unwrap();
            assert!(closed(&f) && outward(&p, &f), "n {n}");
            assert_eq!(euler(&p, &f), 2);
        }
    }

    #[test]
    fn clamped_and_other_shapes() {
        let (mut p, mut f) = poly_cube(20.0);
        let e = all_edges(&f);
        let r = bevel(&mut p, &mut f, &e, BevelOptions { width: 30.0, segments: 2, chamfer: false }).unwrap();
        assert!(r.clamped && r.width < 10.0);
        assert!(closed(&f));
        // a cylinder, all its edges and only its rims
        let (mut p, mut f) = poly_cylinder(20.0, 16);
        let e = all_edges(&f);
        bevel(&mut p, &mut f, &e, BevelOptions { width: 1.0, segments: 3, chamfer: false }).unwrap();
        assert!(closed(&f) && outward(&p, &f));
        assert_eq!(euler(&p, &f), 2);
        let (mut p, mut f) = poly_cylinder(20.0, 16);
        let rims: Vec<(u32, u32)> = (0..16u32).flat_map(|k| [edge_key(k, (k + 1) % 16), edge_key(16 + k, 16 + (k + 1) % 16)]).collect();
        bevel(&mut p, &mut f, &rims, BevelOptions { width: 2.0, segments: 2, chamfer: false }).unwrap();
        assert!(closed(&f) && outward(&p, &f));
        assert_eq!(f.len(), 18 + 32 * 2);
        // a pocket in the top: concave edges round its rim
        let (mut p, mut f) = poly_cube(20.0);
        crate::mesh::inset_faces(&mut p, &mut f, &[3], 0.3);
        extrude_faces(&mut p, &mut f, &[3], -5.0);
        let e = all_edges(&f);
        bevel(&mut p, &mut f, &e, BevelOptions { width: 1.0, segments: 2, chamfer: false }).unwrap();
        assert!(closed(&f));
        assert_eq!(euler(&p, &f), 2);
        // a plane: an inner edge running to the border
        let (mut p, mut f) = poly_plane(20.0, 4);
        let r = bevel(&mut p, &mut f, &[(1, 6), (6, 11)], BevelOptions { width: 1.0, segments: 2, chamfer: true });
        assert!(r.is_some());
        let mut c: HashMap<(u32, u32), i32> = HashMap::new();
        for q in &f {
            for i in 0..q.len() {
                *c.entry(edge_key(q[i], q[(i + 1) % q.len()])).or_default() += 1;
            }
        }
        assert!(c.values().all(|n| *n <= 2));
    }
}
