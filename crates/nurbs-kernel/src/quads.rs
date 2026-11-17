//! The quad converter (Romain, 17 nov. 2026: "un premier convertisseur
//! polygonal sur un objet simple avec fillet", then "continue de pousser"):
//! the B-rep solid as quads with a clean flow, watertight.
//!
//! Each face is cut into four-sided patches by straight cuts in its own
//! (u, v) coordinates, scaled to about millimetres (Romain's rules from
//! Forme SDF's Bake: straight rays, one patch per rounded corner, a cut
//! stops in a T on the first line it meets):
//! - a hole is joined to the outline by two straight rays, left and right;
//! - an inner (concave) corner sends a ray along one of its sides;
//! - where a straight side runs into a rounded corner, a ray goes straight
//!   in, so each rounded corner is its own patch;
//! - a region with more than four corners is cut between two of its corners
//!   or the middles of its sides, choosing the cut that leaves four-cornered
//!   regions with corners closest to right angles;
//! - a region with fewer than four corners gets corners in the middle of its
//!   longest runs.
//!
//! Every piece of edge or cut gets a number of segments (by length, a
//! fillet's arc by angle, like a bevel's rows), raised until opposite sides
//! of every patch have the same count. Points along edges are shared by the
//! faces on them, so the mesh is closed; inside each patch, a Coons grid in
//! (u, v), put on the surface (a fillet stays round).

use forme_occt::{QuadTopo, Shape};
use std::collections::{HashMap, HashSet};
use std::f64::consts::{FRAC_PI_2, PI};

#[derive(Clone, Debug, Default)]
pub struct QuadMesh {
    pub positions: Vec<[f64; 3]>,
    pub quads: Vec<[u32; 4]>,
    /// The face (index in the shape's face map) of each quad.
    pub quad_face: Vec<u32>,
    /// Faces left out, and why.
    pub skipped: Vec<(usize, String)>,
}

/// Turning angle above which a point of an outline is a corner.
const CORNER: f64 = 25.0 * PI / 180.0;
/// A fillet's arc: one segment every this many radians (90° → 4 rows).
const ARC_STEP: f64 = PI / 8.0;
/// A side turning less than this is straight; more (to the inside), a
/// rounded corner.
const STRAIGHT: f64 = 2.0 * PI / 180.0;
const ROUNDED: f64 = 10.0 * PI / 180.0;
/// Two fractions of a line closer than this are the same place.
const EPS_F: f64 = 1e-7;

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

type V2 = [f64; 2];

fn s2(a: V2, b: V2) -> V2 {
    [a[0] - b[0], a[1] - b[1]]
}
fn a2(a: V2, b: V2) -> V2 {
    [a[0] + b[0], a[1] + b[1]]
}
fn k2(a: V2, k: f64) -> V2 {
    [a[0] * k, a[1] * k]
}
fn d2(a: V2, b: V2) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn x2(a: V2, b: V2) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn l2(a: V2) -> f64 {
    d2(a, a).sqrt()
}
fn u2(a: V2) -> V2 {
    let l = l2(a);
    if l < 1e-30 {
        [0.0, 0.0]
    } else {
        [a[0] / l, a[1] / l]
    }
}
fn lerp2(a: V2, b: V2, t: f64) -> V2 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}
/// The direction a quarter turn to the left (the inside of a loop that runs
/// counter-clockwise).
fn left2(a: V2) -> V2 {
    [-a[1], a[0]]
}
/// Signed angle from a to b (positive: to the left).
fn turn(a: V2, b: V2) -> f64 {
    x2(a, b).atan2(d2(a, b))
}

/// An edge's length along its samples, and the parameter at a fraction of
/// that length.
struct Arc {
    cum: Vec<f64>,
    t: [f64; 2],
}

impl Arc {
    fn new(pts: &[[f64; 3]], t: [f64; 2]) -> Arc {
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            cum.push(cum.last().unwrap() + len(sub(w[1], w[0])));
        }
        Arc { cum, t }
    }
    fn length(&self) -> f64 {
        *self.cum.last().unwrap()
    }
    /// The parameter at fraction `f` of the length (0 at t0, 1 at t1).
    fn param(&self, f: f64) -> f64 {
        let total = self.length();
        let n = self.cum.len() - 1;
        let u = if total <= 1e-12 {
            f
        } else {
            let want = f.clamp(0.0, 1.0) * total;
            let k = self.cum.partition_point(|c| *c < want).clamp(1, n);
            let (a, b) = (self.cum[k - 1], self.cum[k]);
            let s = if b - a > 1e-12 { (want - a) / (b - a) } else { 0.0 };
            ((k - 1) as f64 + s) / n as f64
        };
        self.t[0] + (self.t[1] - self.t[0]) * u
    }
}

/// A line the mesh follows: a B-rep edge, or a straight cut in one face's
/// (u, v). Positions along it are fractions of its length (an edge: in its
/// natural direction).
struct Line {
    /// The B-rep edge (1-based), or 0 for a cut.
    edge: i32,
    /// A cut: its ends in its face's (u, v), and its length in space.
    uv: [V2; 2],
    length: f64,
    /// The mesh nodes on it: (fraction, node), sorted, 0 and 1 included.
    splits: Vec<(f64, usize)>,
}

/// A stretch of a line, from fraction f0 to f1 (f1 < f0: travelled backwards).
#[derive(Clone, Copy, Debug)]
struct Span {
    line: usize,
    f0: f64,
    f1: f64,
    /// An edge: which of its curves on the face (a seam has two), as
    /// OpenCascade orients it there.
    pc: bool,
}

impl Span {
    fn rev(&self) -> bool {
        self.f1 < self.f0
    }
    fn range(&self) -> (f64, f64) {
        (self.f0.min(self.f1), self.f0.max(self.f1))
    }
}

/// A place on a loop: on span `i`, at fraction `f` of its line (the end of
/// the span: the junction with the next one).
#[derive(Clone, Copy, Debug)]
struct LPos {
    i: usize,
    f: f64,
}

/// A face's (u, v) scaled to about millimetres; `su` is negative when that
/// is needed for its outline to run counter-clockwise.
#[derive(Clone, Copy)]
struct FMap {
    su: f64,
    sv: f64,
}

impl FMap {
    fn to(&self, uv: V2) -> V2 {
        [uv[0] * self.su, uv[1] * self.sv]
    }
}

/// A loop's points in the map, span by span, with their line fractions, in
/// travel order.
type Geo = Vec<Vec<(f64, V2)>>;

fn tan_start(p: &[(f64, V2)]) -> V2 {
    u2(s2(p[1].1, p[0].1))
}
fn tan_end(p: &[(f64, V2)]) -> V2 {
    let n = p.len();
    u2(s2(p[n - 1].1, p[n - 2].1))
}
/// How much a span turns along its length (positive: to the left).
fn span_turn(p: &[(f64, V2)]) -> f64 {
    let mut t = 0.0;
    for w in p.windows(3) {
        let (a, b) = (s2(w[1].1, w[0].1), s2(w[2].1, w[1].1));
        if l2(a) > 1e-12 && l2(b) > 1e-12 {
            t += turn(a, b);
        }
    }
    t
}
fn span_len(p: &[(f64, V2)]) -> f64 {
    p.windows(2).map(|w| l2(s2(w[1].1, w[0].1))).sum()
}
/// The point and the direction at fraction `f` of a span's line.
fn on_span(p: &[(f64, V2)], f: f64) -> (V2, V2) {
    for w in p.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (lo, hi) = (a.0.min(b.0), a.0.max(b.0));
        if f >= lo - EPS_F && f <= hi + EPS_F {
            let s = if (b.0 - a.0).abs() > 1e-15 { ((f - a.0) / (b.0 - a.0)).clamp(0.0, 1.0) } else { 0.0 };
            return (lerp2(a.1, b.1, s), u2(s2(b.1, a.1)));
        }
    }
    (p[p.len() - 1].1, tan_end(p))
}
/// The line fraction at fraction `s` of a span's length.
fn frac_at_length(p: &[(f64, V2)], s: f64) -> f64 {
    let total = span_len(p);
    let want = s.clamp(0.0, 1.0) * total;
    let mut acc = 0.0;
    for w in p.windows(2) {
        let l = l2(s2(w[1].1, w[0].1));
        if acc + l >= want && l > 1e-15 {
            return w[0].0 + (w[1].0 - w[0].0) * ((want - acc) / l);
        }
        acc += l;
    }
    p[p.len() - 1].0
}
fn loop_points(g: &Geo) -> Vec<V2> {
    g.iter().flat_map(|p| p.iter().skip(1).map(|q| q.1)).collect()
}
fn signed_area(pts: &[V2]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| x2(pts[i], pts[(i + 1) % n])).sum::<f64>() * 0.5
}
fn inside(g: &Geo, p: V2) -> bool {
    let pts = loop_points(g);
    let n = pts.len();
    let mut c = false;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if x > p[0] {
                c = !c;
            }
        }
    }
    c
}
/// The chord a → b crosses the loop somewhere between its ends.
fn crosses(g: &Geo, a: V2, b: V2) -> bool {
    let d = s2(b, a);
    for p in g {
        for w in p.windows(2) {
            let (c, e) = (w[0].1, s2(w[1].1, w[0].1));
            let den = x2(d, e);
            let ca = s2(c, a);
            if den.abs() < 1e-14 {
                // parallel: overlapping is crossing
                if x2(ca, d).abs() < 1e-9 * (1.0 + l2(d)) {
                    let t0 = d2(ca, d) / d2(d, d);
                    let t1 = d2(s2(w[1].1, a), d) / d2(d, d);
                    if t0.max(t1) > 1e-6 && t0.min(t1) < 1.0 - 1e-6 {
                        return true;
                    }
                }
                continue;
            }
            let t = x2(ca, e) / den;
            let s = x2(ca, d) / den;
            if t > 1e-6 && t < 1.0 - 1e-6 && s > -1e-9 && s < 1.0 + 1e-9 {
                return true;
            }
        }
    }
    false
}
/// The chord a → b keeps away from the loop by `gap`, except close to its
/// ends (where it meets the loop).
fn clear(g: &Geo, a: V2, b: V2, gap: f64) -> bool {
    let d = s2(b, a);
    let l = l2(d);
    if l < 1e-12 {
        return false;
    }
    let near = (4.0 * gap).min(0.25 * l);
    for p in g.iter().flatten() {
        let q = p.1;
        if l2(s2(q, a)) < near || l2(s2(q, b)) < near {
            continue;
        }
        let t = (d2(s2(q, a), d) / (l * l)).clamp(0.0, 1.0);
        if l2(s2(q, lerp2(a, b, t))) < gap {
            return false;
        }
    }
    true
}

/// The first crossing of a ray with a loop, from its inside: the place, the
/// point, the distance.
fn cast(g: &Geo, o: V2, d: V2, tmin: f64) -> Option<(LPos, V2, f64)> {
    let mut best: Option<(LPos, V2, f64)> = None;
    for (i, p) in g.iter().enumerate() {
        for w in p.windows(2) {
            let (a, b) = (w[0].1, w[1].1);
            let e = s2(b, a);
            let den = x2(d, e);
            // only where the ray leaves the inside (on the left of the loop)
            if den.abs() < 1e-14 || x2(e, d) >= 0.0 {
                continue;
            }
            let ao = s2(a, o);
            let t = x2(ao, e) / den;
            let s = x2(ao, d) / den;
            if t > tmin && (-1e-9..=1.0 + 1e-9).contains(&s) && best.map(|b| t < b.2).unwrap_or(true) {
                let f = w[0].0 + (w[1].0 - w[0].0) * s.clamp(0.0, 1.0);
                best = Some((LPos { i, f }, a2(o, k2(d, t)), t));
            }
        }
    }
    best
}

/// A piece of a line: between its splits j and j + 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Piece {
    line: usize,
    j: usize,
}

/// Where a point of the mesh lies, to share it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key {
    Node(usize),
    Inner(Piece, usize),
}

/// One four-sided patch: its face and its sides (pieces, travelled
/// backwards or not), from its first corner round.
struct Patch {
    face: usize,
    sides: [Vec<(Piece, bool, bool)>; 4],
    /// Sides on a fillet's end (an arc of under 100°): their rows may end
    /// in T inside the patch.
    fillet: [bool; 4],
}

struct Ctx<'a> {
    shape: &'a Shape,
    topo: &'a QuadTopo,
    arcs: Vec<Arc>,
    lines: Vec<Line>,
    nodes: usize,
    /// Each edge on a face, sampled: (fraction, uv), natural direction.
    polys: HashMap<(usize, i32, bool), Vec<(f64, V2)>>,
    target: f64,
}

impl Ctx<'_> {
    /// The node at fraction `f` of a line (made if there is none there).
    fn node_at(&mut self, line: usize, f: f64) -> (f64, usize) {
        let s = &mut self.lines[line].splits;
        if let Some(&(g, n)) = s.iter().find(|(g, _)| (g - f).abs() < EPS_F) {
            return (g, n);
        }
        let n = self.nodes;
        self.nodes += 1;
        let k = s.partition_point(|(g, _)| *g < f);
        s.insert(k, (f, n));
        (f, n)
    }

    /// The direction of travel along a span at a line fraction, from the
    /// curve itself (in the map).
    fn span_tan(&mut self, face: usize, map: FMap, s: Span, f: f64) -> Result<V2, String> {
        let (lo, hi) = s.range();
        let d = 1e-4 * (hi - lo);
        let (fa, fb) = if s.rev() { ((f + d).min(hi), (f - d).max(lo)) } else { ((f - d).max(lo), (f + d).min(hi)) };
        let ua = self.uv_at(face, s.line, fa, s.pc)?;
        let ub = self.uv_at(face, s.line, fb, s.pc)?;
        Ok(u2(s2(map.to(ub), map.to(ua))))
    }

    fn node_of(&self, line: usize, f: f64) -> Option<usize> {
        self.lines[line].splits.iter().find(|(g, _)| (g - f).abs() < EPS_F).map(|x| x.1)
    }

    fn poly(&mut self, face: usize, edge: i32, rev: bool) -> Result<Vec<(f64, V2)>, String> {
        let key = (face, edge, rev);
        if !self.polys.contains_key(&key) {
            let fr: Vec<f64> = (0..=64).map(|i| i as f64 / 64.0).collect();
            let arc = &self.arcs[(edge - 1) as usize];
            let ts: Vec<f64> = fr.iter().map(|f| arc.param(*f)).collect();
            let (_, uv) = self.shape.edge_on_face(face, edge, rev, &ts)?;
            self.polys.insert(key, fr.into_iter().zip(uv).collect());
        }
        Ok(self.polys[&key].clone())
    }

    /// The (u, v) on face `face` of a line's point (an edge: on its curve
    /// there, by its orientation).
    fn uv_at(&mut self, face: usize, line: usize, f: f64, rev: bool) -> Result<V2, String> {
        let l = &self.lines[line];
        if l.edge == 0 {
            return Ok(lerp2(l.uv[0], l.uv[1], f));
        }
        let e = l.edge;
        let t = self.arcs[(e - 1) as usize].param(f);
        let (_, uv) = self.shape.edge_on_face(face, e, rev, &[t])?;
        Ok(uv[0])
    }

    fn span_geo(&mut self, face: usize, map: FMap, s: Span) -> Result<Vec<(f64, V2)>, String> {
        let (lo, hi) = s.range();
        let l = &self.lines[s.line];
        let mut out: Vec<(f64, V2)> = if l.edge == 0 {
            let (a, b) = (l.uv[0], l.uv[1]);
            vec![(lo, map.to(lerp2(a, b, lo))), (hi, map.to(lerp2(a, b, hi)))]
        } else {
            let p = self.poly(face, l.edge, s.pc)?;
            let at = |f: f64| -> V2 {
                let k = p.partition_point(|q| q.0 < f).clamp(1, p.len() - 1);
                let (a, b) = (p[k - 1], p[k]);
                let t = if b.0 - a.0 > 1e-15 { ((f - a.0) / (b.0 - a.0)).clamp(0.0, 1.0) } else { 0.0 };
                lerp2(a.1, b.1, t)
            };
            let mut v = vec![(lo, map.to(at(lo)))];
            v.extend(p.iter().filter(|q| q.0 > lo + EPS_F && q.0 < hi - EPS_F).map(|q| (q.0, map.to(q.1))));
            v.push((hi, map.to(at(hi))));
            v
        };
        if s.rev() {
            out.reverse();
        }
        Ok(out)
    }

    fn geo(&mut self, face: usize, map: FMap, lp: &[Span]) -> Result<Geo, String> {
        lp.iter().map(|s| self.span_geo(face, map, *s)).collect()
    }

    /// A straight cut from node a to node b, in (u, v).
    fn new_cut(&mut self, face: usize, a: (V2, usize), b: (V2, usize)) -> Result<usize, String> {
        if a.1 == b.1 {
            return Err("a cut of no length".into());
        }
        let pts: Vec<V2> = (0..=8).map(|i| lerp2(a.0, b.0, i as f64 / 8.0)).collect();
        let ev = self.shape.face_eval(face, &pts)?;
        let length = ev.windows(2).map(|w| len(sub(w[1].0, w[0].0))).sum();
        self.lines.push(Line { edge: 0, uv: [a.0, b.0], length, splits: vec![(0.0, a.1), (1.0, b.1)] });
        Ok(self.lines.len() - 1)
    }

    /// The loop's corners: (junction after span k, its turn); a node in
    /// `forced` is a corner even where the outline runs on smoothly.
    fn corners(&self, g: &Geo, lp: &[Span], forced: &HashSet<usize>) -> Vec<(usize, f64)> {
        let n = lp.len();
        (0..n)
            .filter_map(|k| {
                let t = turn(tan_end(&g[k]), tan_start(&g[(k + 1) % n]));
                let node = self.node_of(lp[k].line, lp[k].f1);
                (t.abs() > CORNER || node.map(|x| forced.contains(&x)).unwrap_or(false)).then_some((k, t))
            })
            .collect()
    }

    /// Makes a place a junction of the loop (its span split there, with a
    /// node on the line): the junction (after span k), and the span split.
    fn cut_at(&mut self, lp: &mut Vec<Span>, p: LPos) -> (usize, Option<usize>) {
        let s = lp[p.i];
        let n = lp.len();
        if (p.f - s.f1).abs() < EPS_F {
            return (p.i, None);
        }
        if (p.f - s.f0).abs() < EPS_F {
            return ((p.i + n - 1) % n, None);
        }
        let (f, _) = self.node_at(s.line, p.f);
        if (f - s.f1).abs() < EPS_F {
            return (p.i, None);
        }
        if (f - s.f0).abs() < EPS_F {
            return ((p.i + n - 1) % n, None);
        }
        lp[p.i] = Span { f1: f, ..s };
        lp.insert(p.i + 1, Span { f0: f, ..s });
        (p.i, Some(p.i))
    }

    /// Both places made junctions of the same loop.
    fn cut_both(&mut self, lp: &mut Vec<Span>, pa: LPos, pb: LPos) -> (usize, usize) {
        let along = |lp: &Vec<Span>, p: LPos| {
            let s = lp[p.i];
            if (s.f1 - s.f0).abs() > 1e-15 {
                (p.f - s.f0) / (s.f1 - s.f0)
            } else {
                0.0
            }
        };
        let a_later = pa.i > pb.i || (pa.i == pb.i && along(lp, pa) > along(lp, pb));
        let (first, second) = if a_later { (pa, pb) } else { (pb, pa) };
        let (mut kf, _) = self.cut_at(lp, first);
        let (ks, split) = self.cut_at(lp, second);
        if let Some(s) = split {
            if kf >= s {
                kf += 1;
            }
        }
        if a_later {
            (kf, ks)
        } else {
            (ks, kf)
        }
    }

    fn end_of(&mut self, face: usize, s: Span) -> Result<(V2, usize), String> {
        let node = self.node_of(s.line, s.f1).ok_or("a junction without a node")?;
        Ok((self.uv_at(face, s.line, s.f1, s.pc)?, node))
    }

    /// A loop cut in two by a straight cut from a to b.
    fn split_loop(&mut self, face: usize, lp: &[Span], pa: LPos, pb: LPos) -> Result<(Vec<Span>, Vec<Span>), String> {
        let mut lp = lp.to_vec();
        let (ka, kb) = self.cut_both(&mut lp, pa, pb);
        if ka == kb {
            return Err("a cut of no length".into());
        }
        let (one, two, _) = self.split_path(face, &lp, ka, &[], kb)?;
        Ok((one, two))
    }

    /// A loop cut in two along straight pieces from the junction after span
    /// ka to the one after span kb, through new points `mids` (in uv) inside
    /// it: the part from a to b, the part from b to a, the new nodes.
    fn split_path(&mut self, face: usize, lp: &[Span], ka: usize, mids: &[V2], kb: usize) -> Result<(Vec<Span>, Vec<Span>, Vec<usize>), String> {
        let n = lp.len();
        let mut pts = vec![self.end_of(face, lp[ka])?];
        let mut made = vec![];
        for m in mids {
            made.push(self.nodes);
            pts.push((*m, self.nodes));
            self.nodes += 1;
        }
        pts.push(self.end_of(face, lp[kb])?);
        let mut cuts = vec![];
        for w in pts.windows(2) {
            cuts.push(self.new_cut(face, w[0], w[1])?);
        }
        let mut one = vec![];
        let mut k = (ka + 1) % n;
        loop {
            one.push(lp[k]);
            if k == kb {
                break;
            }
            k = (k + 1) % n;
        }
        one.extend(cuts.iter().rev().map(|c| Span { line: *c, f0: 1.0, f1: 0.0, pc: false }));
        let mut two = vec![];
        k = (kb + 1) % n;
        loop {
            two.push(lp[k]);
            if k == ka {
                break;
            }
            k = (k + 1) % n;
        }
        two.extend(cuts.iter().map(|c| Span { line: *c, f0: 0.0, f1: 1.0, pc: false }));
        Ok((one, two, made))
    }

    /// A disc (a round loop with no corner): a square in the middle and a
    /// patch on each of its sides, joined to the outline along the
    /// diagonals. None when the loop is not round.
    fn disc(&mut self, fi: usize, map: FMap, lp: &[Span], g: &Geo, tmin: f64) -> Result<Option<Vec<Vec<Span>>>, String> {
        let turn: f64 = g.iter().map(|p| span_turn(p)).sum();
        if (turn - 2.0 * PI).abs() > 0.2 || g.iter().any(|p| span_turn(p) < -STRAIGHT) {
            return Ok(None);
        }
        let pts = loop_points(g);
        let c = k2(pts.iter().fold([0.0; 2], |a, p| a2(a, *p)), 1.0 / pts.len().max(1) as f64);
        let ds: Vec<f64> = pts.iter().map(|p| l2(s2(*p, c))).collect();
        let (lo, hi) = ds.iter().fold((f64::MAX, 0.0f64), |(a, b), d| (a.min(*d), b.max(*d)));
        if lo < 0.7 * hi {
            return Ok(None);
        }
        let r = ds.iter().sum::<f64>() / ds.len() as f64;
        let diag = |k: usize| {
            let a = PI / 4.0 + k as f64 * FRAC_PI_2;
            [a.cos(), a.sin()]
        };
        // the four places on the outline along the diagonals, made nodes
        let mut lp = lp.to_vec();
        let mut q = vec![];
        for k in 0..4 {
            let g = self.geo(fi, map, &lp)?;
            let (p, _) = self.cast_snap(&lp, &g, c, diag(k), tmin).ok_or("a disc with no outline")?;
            let (j, _) = self.cut_at(&mut lp, p);
            q.push(self.node_of(lp[j].line, lp[j].f1).ok_or("a corner without a node")?);
        }
        // the square's corners, inside (back in uv)
        let inner: Vec<V2> = (0..4)
            .map(|k| {
                let m = a2(c, k2(diag(k), 0.6 * r));
                [m[0] / map.su, m[1] / map.sv]
            })
            .collect();
        let find = |cx: &Ctx, lp: &[Span], node: usize| lp.iter().position(|s| cx.node_of(s.line, s.f1) == Some(node)).ok_or("a disc lost a node".to_string());
        let mut out = vec![];
        // q0 → p0 → p1 → q1: the first side patch and the rest
        let (ka, kb) = (find(self, &lp, q[0])?, find(self, &lp, q[1])?);
        let (side, mut rest, p01) = self.split_path(fi, &lp, ka, &inner[0..2], kb)?;
        out.push(side);
        // p1 → p2 → q2, then p2 → p3 → q3
        let mut p = vec![p01[0], p01[1]];
        for k in 2..4 {
            let (ka, kb) = (find(self, &rest, p[k - 1])?, find(self, &rest, q[k])?);
            let (side, r2, made) = self.split_path(fi, &rest, ka, &inner[k..k + 1], kb)?;
            out.push(side);
            rest = r2;
            p.push(made[0]);
        }
        // p3 → p0: the last side patch and the square
        let (ka, kb) = (find(self, &rest, p[3])?, find(self, &rest, p[0])?);
        let (side, square, _) = self.split_path(fi, &rest, ka, &[], kb)?;
        out.push(side);
        out.push(square);
        Ok(Some(out))
    }

    /// A hole joined to a loop by a straight cut (both ways along it).
    fn merge(&mut self, face: usize, outer: &[Span], pa: LPos, hole: &[Span], pb: LPos) -> Result<Vec<Span>, String> {
        let mut o = outer.to_vec();
        let (ka, _) = self.cut_at(&mut o, pa);
        let mut h = hole.to_vec();
        let (kb, _) = self.cut_at(&mut h, pb);
        let a = self.end_of(face, o[ka])?;
        let b = self.end_of(face, h[kb])?;
        let c = self.new_cut(face, a, b)?;
        let mut out: Vec<Span> = o[..=ka].to_vec();
        out.push(Span { line: c, f0: 0.0, f1: 1.0, pc: false });
        let m = h.len();
        for j in 1..=m {
            out.push(h[(kb + j) % m]);
        }
        out.push(Span { line: c, f0: 1.0, f1: 0.0, pc: false });
        out.extend_from_slice(&o[ka + 1..]);
        Ok(out)
    }

    /// Several holes in one region: the straight cut across it that keeps
    /// them apart with the widest margin (along a straight side of the
    /// outline, or square to the line between two holes).
    fn separate(&mut self, fi: usize, map: FMap, outer: &[Span], hs: &[Vec<Span>], tmin: f64) -> Result<Option<(Vec<Span>, Vec<Span>)>, String> {
        let go = self.geo(fi, map, outer)?;
        let mut hp: Vec<Vec<V2>> = vec![];
        for h in hs {
            hp.push(loop_points(&self.geo(fi, map, h)?));
        }
        let mut dirs: Vec<(V2, f64)> = vec![];
        for p in &go {
            if span_turn(p).abs() < STRAIGHT && span_len(p) > 1e-9 {
                dirs.push((u2(s2(p[p.len() - 1].1, p[0].1)), 1.0));
            }
        }
        let centre = |pts: &[V2]| k2(pts.iter().fold([0.0; 2], |a, p| a2(a, *p)), 1.0 / pts.len().max(1) as f64);
        for i in 0..hp.len() {
            for j in i + 1..hp.len() {
                let d = s2(centre(&hp[j]), centre(&hp[i]));
                if l2(d) > 1e-9 {
                    dirs.push((left2(u2(d)), 0.8));
                }
            }
        }
        let mut best: Option<(f64, V2, V2)> = None;
        for (d, w) in dirs {
            let n = left2(d);
            let mut iv: Vec<(f64, f64)> = hp.iter().map(|pts| pts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(d2(*p, n)), hi.max(d2(*p, n))))).collect();
            iv.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut top = iv[0].1;
            for k in 1..iv.len() {
                if iv[k].0 > top {
                    // the line n·p = c, in the middle of the gap, must cross
                    // the outline twice only
                    let c = 0.5 * (top + iv[k].0);
                    let sc = w * (iv[k].0 - top);
                    let mut xs: Vec<V2> = vec![];
                    for p in &go {
                        for s in p.windows(2) {
                            let (a, b) = (d2(s[0].1, n) - c, d2(s[1].1, n) - c);
                            if (a < 0.0) != (b < 0.0) {
                                xs.push(lerp2(s[0].1, s[1].1, a / (a - b)));
                            }
                        }
                    }
                    if xs.len() == 2 && best.map(|b| sc > b.0).unwrap_or(true) {
                        best = Some((sc, lerp2(xs[0], xs[1], 0.5), d));
                    }
                }
                top = top.max(iv[k].1);
            }
        }
        let Some((_, o, d)) = best else {
            return Ok(None);
        };
        let (Some((pa, _)), Some((pb, _))) = (self.cast_snap(outer, &go, o, d, tmin), self.cast_snap(outer, &go, o, k2(d, -1.0), tmin)) else {
            return Ok(None);
        };
        Ok(Some(self.split_loop(fi, outer, pa, pb)?))
    }

    /// The first crossing of a ray with the loop, put on a node of the line
    /// there when one is close.
    fn cast_snap(&self, lp: &[Span], g: &Geo, o: V2, d: V2, tmin: f64) -> Option<(LPos, f64)> {
        let (mut p, h, t) = cast(g, o, d, tmin)?;
        let mut best = (0.3 * self.target).min(0.15 * t);
        let (lo, hi) = lp[p.i].range();
        for &(fs, _) in &self.lines[lp[p.i].line].splits {
            if fs >= lo - EPS_F && fs <= hi + EPS_F {
                let (q, _) = on_span(&g[p.i], fs);
                let dd = l2(s2(q, h));
                if dd < best {
                    best = dd;
                    p.f = fs;
                }
            }
        }
        Some((p, t))
    }

    /// The farthest point of a loop's spans in a direction (spans of `lines`
    /// only, when given).
    fn extreme(g: &Geo, lp: &[Span], lines: Option<&HashSet<usize>>, dir: V2) -> Option<(LPos, V2)> {
        let mut best: Option<(f64, LPos, V2)> = None;
        for (i, p) in g.iter().enumerate() {
            if lines.map(|l| !l.contains(&lp[i].line)).unwrap_or(false) {
                continue;
            }
            for q in p {
                let v = d2(q.1, dir);
                if best.map(|b| v > b.0 + 1e-12).unwrap_or(true) {
                    best = Some((v, LPos { i, f: q.0 }, q.1));
                }
            }
        }
        best.map(|b| (b.1, b.2))
    }
}

/// The point, the direction coming in and going out at a place of a loop.
fn place(lp: &[Span], g: &Geo, p: LPos) -> (V2, V2, V2) {
    let n = lp.len();
    let s = lp[p.i];
    if (p.f - s.f1).abs() < EPS_F {
        return (g[p.i][g[p.i].len() - 1].1, tan_end(&g[p.i]), tan_start(&g[(p.i + 1) % n]));
    }
    if (p.f - s.f0).abs() < EPS_F {
        let j = (p.i + n - 1) % n;
        return (g[p.i][0].1, tan_end(&g[j]), tan_start(&g[p.i]));
    }
    let (q, t) = on_span(&g[p.i], p.f);
    (q, t, t)
}

/// A place as a position round the loop, in [0, n): span index plus the
/// part of it travelled.
fn loop_param(lp: &[Span], p: LPos) -> f64 {
    let n = lp.len() as f64;
    let s = lp[p.i];
    let t = if (s.f1 - s.f0).abs() > 1e-15 { ((p.f - s.f0) / (s.f1 - s.f0)).clamp(0.0, 1.0) } else { 0.0 };
    (p.i as f64 + t).rem_euclid(n)
}

/// x strictly between lo and hi going round a loop of length n.
fn between(x: f64, lo: f64, hi: f64) -> bool {
    let e = 1e-9;
    if lo < hi {
        x > lo + e && x < hi - e
    } else {
        x > lo + e || x < hi - e
    }
}

/// How good a cut from a to b is (lower is better), or None when it leaves
/// the region or makes an inner corner.
fn score(lp: &[Span], g: &Geo, cs: &[(usize, f64)], pa: LPos, pb: LPos, strict: bool) -> Option<f64> {
    let (p, in_p, out_p) = place(lp, g, pa);
    let (q, in_q, out_q) = place(lp, g, pb);
    if l2(s2(q, p)) < 1e-9 {
        return None;
    }
    if crosses(g, p, q) || !inside(g, lerp2(p, q, 0.5)) {
        return None;
    }
    // no cut brushing past the outline (it would leave a crescent)
    if !clear(g, p, q, if strict { 0.06 } else { 0.02 } * l2(s2(q, p))) {
        return None;
    }
    let c = u2(s2(q, p));
    let back = k2(c, -1.0);
    // region one: a → … → b along the loop, then back; region two: b → … → a
    let t1q = turn(in_q, back);
    let t1p = turn(back, out_p);
    let t2p = turn(in_p, c);
    let t2q = turn(c, out_q);
    // no inner corner, and no sliver (a cut leaving a side at under 25°)
    let sliver = if strict { 155f64 } else { 170f64 }.to_radians();
    if [t1p, t1q, t2p, t2q].iter().any(|t| *t < -CORNER || *t > sliver) {
        return None;
    }
    let corner = |t: f64| (t.abs() > CORNER) as usize;
    // the new angles at an end: two corners are best halving the angle
    // there (a right angle from a straight side, 45° from a square
    // corner); one alone is best square
    let dev = |ta: f64, tb: f64| {
        let (a, b) = (PI - ta, PI - tb);
        match (ta.abs() > CORNER, tb.abs() > CORNER) {
            (true, true) => (a - b).abs(),
            (true, false) => (a - FRAC_PI_2).abs(),
            (false, true) => (b - FRAC_PI_2).abs(),
            _ => 0.0,
        }
    };
    let len = lp.len();
    let n = len as f64;
    let (a, b) = (loop_param(lp, pa), loop_param(lp, pb));
    let at: Vec<f64> = cs.iter().map(|c| (c.0 as f64 + 1.0).rem_euclid(n)).collect();
    let n1 = at.iter().filter(|x| between(**x, a, b)).count() + corner(t1p) + corner(t1q);
    let n2 = at.iter().filter(|x| between(**x, b, a)).count() + corner(t2p) + corner(t2q);
    // a region with more than four corners is only cut into regions with
    // fewer (no endless slicing of the same shape)
    if cs.len() > 4 && cs.iter().all(|c| c.1 > -CORNER) && (n1 >= cs.len() || n2 >= cs.len()) {
        return None;
    }
    let cost = |m: usize| match m {
        4 => 0.0,
        // three corners: a fourth goes in the middle of a side, flat there
        m if m < 4 => 1.5,
        // a region left with many corners is worse than two with a few
        m => (m - 4) as f64 * (1.0 + 0.25 * (m - 5) as f64),
    };
    // a side of a new region that bends more than 100° (half a hole as one
    // side) makes a poor patch
    let st: Vec<f64> = g.iter().map(|p| span_turn(p)).collect();
    let jt: Vec<f64> = (0..len).map(|k| turn(tan_end(&g[k]), tan_start(&g[(k + 1) % len]))).collect();
    let is_c: Vec<bool> = (0..len).map(|k| cs.iter().any(|c| c.0 == k)).collect();
    let along = |x: f64, y: f64| -> f64 {
        let y = if y <= x { y + n } else { y };
        let mut t = 0.0;
        let mut k = x.floor() as i64;
        while (k as f64) < y {
            let i = k.rem_euclid(len as i64) as usize;
            let (lo, hi) = ((k as f64).max(x), ((k + 1) as f64).min(y));
            if hi > lo {
                t += st[i] * (hi - lo);
            }
            let j = (k + 1) as f64;
            if j > x + 1e-9 && j < y - 1e-9 && !is_c[i] {
                t += jt[i];
            }
            k += 1;
        }
        t
    };
    let bend = |from: f64, to: f64| -> f64 {
        let mut pts: Vec<f64> = at.iter().copied().filter(|x| between(*x, from, to)).collect();
        pts.sort_by(|x, y| (x - from).rem_euclid(n).total_cmp(&(y - from).rem_euclid(n)));
        let mut stops = vec![from];
        stops.extend(pts);
        stops.push(to);
        stops.windows(2).map(|w| (along(w[0], w[1]).abs() - 100f64.to_radians()).max(0.0)).sum::<f64>()
    };
    Some(cost(n1) + cost(n2) + 0.8 * (dev(t1p, t2p) + dev(t1q, t2q)) + bend(a, b) + bend(b, a))
}

impl Ctx<'_> {
    /// A face cut into four-cornered regions: each region's loop and its
    /// four corners (junctions).
    fn face_regions(&mut self, fi: usize) -> Result<Vec<(Vec<Span>, Vec<usize>)>, String> {
        let wires = self.topo.faces[fi].wires.clone();
        if wires.is_empty() {
            return Err("no outline".into());
        }
        let pole = wires.iter().flatten().any(|(e, _)| self.topo.edges[(*e - 1) as usize].degenerated);
        if pole && wires.len() > 1 {
            return Err("a pole (degenerated edge) on a face with holes".into());
        }
        // the loops, joined up in (u, v): OpenCascade's order is kept, but
        // an edge whose curve runs the other way there (a closed edge on a
        // seamed surface) is travelled backwards
        let mut loops: Vec<Vec<Span>> = vec![];
        for w in &wires {
            let mut lp: Vec<Span> = vec![];
            let mut prev: Option<V2> = None;
            for &(e, r) in w {
                let line = (e - 1) as usize;
                let mut best: Option<(f64, Span)> = None;
                // its own curve first, then (a seam) its other one
                for pc in [r, !r] {
                    let p = self.poly(fi, e, pc)?;
                    let (a, b) = (p[0].1, p[p.len() - 1].1);
                    for back in [r, !r] {
                        let start = if back { b } else { a };
                        let gap = prev.map(|q| l2(s2(start, q))).unwrap_or(0.0);
                        let span = Span { line, f0: if back { 1.0 } else { 0.0 }, f1: if back { 0.0 } else { 1.0 }, pc };
                        if best.as_ref().map(|x| gap < x.0 - 1e-9).unwrap_or(true) {
                            best = Some((gap, span));
                        }
                    }
                }
                let span = best.unwrap().1;
                let p = self.poly(fi, e, span.pc)?;
                prev = Some(if span.rev() { p[0].1 } else { p[p.len() - 1].1 });
                lp.push(span);
            }
            loops.push(lp);
        }

        // the map: (u, v) scaled to millimetres at the middle of the outline
        let unit = FMap { su: 1.0, sv: 1.0 };
        let g0 = self.geo(fi, unit, &loops[0])?;
        if std::env::var("FORME_QUADS_DEBUG").is_ok() {
            for (w, lp) in loops.iter().enumerate() {
                let g = self.geo(fi, unit, lp)?;
                for (i, s) in lp.iter().enumerate() {
                    eprintln!("face {fi} wire {w} span {i}: edge {} {:.0}->{:.0} uv {:?} -> {:?}", self.lines[s.line].edge, s.f0, s.f1, g[i][0].1, g[i][g[i].len() - 1].1);
                }
            }
        }
        let pts = loop_points(&g0);
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for p in &pts {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let c = lerp2(lo, hi, 0.5);
        let (hu, hv) = (1e-3 * (hi[0] - lo[0]).max(1e-9), 1e-3 * (hi[1] - lo[1]).max(1e-9));
        let ev = self.shape.face_eval(fi, &[[c[0] - hu, c[1]], [c[0] + hu, c[1]], [c[0], c[1] - hv], [c[0], c[1] + hv]])?;
        let su = len(sub(ev[1].0, ev[0].0)) / (2.0 * hu);
        let sv = len(sub(ev[3].0, ev[2].0)) / (2.0 * hv);
        let mut map = FMap { su: if su > 1e-12 { su } else { 1.0 }, sv: if sv > 1e-12 { sv } else { 1.0 } };
        if signed_area(&pts) < 0.0 {
            map.su = -map.su;
        }
        let gm = self.geo(fi, map, &loops[0])?;
        let mp = loop_points(&gm);
        let size = mp.iter().fold(0.0f64, |m, p| m.max(l2(s2(*p, mp[0]))));
        let tmin = 1e-6 * size.max(1e-9);
        // holes run the other way round (clockwise)
        for h in loops.iter_mut().skip(1) {
            let g = self.geo(fi, map, h)?;
            if signed_area(&loop_points(&g)) > 0.0 {
                h.reverse();
                for s in h.iter_mut() {
                    std::mem::swap(&mut s.f0, &mut s.f1);
                }
            }
        }

        // a corner patch closing on a pole (where three fillets meet: a
        // sphere's eighth): three quads round a point in its middle
        if pole {
            return self.triangle(fi, map, &loops[0]);
        }

        // 0. several holes: straight cuts across the face between them
        // first, until each region keeps one
        let mut regions: Vec<(Vec<Span>, Vec<Vec<Span>>)> = vec![];
        let mut todo = vec![(loops[0].clone(), loops[1..].to_vec())];
        while let Some((outer, hs)) = todo.pop() {
            if hs.len() > 1 {
                if let Some((one, two)) = self.separate(fi, map, &outer, &hs, tmin)? {
                    if std::env::var("FORME_QUADS_DEBUG").is_ok() {
                        eprintln!("face {fi}: holes kept apart ({} holes)", hs.len());
                    }
                    let g1 = self.geo(fi, map, &one)?;
                    let (mut h1, mut h2) = (vec![], vec![]);
                    for h in hs {
                        let gh = self.geo(fi, map, &h)?;
                        if inside(&g1, gh[0][0].1) {
                            h1.push(h);
                        } else {
                            h2.push(h);
                        }
                    }
                    todo.push((one, h1));
                    todo.push((two, h2));
                    continue;
                }
            }
            regions.push((outer, hs));
        }

        // 1. holes: two straight rays each, left then right (holes taken
        // from left to right, so the left ray always meets the outline)
        let mut holes: Vec<(f64, Vec<Span>)> = vec![];
        for h in &loops[1..] {
            let g = self.geo(fi, map, h)?;
            let x = loop_points(&g).iter().fold(f64::MAX, |m, p| m.min(p[0]));
            holes.push((x, h.clone()));
        }
        holes.sort_by(|a, b| a.0.total_cmp(&b.0));
        let order: Vec<HashSet<usize>> = holes.iter().map(|h| h.1.iter().map(|s| s.line).collect()).collect();
        for lines in &order {
            let first = *lines.iter().next().unwrap();
            let r = regions.iter().position(|(o, hs)| o.iter().any(|s| s.line == first) || hs.iter().any(|h| h.iter().any(|s| s.line == first))).ok_or("a hole lost")?;
            let (mut outer, mut hs) = regions.remove(r);
            if let Some(k) = hs.iter().position(|h| h.iter().any(|s| s.line == first)) {
                // the left ray, to the outline
                let h = hs.remove(k);
                let gh = self.geo(fi, map, &h)?;
                let (ph, o) = Ctx::extreme(&gh, &h, None, [-1.0, 0.0]).ok_or("an empty hole")?;
                let go = self.geo(fi, map, &outer)?;
                let (po, _) = self.cast_snap(&outer, &go, o, [-1.0, 0.0], tmin).ok_or("a hole's ray meets nothing")?;
                outer = self.merge(fi, &outer, po, &h, ph)?;
            }
            // the right ray: to the outline (the region is cut in two), or to
            // another hole (joined; its own right ray comes later)
            let go = self.geo(fi, map, &outer)?;
            let (pa, o) = Ctx::extreme(&go, &outer, Some(lines), [1.0, 0.0]).ok_or("a hole lost")?;
            let mut best: Option<(f64, Option<usize>, LPos)> = self.cast_snap(&outer, &go, o, [1.0, 0.0], tmin).map(|(p, t)| (t, None, p));
            // (a hole runs the other way round, its inside on the right: the
            // ray crosses it from left to right too)
            for (k, h) in hs.iter().enumerate() {
                let gh = self.geo(fi, map, h)?;
                if let Some((p, t)) = self.cast_snap(h, &gh, o, [1.0, 0.0], tmin) {
                    if best.as_ref().map(|b| t < b.0).unwrap_or(true) {
                        best = Some((t, Some(k), p));
                    }
                }
            }
            match best.ok_or("a hole's ray meets nothing")? {
                (_, None, pb) => {
                    let (one, two) = self.split_loop(fi, &outer, pa, pb)?;
                    let g1 = self.geo(fi, map, &one)?;
                    let (mut h1, mut h2) = (vec![], vec![]);
                    for h in hs {
                        let gh = self.geo(fi, map, &h)?;
                        if inside(&g1, gh[0][0].1) {
                            h1.push(h);
                        } else {
                            h2.push(h);
                        }
                    }
                    regions.push((one, h1));
                    regions.push((two, h2));
                }
                (_, Some(k), pb) => {
                    let h = hs.remove(k);
                    let merged = self.merge(fi, &outer, pa, &h, pb)?;
                    regions.push((merged, hs));
                }
            }
        }
        if regions.iter().any(|r| !r.1.is_empty()) {
            return Err("a hole could not be joined".into());
        }

        // 2. cut until every region has four corners
        let mut forced: HashSet<usize> = HashSet::new();
        let mut work: Vec<Vec<Span>> = regions.into_iter().map(|r| r.0).collect();
        let mut done = vec![];
        let mut steps = 0;
        while let Some(lp) = work.pop() {
            steps += 1;
            if steps > 400 {
                return Err("too many cuts".into());
            }
            let g = self.geo(fi, map, &lp)?;
            let cs = self.corners(&g, &lp, &forced);
            let n = lp.len();

            // an inner corner: a ray along one of its sides
            let reflex: Vec<usize> = cs.iter().filter(|c| c.1 < -CORNER).map(|c| c.0).collect();
            if !reflex.is_empty() {
                let mut best: Option<(f64, LPos, LPos)> = None;
                for &k in &reflex {
                    let o = g[k][g[k].len() - 1].1;
                    let pa = LPos { i: k, f: lp[k].f1 };
                    for d in [tan_end(&g[k]), k2(tan_start(&g[(k + 1) % n]), -1.0)] {
                        if let Some((pb, t)) = self.cast_snap(&lp, &g, o, d, tmin) {
                            if let Some(sc) = score(&lp, &g, &cs, pa, pb, true).or_else(|| score(&lp, &g, &cs, pa, pb, false).map(|x| x + 10.0)) {
                                let sc = sc + 0.02 * t / size.max(1e-9);
                                if best.as_ref().map(|b| sc < b.0).unwrap_or(true) {
                                    best = Some((sc, pa, pb));
                                }
                            }
                        }
                    }
                }
                if best.is_none() && std::env::var("FORME_QUADS_DEBUG").is_ok() {
                    eprintln!("face {fi}: inner corner, no way in; corners {cs:?}");
                    for (i, s) in lp.iter().enumerate() {
                        eprintln!("  span {i} line {} edge {} {:.3}->{:.3}  {:?} -> {:?} turn {:.1}", s.line, self.lines[s.line].edge, s.f0, s.f1, g[i][0].1, g[i][g[i].len() - 1].1, span_turn(&g[i]).to_degrees());
                    }
                    for &k in &reflex {
                        let o = g[k][g[k].len() - 1].1;
                        let pa = LPos { i: k, f: lp[k].f1 };
                        for d in [tan_end(&g[k]), k2(tan_start(&g[(k + 1) % n]), -1.0)] {
                            let hit = self.cast_snap(&lp, &g, o, d, tmin);
                            eprintln!("  ray from {k} {o:?} dir {d:?}: {hit:?} score {:?}", hit.and_then(|(pb, _)| score(&lp, &g, &cs, pa, pb, false)));
                        }
                    }
                }
                let (_, pa, pb) = best.ok_or("an inner corner with no way in")?;
                if std::env::var("FORME_QUADS_DEBUG").is_ok() { eprintln!("face {fi} inner: {:?} -> {:?} ({} corners)", place(&lp, &g, pa).0, place(&lp, &g, pb).0, cs.len()); }
                let (one, two) = self.split_loop(fi, &lp, pa, pb)?;
                work.push(one);
                work.push(two);
                continue;
            }

            // a hole's half (an arc bending inwards by more than 120°): a ray
            // from its middle, straight out from it
            let mut ray = None;
            for k in 0..n {
                if span_turn(&g[k]) < -120f64.to_radians() {
                    // from its middle, or better from the place nearest it
                    // where the ray runs along u or v (square to the sides)
                    let s = lp[k];
                    let p = &g[k];
                    let total = span_len(p);
                    let mut f = frac_at_length(p, 0.5);
                    let mut near = 0.3;
                    let mut acc = 0.0;
                    for i in 1..p.len().saturating_sub(1) {
                        acc += l2(s2(p[i].1, p[i - 1].1));
                        let (ta, tb) = (s2(p[i].1, p[i - 1].1), s2(p[i + 1].1, p[i].1));
                        for ax in 0..2 {
                            let x = acc / total.max(1e-12);
                            if (ta[ax] < 0.0) != (tb[ax] < 0.0) && (x - 0.5).abs() < near {
                                near = (x - 0.5).abs();
                                // the exact place, halving the bracket
                                let (mut lo, mut hi) = (p[i - 1].0, p[i + 1].0);
                                let sign_lo = self.span_tan(fi, map, s, lo)?[ax] < 0.0;
                                for _ in 0..40 {
                                    let mid = 0.5 * (lo + hi);
                                    if (self.span_tan(fi, map, s, mid)?[ax] < 0.0) == sign_lo {
                                        lo = mid;
                                    } else {
                                        hi = mid;
                                    }
                                }
                                f = 0.5 * (lo + hi);
                            }
                        }
                    }
                    let (o, _) = on_span(p, f);
                    // the direction along the curve there, from the curve itself
                    let t = self.span_tan(fi, map, s, f)?;
                    if let Some((pb, _)) = self.cast_snap(&lp, &g, o, left2(t), tmin) {
                        ray = Some((LPos { i: k, f }, pb));
                        break;
                    }
                }
            }
            if let Some((pa, pb)) = ray {
                if std::env::var("FORME_QUADS_DEBUG").is_ok() {
                    eprintln!("face {fi} hole ray: {:?} -> {:?}", place(&lp, &g, pa).0, place(&lp, &g, pb).0);
                }
                let (one, two) = self.split_loop(fi, &lp, pa, pb)?;
                work.push(one);
                work.push(two);
                continue;
            }

            // a straight side running into a rounded corner: a ray straight in
            let mut ray = None;
            for k in 0..n {
                if cs.iter().any(|c| c.0 == k) {
                    continue;
                }
                let (a, b) = (&g[k], &g[(k + 1) % n]);
                let (ta, tb) = (span_turn(a), span_turn(b));
                let (sa, sb) = (ta.abs() < STRAIGHT, tb.abs() < STRAIGHT);
                if (sa && tb > ROUNDED) || (ta > ROUNDED && sb) {
                    let o = a[a.len() - 1].1;
                    let t = if sa { tan_end(a) } else { tan_start(b) };
                    if let Some((pb, _)) = self.cast_snap(&lp, &g, o, left2(t), tmin) {
                        // not when it only grazes what it meets (a cusp), or
                        // brushes past the outline on its way
                        let (q, ti, to) = place(&lp, &g, pb);
                        if x2(left2(t), ti).abs().min(x2(left2(t), to).abs()) < 20f64.to_radians().sin() || !clear(&g, o, q, 0.3 * self.target) {
                            continue;
                        }
                        ray = Some((LPos { i: k, f: lp[k].f1 }, pb));
                        break;
                    }
                }
            }
            if let Some((pa, pb)) = ray {
                if std::env::var("FORME_QUADS_DEBUG").is_ok() { eprintln!("face {fi} rounded: {:?} -> {:?} ({} corners)", place(&lp, &g, pa).0, place(&lp, &g, pb).0, cs.len()); }
                let (one, two) = self.split_loop(fi, &lp, pa, pb)?;
                work.push(one);
                work.push(two);
                continue;
            }

            // too many corners: the best cut between corners and middles of
            // the runs between them
            if cs.len() > 4 {
                let m = cs.len();
                let lens: Vec<f64> = g.iter().map(|p| span_len(p)).collect();
                let mut cands: Vec<(LPos, usize, usize)> = vec![];
                for (r, c) in cs.iter().enumerate() {
                    cands.push((LPos { i: c.0, f: lp[c.0].f1 }, (r + m - 1) % m, r));
                    // the middle of run r: from corner r to corner r + 1
                    let (k0, k1) = (c.0, cs[(r + 1) % m].0);
                    let mut spans = vec![];
                    let mut k = (k0 + 1) % n;
                    loop {
                        spans.push(k);
                        if k == k1 {
                            break;
                        }
                        k = (k + 1) % n;
                    }
                    let run: f64 = spans.iter().map(|k| lens[*k]).sum();
                    let mut acc = 0.0;
                    for k in spans {
                        if acc + lens[k] >= run * 0.5 {
                            let f = frac_at_length(&g[k], (run * 0.5 - acc) / lens[k].max(1e-15));
                            cands.push((LPos { i: k, f }, r, r));
                            break;
                        }
                        acc += lens[k];
                    }
                }
                let mut best: Option<(f64, LPos, LPos)> = None;
                for x in 0..cands.len() {
                    for y in x + 1..cands.len() {
                        let (a, b) = (cands[x], cands[y]);
                        if a.1 == b.1 || a.1 == b.2 || a.2 == b.1 || a.2 == b.2 {
                            continue;
                        }
                        let got = score(&lp, &g, &cs, a.0, b.0, true).or_else(|| score(&lp, &g, &cs, a.0, b.0, false).map(|x| x + 10.0));
                        if std::env::var("FORME_QUADS_CANDS").is_ok() {
                            eprintln!("    cand {:?} -> {:?}: {:?}", place(&lp, &g, a.0).0, place(&lp, &g, b.0).0, got);
                        }
                        if let Some(sc) = got {
                            if best.as_ref().map(|bb| sc < bb.0 - 1e-9).unwrap_or(true) {
                                best = Some((sc, a.0, b.0));
                            }
                        }
                    }
                }
                if best.is_none() && std::env::var("FORME_QUADS_DEBUG").is_ok() {
                    eprintln!("face {fi}: {} corners, no cut; corners {cs:?}", cs.len());
                    for (i, s) in lp.iter().enumerate() {
                        eprintln!("  span {i} line {} edge {} {:.3}->{:.3}  {:?} -> {:?} turn {:.1}", s.line, self.lines[s.line].edge, s.f0, s.f1, g[i][0].1, g[i][g[i].len() - 1].1, span_turn(&g[i]).to_degrees());
                    }
                }
                let (_, pa, pb) = best.ok_or(format!("{} corners and no cut found", cs.len()))?;
                if std::env::var("FORME_QUADS_DEBUG").is_ok() { eprintln!("face {fi} chord: {:?} -> {:?} ({} corners)", place(&lp, &g, pa).0, place(&lp, &g, pb).0, cs.len()); }
                let (one, two) = self.split_loop(fi, &lp, pa, pb)?;
                work.push(one);
                work.push(two);
                continue;
            }

            // a disc: a square in the middle, a patch on each of its sides
            if cs.is_empty() {
                if let Some(rs) = self.disc(fi, map, &lp, &g, tmin)? {
                    if std::env::var("FORME_QUADS_DEBUG").is_ok() {
                        eprintln!("face {fi}: a disc, a square in the middle");
                    }
                    work.extend(rs);
                    continue;
                }
            }

            // too few: corners in the middle of the longest runs
            if cs.len() < 4 {
                let lens: Vec<f64> = g.iter().map(|p| span_len(p)).collect();
                let total: f64 = lens.iter().sum();
                let mut cum = vec![0.0];
                for l in &lens {
                    cum.push(cum.last().unwrap() + l);
                }
                // (the middle s of the longest run, half its length)
                let (s, half) = if cs.is_empty() {
                    let k = (0..n).max_by(|a, b| ((span_turn(&g[*a]) > ROUNDED), lens[*a]).partial_cmp(&((span_turn(&g[*b]) > ROUNDED), lens[*b])).unwrap()).unwrap();
                    (cum[k] + lens[k] * 0.5, 0.0)
                } else {
                    let mut pos: Vec<f64> = cs.iter().map(|c| cum[c.0 + 1]).collect();
                    pos.sort_by(|a, b| a.total_cmp(b));
                    let m = pos.len();
                    let (i, run) = (0..m)
                        .map(|i| {
                            let (a, b) = (pos[i], pos[(i + 1) % m]);
                            (i, if b > a + 1e-9 { b - a } else { b + total - a })
                        })
                        .max_by(|a, b| a.1.total_cmp(&b.1))
                        .unwrap();
                    (pos[i] + run * 0.5, run * 0.5)
                };
                // a rounded part of the run keeps a side of its own: the
                // corner goes where it meets a straight side (nearest the
                // middle of the run)
                let mut at_join: Option<(f64, usize)> = None;
                for k in 0..n {
                    let j = (k + 1) % n;
                    let (ta, tb) = (span_turn(&g[k]).abs(), span_turn(&g[j]).abs());
                    if cs.iter().any(|c| c.0 == k) || !((ta < STRAIGHT && tb > ROUNDED) || (ta > ROUNDED && tb < STRAIGHT)) {
                        continue;
                    }
                    let x = cum[k + 1];
                    let d = (x - s).rem_euclid(total).min((s - x).rem_euclid(total));
                    if d < half - 1e-9 && at_join.map(|b| d < b.0).unwrap_or(true) {
                        at_join = Some((d, k));
                    }
                }
                if let Some((_, k)) = at_join {
                    let node = self.node_of(lp[k].line, lp[k].f1).ok_or("a corner without a node")?;
                    if !forced.insert(node) {
                        return Err("its corners could not be placed".into());
                    }
                    work.push(lp);
                    continue;
                }
                let s = s.rem_euclid(total);
                let mut k = 0;
                while k < n - 1 && s > cum[k + 1] {
                    k += 1;
                }
                let frac = if lens[k] > 0.0 { (s - cum[k]) / lens[k] } else { 0.5 };
                let is_corner = |j: usize| cs.iter().any(|c| c.0 == j);
                let prev = (k + n - 1) % n;
                let p = if frac < 0.2 && !is_corner(prev) {
                    LPos { i: prev, f: lp[prev].f1 }
                } else if frac > 0.8 && !is_corner(k) {
                    LPos { i: k, f: lp[k].f1 }
                } else {
                    LPos { i: k, f: frac_at_length(&g[k], frac) }
                };
                let mut lp = lp;
                let (j, _) = self.cut_at(&mut lp, p);
                let node = self.node_of(lp[j].line, lp[j].f1).ok_or("a corner without a node")?;
                if !forced.insert(node) {
                    return Err("its corners could not be placed".into());
                }
                work.push(lp);
                continue;
            }
            done.push((lp, cs.iter().map(|c| c.0).collect()));
        }
        Ok(done)
    }

    /// A degenerated edge (a pole: one point in space, a line in uv).
    fn is_pole(&self, line: usize) -> bool {
        let e = self.lines[line].edge;
        e > 0 && self.topo.edges[(e - 1) as usize].degenerated
    }

    /// A span's length in space.
    fn span_len3(&self, s: Span) -> f64 {
        let l = &self.lines[s.line];
        let whole = if l.edge > 0 { self.arcs[(l.edge - 1) as usize].length() } else { l.length };
        whole * (s.f1 - s.f0).abs()
    }

    /// The point in space at a line fraction, seen from a face.
    fn point3(&mut self, face: usize, s: Span, f: f64) -> Result<[f64; 3], String> {
        let l = &self.lines[s.line];
        if l.edge == 0 {
            let uv = lerp2(l.uv[0], l.uv[1], f);
            return Ok(self.shape.face_eval(face, &[uv])?[0].0);
        }
        let e = l.edge;
        let t = self.arcs[(e - 1) as usize].param(f);
        Ok(self.shape.edge_on_face(face, e, s.pc, &[t])?.0[0])
    }

    /// The junction of a loop that ends on `node` (on a pole, the one just
    /// before the degenerated edge).
    fn junction(&self, lp: &[Span], node: usize) -> Option<usize> {
        let n = lp.len();
        let at: Vec<usize> = (0..n).filter(|k| self.node_of(lp[*k].line, lp[*k].f1) == Some(node)).collect();
        at.iter().copied().find(|k| self.is_pole(lp[(k + 1) % n].line)).or(at.first().copied())
    }

    /// A face of three corners closing on a pole (where three fillets meet:
    /// a sphere's eighth): a point in its middle joined to the middles of
    /// its sides, three quads meeting there (a pole of three, on purpose).
    fn triangle(&mut self, fi: usize, map: FMap, lp: &[Span]) -> Result<Vec<(Vec<Span>, Vec<usize>)>, String> {
        let n = lp.len();
        let kd: Vec<usize> = (0..n).filter(|k| self.is_pole(lp[*k].line)).collect();
        if kd.len() != 1 {
            return Err("a face with several poles".into());
        }
        let kd = kd[0];
        let g = self.geo(fi, map, lp)?;
        // the corners: the pole (the junction before it), and where the
        // outline turns elsewhere
        let mut cs = vec![(kd + n - 1) % n];
        for k in 0..n {
            let j = (k + 1) % n;
            if k == kd || j == kd || k == (kd + n - 1) % n {
                continue;
            }
            if turn(tan_end(&g[k]), tan_start(&g[j])).abs() > CORNER {
                cs.push(k);
            }
        }
        if cs.len() != 3 {
            return Err(format!("a pole on a face of {} corners", cs.len()));
        }
        cs.sort();
        let corner_nodes: Vec<usize> = cs.iter().map(|k| self.node_of(lp[*k].line, lp[*k].f1).ok_or("a corner without a node")).collect::<Result<_, _>>()?;
        // the middle of each side, by length in space
        let mut mids: Vec<(usize, LPos)> = vec![];
        for r in 0..3 {
            let (k0, k1) = (cs[r], cs[(r + 1) % 3]);
            let mut spans = vec![];
            let mut k = (k0 + 1) % n;
            loop {
                spans.push(k);
                if k == k1 {
                    break;
                }
                k = (k + 1) % n;
            }
            let lens: Vec<f64> = spans.iter().map(|k| self.span_len3(lp[*k])).collect();
            let half = 0.5 * lens.iter().sum::<f64>();
            let mut acc = 0.0;
            for (k, l) in spans.iter().zip(&lens) {
                if acc + l >= half && *l > 0.0 {
                    let t = ((half - acc) / l).clamp(0.0, 1.0);
                    let s = lp[*k];
                    mids.push((r, LPos { i: *k, f: s.f0 + (s.f1 - s.f0) * t }));
                    break;
                }
                acc += l;
            }
        }
        if mids.len() != 3 {
            return Err("a side of no length".into());
        }
        // made nodes, from the last span back (the indices before stay)
        let mut lp = lp.to_vec();
        let mut order = mids.clone();
        order.sort_by(|a, b| b.1.i.cmp(&a.1.i).then(b.1.f.total_cmp(&a.1.f)));
        let mut mid_nodes = [0usize; 3];
        let mut mid_pts = [[0.0; 3]; 3];
        let mut mid_uv = [[0.0; 2]; 3];
        for (r, p) in order {
            let s = lp[p.i];
            mid_pts[r] = self.point3(fi, s, p.f)?;
            mid_uv[r] = self.uv_at(fi, s.line, p.f, s.pc)?;
            let (j, _) = self.cut_at(&mut lp, p);
            mid_nodes[r] = self.node_of(lp[j].line, lp[j].f1).ok_or("a middle without a node")?;
        }
        // the middle: the surface's point nearest the middle of the sides'
        // middles and the corners, in the face's own (u, v) period
        let mut sum = [0.0; 3];
        for p in mid_pts.iter() {
            for c in 0..3 {
                sum[c] += p[c] / 3.0;
            }
        }
        let guess = k2(a2(a2(mid_uv[0], mid_uv[1]), mid_uv[2]), 1.0 / 3.0);
        let centre = match self.shape.face_project(fi, &[sum]) {
            Ok(v) => {
                let (mut uv, p, _) = v[0];
                for c in 0..2 {
                    let period = 2.0 * PI;
                    while uv[c] - guess[c] > 0.5 * period {
                        uv[c] -= period;
                    }
                    while guess[c] - uv[c] > 0.5 * period {
                        uv[c] += period;
                    }
                }
                let back = self.shape.face_eval(fi, &[uv])?[0].0;
                if len(sub(back, p)) < 1e-6 { uv } else { guess }
            }
            Err(_) => guess,
        };
        // three cuts: middle of side 0 → centre → middle of side 1, then
        // centre → middle of side 2
        let ka = self.junction(&lp, mid_nodes[0]).ok_or("a middle lost")?;
        let kb = self.junction(&lp, mid_nodes[1]).ok_or("a middle lost")?;
        let (one, two, made) = self.split_path(fi, &lp, ka, &[centre], kb)?;
        let c = made[0];
        let ka = self.junction(&two, c).ok_or("the centre lost")?;
        let kb = self.junction(&two, mid_nodes[2]).ok_or("a middle lost")?;
        let (two_a, two_b, _) = self.split_path(fi, &two, ka, &[], kb)?;
        let mut keys: Vec<usize> = corner_nodes;
        keys.extend(mid_nodes);
        keys.push(c);
        let mut out = vec![];
        for r in [one, two_a, two_b] {
            let mut cs: Vec<usize> = keys.iter().filter_map(|k| self.junction(&r, *k)).collect();
            cs.sort();
            cs.dedup();
            if cs.len() != 4 {
                return Err(format!("a corner patch with {} corners", cs.len()));
            }
            out.push((r, cs));
        }
        if std::env::var("FORME_QUADS_DEBUG").is_ok() {
            eprintln!("face {fi}: three corners round a pole, three quads round a middle");
        }
        Ok(out)
    }

    /// The pieces of a span, in travel order.
    fn pieces(&self, s: Span) -> Vec<(Piece, bool, bool)> {
        let sp = &self.lines[s.line].splits;
        let (lo, hi) = s.range();
        let mut v: Vec<(Piece, bool, bool)> = (0..sp.len() - 1).filter(|j| sp[*j].0 >= lo - EPS_F && sp[*j + 1].0 <= hi + EPS_F).map(|j| (Piece { line: s.line, j }, s.rev(), s.pc)).collect();
        if s.rev() {
            v.reverse();
        }
        v
    }

    fn piece_len(&self, p: Piece) -> f64 {
        let l = &self.lines[p.line];
        let whole = if l.edge > 0 { self.arcs[(l.edge - 1) as usize].length() } else { l.length };
        whole * (l.splits[p.j + 1].0 - l.splits[p.j].0)
    }
}

/// A point of a patch's mesh: on one of its sides (side, index in the
/// side's points), or one of the points made inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Vtx {
    Side(usize, usize),
    Inner(usize),
}

/// The rows of a patch whose two opposite sides differ (a fillet's rows
/// meeting a face cut more coarsely): rows of quads from the dense side,
/// each row dropping two segments per "3 into 1" step (three quads below,
/// one above, two points of three in between), so the rows end in T inside
/// the patch instead of running on across the face.
/// The fewest rows that take a side of `hi` segments down to `lo`.
fn rows_to(hi: usize, lo: usize, rows: usize) -> Option<Vec<usize>> {
    if lo == 0 || hi < lo || (hi - lo) % 2 != 0 {
        return None;
    }
    let mut n = vec![hi];
    let mut cur = hi;
    for _ in 0..rows {
        let t = ((cur - lo) / 2).min(cur / 3);
        cur -= 2 * t;
        n.push(cur);
    }
    (cur == lo).then_some(n)
}

/// May a patch with these side counts (bottom, right, top, left) be made
/// with rows ending in T? Only where one side is much denser than its
/// opposite (a fillet's rows), the other two equal.
fn transition_ok(c: [usize; 4], fillet: [bool; 4], relaxed: bool) -> bool {
    let one = |k: usize| {
        let (a, b, c2, d) = (c[k], c[(k + 1) % 4], c[(k + 2) % 4], c[(k + 3) % 4]);
        let (hi, lo) = (a.max(c2), a.min(c2));
        let dense = if a > c2 { fillet[k] } else { fillet[(k + 2) % 4] };
        dense && 2 * hi >= 3 * lo && b == d && hi != lo && rows_to(hi, lo, b).is_some()
    };
    one(0) || one(1) || (relaxed && (c[0] != c[2] || c[1] != c[3]) && layout_ok(c))
}

/// A point of a patch's boundary while laying it out: its name, and where
/// it is in the whole patch (s along the bottom, t up the sides).
type BPt = (Vtx, (f64, f64));

/// Points along a side by its params at fraction x (0..1 of its count).
fn side_at(side: &[BPt], x: f64) -> (f64, f64) {
    let n = side.len() - 1;
    let f = (x * n as f64).clamp(0.0, n as f64);
    let i = (f.floor() as usize).min(n.saturating_sub(1));
    let r = f - i as f64;
    let (a, b) = (side[i].1, side[(i + 1).min(n)].1);
    (a.0 + (b.0 - a.0) * r, a.1 + (b.1 - a.1) * r)
}

/// A point inside a part of a patch, from its four sides (bottom and top
/// by x, left and right by y; all in loop order: bottom left → right,
/// right bottom → top, top right → left, left top → bottom).
fn sub_coons(sd: &[Vec<BPt>; 4], x: f64, y: f64) -> (f64, f64) {
    let b = side_at(&sd[0], x);
    let t = side_at(&sd[2], 1.0 - x);
    let r = side_at(&sd[1], y);
    let l = side_at(&sd[3], 1.0 - y);
    let (c00, c10, c11, c01) = (sd[0][0].1, sd[1][0].1, sd[2][0].1, sd[3][0].1);
    let f = |k: usize| -> f64 {
        let g = |p: (f64, f64)| if k == 0 { p.0 } else { p.1 };
        (1.0 - y) * g(b) + y * g(t) + (1.0 - x) * g(l) + x * g(r) - ((1.0 - x) * (1.0 - y) * g(c00) + x * (1.0 - y) * g(c10) + x * y * g(c11) + (1.0 - x) * y * g(c01))
    };
    (f(0), f(1))
}

/// Lays out a part of a patch given its four sides (loop order): a grid
/// when opposite sides match, rows ending in T when one pair differs (by
/// an even number), and when both differ, cut in two (rows ending in T
/// one way on each side of the cut). Points made inside go to `inner`.
fn layout(sd: [Vec<BPt>; 4], inner: &mut Vec<(f64, f64)>, quads: &mut Vec<[Vtx; 4]>) -> Option<()> {
    let n: [usize; 4] = std::array::from_fn(|k| sd[k].len() - 1);
    if n.iter().any(|c| *c == 0) {
        return None;
    }
    let new_pt = |inner: &mut Vec<(f64, f64)>, p: (f64, f64)| {
        inner.push(p);
        Vtx::Inner(inner.len() - 1)
    };
    // a grid
    if n[0] == n[2] && n[1] == n[3] {
        let (a, m) = (n[0], n[1]);
        let mut grid = vec![vec![sd[0][0].0; m + 1]; a + 1];
        for i in 0..=a {
            for j in 0..=m {
                grid[i][j] = if j == 0 {
                    sd[0][i].0
                } else if j == m {
                    sd[2][a - i].0
                } else if i == 0 {
                    sd[3][m - j].0
                } else if i == a {
                    sd[1][j].0
                } else {
                    // discrete Coons: the lines through the sides' points
                    let (b, t, l, r) = (sd[0][i].1, sd[2][a - i].1, sd[3][m - j].1, sd[1][j].1);
                    let (u, v) = (i as f64 / a as f64, j as f64 / m as f64);
                    let (c00, c10, c11, c01) = (sd[0][0].1, sd[1][0].1, sd[2][0].1, sd[3][0].1);
                    let f = |k: usize| -> f64 {
                        let g = |p: (f64, f64)| if k == 0 { p.0 } else { p.1 };
                        (1.0 - v) * g(b) + v * g(t) + (1.0 - u) * g(l) + u * g(r) - ((1.0 - u) * (1.0 - v) * g(c00) + u * (1.0 - v) * g(c10) + u * v * g(c11) + (1.0 - u) * v * g(c01))
                    };
                    new_pt(inner, (f(0), f(1)))
                };
            }
        }
        for i in 0..a {
            for j in 0..m {
                quads.push([grid[i][j], grid[i + 1][j], grid[i + 1][j + 1], grid[i][j + 1]]);
            }
        }
        return Some(());
    }
    // one pair differs: turned so that it is bottom and top
    let one = |k: usize| n[(k + 1) % 4] == n[(k + 3) % 4] && n[k] != n[(k + 2) % 4];
    if one(0) || one(1) {
        let r = if one(0) { 0 } else { 1 };
        let sd: [Vec<BPt>; 4] = std::array::from_fn(|k| sd[(k + r) % 4].clone());
        let (a, m, c) = (sd[0].len() - 1, sd[1].len() - 1, sd[2].len() - 1);
        let counts = rows_to(a.max(c), a.min(c), m)?;
        let counts: Vec<usize> = if a > c { counts } else { counts.into_iter().rev().collect() };
        // the row lines: points and their places
        let mut lines: Vec<Vec<BPt>> = vec![];
        for j in 0..=m {
            let k = counts[j];
            let y = j as f64 / m as f64;
            let row: Vec<BPt> = (0..=k)
                .map(|i| {
                    if j == 0 {
                        sd[0][i]
                    } else if j == m {
                        sd[2][c - i]
                    } else if i == 0 {
                        sd[3][m - j]
                    } else if i == k {
                        sd[1][j]
                    } else {
                        let p = sub_coons(&sd, i as f64 / k as f64, y);
                        (new_pt(inner, p), p)
                    }
                })
                .collect();
            lines.push(row);
        }
        for j in 0..m {
            let (lo, hi) = (&lines[j], &lines[j + 1]);
            let (nl, nh) = (lo.len() - 1, hi.len() - 1);
            if nl == nh {
                for i in 0..nl {
                    quads.push([lo[i].0, lo[i + 1].0, hi[i + 1].0, hi[i].0]);
                }
                continue;
            }
            let flip = nh > nl;
            let (d, e) = if flip { (hi, lo) } else { (lo, hi) };
            let (nd, ne) = (d.len() - 1, e.len() - 1);
            let t = (nd - ne) / 2;
            let mut starts = vec![];
            let mut last: i64 = -3;
            for k in 0..t {
                let want = (((k as f64 + 0.5) * nd as f64 / t as f64) - 1.5).round() as i64;
                let st = want.max(last + 3).min(nd as i64 - 3 * (t - k) as i64);
                starts.push(st as usize);
                last = st;
            }
            let (mut id, mut ie) = (0, 0);
            while id < nd {
                let mut q4: Vec<[Vtx; 4]> = vec![];
                if starts.first() == Some(&id) {
                    starts.remove(0);
                    // two points half way across the row, over the dense
                    // line's inner points
                    let up = |k: usize| {
                        let (p, q) = (d[k].1, side_at(e, k as f64 / nd as f64));
                        (0.5 * (p.0 + q.0), 0.5 * (p.1 + q.1))
                    };
                    let (p0, p1) = (up(id + 1), up(id + 2));
                    let v0 = new_pt(inner, p0);
                    let v1 = new_pt(inner, p1);
                    q4.push([d[id].0, d[id + 1].0, v0, e[ie].0]);
                    q4.push([d[id + 1].0, d[id + 2].0, v1, v0]);
                    q4.push([d[id + 2].0, d[id + 3].0, e[ie + 1].0, v1]);
                    q4.push([v0, v1, e[ie + 1].0, e[ie].0]);
                    id += 3;
                    ie += 1;
                } else {
                    q4.push([d[id].0, d[id + 1].0, e[ie + 1].0, e[ie].0]);
                    id += 1;
                    ie += 1;
                }
                for mut q in q4 {
                    if flip {
                        q.reverse();
                    }
                    quads.push(q);
                }
            }
            if ie != ne {
                return None;
            }
        }
        return Some(());
    }
    // both pairs differ: a cut from bottom to top, w columns from the left;
    // on its left the left side's rows end in T, on its right the bottom's
    let (a, b, c, d) = (n[0], n[1], n[2], n[3]);
    if (a + c) % 2 != 0 || (b + d) % 2 != 0 {
        return None;
    }
    let w = (1..a.min(c)).find(|w| rows_to(b.max(d), b.min(d), *w).is_some() && rows_to((a - w).max(c - w), (a - w).min(c - w), b).is_some())?;
    let (pb, pt) = (sd[0][w], sd[2][c - w]);
    let mut cut: Vec<BPt> = vec![pb];
    for j in 1..b {
        let y = j as f64 / b as f64;
        let p = (pb.1 .0 + (pt.1 .0 - pb.1 .0) * y, pb.1 .1 + (pt.1 .1 - pb.1 .1) * y);
        let p = (0.5 * (p.0 + sub_coons(&sd, w as f64 / a as f64, y).0), 0.5 * (p.1 + sub_coons(&sd, w as f64 / a as f64, y).1));
        cut.push((new_pt(inner, p), p));
    }
    cut.push(pt);
    let back: Vec<BPt> = cut.iter().rev().copied().collect();
    let left = [sd[0][..=w].to_vec(), cut.clone(), sd[2][c - w..].to_vec(), sd[3].clone()];
    let right = [sd[0][w..].to_vec(), sd[1].clone(), sd[2][..=c - w].to_vec(), back];
    layout(left, inner, quads)?;
    layout(right, inner, quads)
}

/// May a patch with these side counts (bottom, right, top, left) be laid
/// out with rows ending in T?
fn layout_ok(n: [usize; 4]) -> bool {
    thread_local! {
        static SEEN: std::cell::RefCell<HashMap<[usize; 4], bool>> = std::cell::RefCell::new(HashMap::new());
    }
    if n.iter().any(|c| *c > 400) {
        return false;
    }
    if let Some(v) = SEEN.with(|m| m.borrow().get(&n).copied()) {
        return v;
    }
    let v = layout_ok_now(n);
    SEEN.with(|m| m.borrow_mut().insert(n, v));
    v
}

fn layout_ok_now(n: [usize; 4]) -> bool {
    let mut inner = vec![];
    let mut quads = vec![];
    let sd: [Vec<BPt>; 4] = std::array::from_fn(|k| (0..=n[k]).map(|i| (Vtx::Side(k, i), (0.0, 0.0))).collect());
    layout(sd, &mut inner, &mut quads).is_some()
}

/// A point of a patch from its four sides (Coons), each side given as its
/// points with their params: bottom and top by s, left and right by t.
fn coons<T: Copy>(s: f64, t: f64, side: &dyn Fn(usize, f64) -> T, mix: &dyn Fn(&[(f64, T)]) -> T) -> T {
    let (b, tp, l, r) = (side(0, s), side(2, s), side(3, t), side(1, t));
    let (c00, c10, c11, c01) = (side(0, 0.0), side(0, 1.0), side(2, 1.0), side(2, 0.0));
    mix(&[(1.0 - t, b), (t, tp), (1.0 - s, l), (s, r), (-(1.0 - s) * (1.0 - t), c00), (-s * (1.0 - t), c10), (-s * t, c11), (-(1.0 - s) * t, c01)])
}

/// The quads of a solid; `target` is the length of a segment (mm).
pub fn convert(shape: &Shape, target: f64) -> Result<QuadMesh, String> {
    let topo = shape.quad_topo()?;
    let arcs: Vec<Arc> = topo.edges.iter().map(|e| Arc::new(&e.pts, e.t)).collect();
    // a node for each B-rep vertex, then one for each new point
    let mut nodes = topo.edges.iter().flat_map(|e| e.v).max().unwrap_or(0).max(0) as usize;
    let mut lines = vec![];
    for (i, e) in topo.edges.iter().enumerate() {
        let mut end = |v: i32| {
            if v > 0 {
                (v - 1) as usize
            } else {
                nodes += 1;
                nodes - 1
            }
        };
        let (a, b) = (end(e.v[0]), end(e.v[1]));
        lines.push(Line { edge: i as i32 + 1, uv: [[0.0; 2]; 2], length: 0.0, splits: vec![(0.0, a), (1.0, b)] });
    }
    let mut cx = Ctx { shape, topo: &topo, arcs, lines, nodes, polys: HashMap::new(), target };
    let mut mesh = QuadMesh::default();

    // 1. each face cut into four-sided regions (all faces first: a face
    // may add nodes on the edges of the faces done before it)
    let mut regions: Vec<(usize, Vec<Span>, Vec<usize>)> = vec![];
    for fi in 0..topo.faces.len() {
        match cx.face_regions(fi) {
            Ok(rs) => regions.extend(rs.into_iter().map(|(lp, cs)| (fi, lp, cs))),
            Err(e) => mesh.skipped.push((fi, e)),
        }
    }
    let mut patches: Vec<Patch> = vec![];
    for (fi, lp, cs) in regions {
        let mut sides: [Vec<(Piece, bool, bool)>; 4] = Default::default();
        let n = lp.len();
        for r in 0..4 {
            let (k0, k1) = (cs[r], cs[(r + 1) % 4]);
            let mut k = (k0 + 1) % n;
            loop {
                sides[r].extend(cx.pieces(lp[k]));
                if k == k1 {
                    break;
                }
                k = (k + 1) % n;
            }
        }
        let fillet = std::array::from_fn(|k| {
            sides[k].iter().any(|(p, _, _)| {
                let e = cx.lines[p.line].edge;
                e > 0 && {
                    let te = &topo.edges[(e - 1) as usize];
                    te.kind == 1 && te.radius > 0.0 && cx.arcs[(e - 1) as usize].length() / te.radius < 100f64.to_radians()
                }
            })
        });
        patches.push(Patch { face: fi, sides, fillet });
    }

    // 2. segments per piece: by length (an arc by angle), then raised until
    // opposite sides match; a face whose counts cannot settle is left out
    let mut count: HashMap<Piece, usize> = HashMap::new();
    // the counts all together first (Romain: "calculer tous les nombres de
    // segments ensemble"): real numbers near their targets that make
    // opposite sides equal (projected constraint by constraint, a
    // fillet's arc never under its rows), then rounded; the whole numbers
    // are then evened out below
    let mut want: HashMap<Piece, f64> = HashMap::new();
    let mut least: HashMap<Piece, f64> = HashMap::new();
    for pt in &patches {
        for (p, _, _) in pt.sides.iter().flatten() {
            if cx.is_pole(p.line) || want.contains_key(p) {
                continue;
            }
            let l = cx.piece_len(*p);
            let line = &cx.lines[p.line];
            let mut lo = 1.0f64;
            if line.edge > 0 {
                let e = &topo.edges[(line.edge - 1) as usize];
                if e.kind == 1 && e.radius > 0.0 {
                    lo = lo.max((l / e.radius / ARC_STEP).round());
                }
            }
            want.insert(*p, (l / target).max(lo));
            least.insert(*p, lo);
        }
    }
    let goal = want.clone();
    for _ in 0..400 {
        let mut worst = 0.0f64;
        for pt in &patches {
            let sum = |k: usize, w: &HashMap<Piece, f64>| pt.sides[k].iter().filter(|x| w.contains_key(&x.0)).map(|x| w[&x.0]).sum::<f64>();
            for (a, b) in [(0usize, 2usize), (1, 3)] {
                let (sa, sb) = (sum(a, &want), sum(b, &want));
                // a fillet's end much denser than the side across: rows
                // will end in T there, no need to match
                let (hi, lo) = if sa > sb { (a, sb) } else { (b, sa) };
                if pt.fillet[hi] && sa.max(sb) >= 1.5 * lo {
                    continue;
                }
                let r = sa - sb;
                worst = worst.max(r.abs());
                let ps: Vec<(Piece, f64)> = pt.sides[a].iter().map(|x| (x.0, 1.0)).chain(pt.sides[b].iter().map(|x| (x.0, -1.0))).filter(|x| want.contains_key(&x.0)).collect();
                let wsum: f64 = ps.iter().map(|(p, _)| goal[p]).sum();
                if wsum <= 0.0 {
                    continue;
                }
                for (p, sgn) in ps {
                    let v = want.get_mut(&p).unwrap();
                    *v = (*v - sgn * r * goal[&p] / wsum).max(least[&p]);
                }
            }
        }
        if worst < 0.05 {
            break;
        }
    }

    // faces whose counts go round in a circle (a fillet's rows coming back
    // to where they started): their rows may end in T anywhere
    let mut relaxed: HashSet<usize> = HashSet::new();
    // after a first try, every count even: a patch's sides then always add
    // up to an even number, which rows ending in T need
    let mut even = false;
    for _round in 0..6 {
        count.clear();
        for pt in &patches {
            for (p, _, _) in pt.sides.iter().flatten() {
                if cx.is_pole(p.line) {
                    count.insert(*p, 0);
                    continue;
                }
                count.entry(*p).or_insert_with(|| {
                    let n = want.get(p).map(|w| w.round() as usize).unwrap_or(1).max(least.get(p).copied().unwrap_or(1.0) as usize);
                    if even {
                        2 * ((n as f64 / 2.0).round() as usize).max(1)
                    } else {
                        n.max(1)
                    }
                });
            }
        }
        if std::env::var("FORME_QUADS_DEBUG").is_ok() {
            for pt in &patches {
                let d: Vec<String> = pt.sides.iter().map(|side| side.iter().map(|(p, r, _)| format!("{}{}.{}{}={}({:.1})", if cx.lines[p.line].edge > 0 { "e" } else { "c" }, if cx.lines[p.line].edge > 0 { cx.lines[p.line].edge as usize } else { p.line }, p.j, if *r { "r" } else { "" }, count[p], cx.piece_len(*p))).collect::<Vec<_>>().join("+")).collect();
                eprintln!("  start face {}: {}", pt.face, d.join(" | "));
            }
        }
        let mut settled = false;
        for _ in 0..3000 {
            let mut changed = false;
            for pt in &patches {
                let sum4 = |k: usize| pt.sides[k].iter().map(|(p, _, _)| count[p]).sum::<usize>();
                if transition_ok([sum4(0), sum4(1), sum4(2), sum4(3)], pt.fillet, relaxed.contains(&pt.face)) {
                    continue;
                }
                for (a, b) in [(0, 2), (1, 3)] {
                    let sum = |k: usize| pt.sides[k].iter().map(|(p, _, _)| count[p]).sum::<usize>();
                    let (na, nb) = (sum(a), sum(b));
                    if na != nb {
                        let short = if na < nb { a } else { b };
                        // the coarsest piece of the short side gets one more
                        let Some((p, _, _)) = pt.sides[short].iter().filter(|x| !cx.is_pole(x.0.line)).max_by(|x, y| (cx.piece_len(x.0) / count[&x.0] as f64).total_cmp(&(cx.piece_len(y.0) / count[&y.0] as f64))) else {
                            continue;
                        };
                        *count.get_mut(p).unwrap() += if even { 2 } else { 1 };
                        changed = true;
                    }
                }
            }
            if !changed {
                settled = true;
                break;
            }
        }
        if settled {
            if std::env::var("FORME_QUADS_DEBUG").is_ok() {
                let mut v: Vec<(&Piece, &usize)> = count.iter().collect();
                v.sort_by_key(|x| (x.0.line, x.0.j));
                for (p, c) in v {
                    let l = cx.piece_len(*p);
                    if *c as f64 > 1.6 * (l / target).max(1.0) && !cx.is_pole(p.line) {
                        eprintln!("  dense: line {} (edge {}) piece {} = {} for {:.1} mm", p.line, cx.lines[p.line].edge, p.j, c, l);
                    }
                }
            }
            break;
        }
        // the faces still off are left out, and the counts made again
        let mut off: HashSet<usize> = HashSet::new();
        for pt in &patches {
            let sum = |k: usize| pt.sides[k].iter().map(|(p, _, _)| count[p]).sum::<usize>();
            if (sum(0) != sum(2) || sum(1) != sum(3)) && !transition_ok([sum(0), sum(1), sum(2), sum(3)], pt.fillet, relaxed.contains(&pt.face)) {
                off.insert(pt.face);
            }
        }
        if std::env::var("FORME_QUADS_DEBUG").is_ok() {
            eprintln!("counts do not settle on faces {off:?}");
            for pt in patches.iter().filter(|p| off.contains(&p.face)) {
                let d: Vec<String> = pt.sides.iter().map(|side| side.iter().map(|(p, r, _)| format!("{}{}.{}{}={}({:.1})", if cx.lines[p.line].edge > 0 { "e" } else { "c" }, if cx.lines[p.line].edge > 0 { cx.lines[p.line].edge as usize } else { p.line }, p.j, if *r { "r" } else { "" }, count[p], cx.piece_len(*p))).collect::<Vec<_>>().join("+")).collect();
                eprintln!("  face {}: {}", pt.face, d.join(" | "));
            }
        }
        if !off.is_subset(&relaxed) || !even {
            even = true;
            if std::env::var("FORME_QUADS_DEBUG").is_ok() {
                eprintln!("rows may end in T on faces {off:?}");
            }
            relaxed.extend(off.iter().copied());
            continue;
        }
        for f in &off {
            mesh.skipped.push((*f, "the segment counts do not settle".into()));
        }
        patches.retain(|p| !off.contains(&p.face));
        if off.is_empty() {
            return Err("the segment counts do not settle".into());
        }
    }

    // 3. the points: shared along the lines, a Coons grid inside each patch
    let mut index: HashMap<Key, u32> = HashMap::new();
    let mut add = |mesh: &mut QuadMesh, key: Key, p: [f64; 3]| -> u32 {
        *index.entry(key).or_insert_with(|| {
            mesh.positions.push(p);
            (mesh.positions.len() - 1) as u32
        })
    };
    let mut failed_faces: HashSet<usize> = HashSet::new();
    let mut t_faces: HashSet<usize> = HashSet::new();
    let pole_faces: HashSet<usize> = (0..topo.faces.len()).filter(|f| topo.faces[*f].wires.iter().flatten().any(|(e, _)| topo.edges[(*e - 1) as usize].degenerated)).collect();
    let mut out_quads: Vec<([u32; 4], usize)> = vec![];
    for pt in &patches {
        let fi = pt.face;
        if failed_faces.contains(&fi) {
            continue;
        }
        let mut side_pts: Vec<Vec<(u32, V2)>> = vec![];
        let mut failed = None;
        for side in pt.sides.iter() {
            let mut pts: Vec<(u32, V2)> = vec![];
            for &(p, rev, pc) in side {
                let n = count[&p];
                let line = &cx.lines[p.line];
                let (fa, fb) = (line.splits[p.j].0, line.splits[p.j + 1].0);
                let fr: Vec<f64> = (0..=n).map(|i| fa + (fb - fa) * i as f64 / n as f64).collect();
                let got = if line.edge > 0 {
                    let arc = &cx.arcs[(line.edge - 1) as usize];
                    let ts: Vec<f64> = fr.iter().map(|f| arc.param(*f)).collect();
                    shape.edge_on_face(fi, line.edge, pc, &ts)
                } else {
                    let uv: Vec<V2> = fr.iter().map(|f| lerp2(line.uv[0], line.uv[1], *f)).collect();
                    shape.face_eval(fi, &uv).map(|e| (e.into_iter().map(|x| x.0).collect(), uv))
                };
                let (xyz, uv) = match got {
                    Ok(r) => r,
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                };
                let keys: Vec<Key> = (0..=n).map(|i| if i == 0 { Key::Node(line.splits[p.j].1) } else if i == n { Key::Node(line.splits[p.j + 1].1) } else { Key::Inner(p, i) }).collect();
                let mut ids: Vec<(u32, V2)> = (0..=n).map(|i| (add(&mut mesh, keys[i], xyz[i]), uv[i])).collect();
                if rev {
                    ids.reverse();
                }
                // the first point is the previous piece's last one
                if !pts.is_empty() {
                    ids.remove(0);
                }
                pts.extend(ids);
            }
            if failed.is_some() {
                break;
            }
            side_pts.push(pts);
        }
        if let Some(e) = failed {
            mesh.skipped.push((fi, e));
            failed_faces.insert(fi);
            continue;
        }
        let nn = side_pts[0].len() - 1;
        let mm = side_pts[1].len() - 1;
        let cnt = [nn, mm, side_pts[2].len() - 1, side_pts[3].len() - 1];
        if cnt.iter().all(|c| *c > 0) && (cnt[0] != cnt[2] || cnt[1] != cnt[3]) {
            // rows ending in T: the sides as curves of (u, v, x, y, z) by
            // length in space
            let pos = |id: u32| mesh.positions[id as usize];
            let par: [Vec<f64>; 4] = std::array::from_fn(|k| {
                let mut c = vec![0.0];
                for w in side_pts[k].windows(2) {
                    c.push(c.last().unwrap() + len(sub(pos(w[1].0), pos(w[0].0))));
                }
                let t = c.last().copied().unwrap_or(1.0).max(1e-12);
                c.iter().map(|x| x / t).collect()
            });
            let sd: [Vec<BPt>; 4] = std::array::from_fn(|k| {
                par[k]
                    .iter()
                    .enumerate()
                    .map(|(i, x)| (Vtx::Side(k, i), match k { 0 => (*x, 0.0), 1 => (1.0, *x), 2 => (1.0 - x, 1.0), _ => (0.0, 1.0 - x) }))
                    .collect()
            });
            let (mut inner, mut qs) = (vec![], vec![]);
            t_faces.insert(fi);
            if layout(sd, &mut inner, &mut qs).is_none() {
                mesh.skipped.push((fi, "opposite sides differ".into()));
                failed_faces.insert(fi);
                continue;
            }
            let curves: Vec<Vec<(f64, [f64; 5])>> = (0..4)
                .map(|k| {
                    let sd = k;
                    let mut c: Vec<(f64, [f64; 5])> = side_pts[sd].iter().zip(&par[sd]).map(|((id, uv), x)| {
                        let p = pos(*id);
                        (*x, [uv[0], uv[1], p[0], p[1], p[2]])
                    }).collect();
                    if k >= 2 {
                        c.reverse();
                        for e in c.iter_mut() {
                            e.0 = 1.0 - e.0;
                        }
                    }
                    c
                })
                .collect();
            let at = |k: usize, x: f64| -> [f64; 5] {
                let c = &curves[k];
                let i = c.partition_point(|e| e.0 < x).clamp(1, c.len() - 1);
                let (a, b) = (c[i - 1], c[i]);
                let f = if b.0 - a.0 > 1e-12 { ((x - a.0) / (b.0 - a.0)).clamp(0.0, 1.0) } else { 0.0 };
                std::array::from_fn(|q| a.1[q] + (b.1[q] - a.1[q]) * f)
            };
            let mix = |w: &[(f64, [f64; 5])]| -> [f64; 5] { std::array::from_fn(|q| w.iter().map(|(k, v)| k * v[q]).sum()) };
            let made: Vec<[f64; 5]> = inner.iter().map(|(x, t)| coons(*x, *t, &at, &mix)).collect();
            // on the surface
            let got = if pole_faces.contains(&fi) {
                shape.face_project(fi, &made.iter().map(|m| [m[2], m[3], m[4]]).collect::<Vec<_>>()).map(|v| v.into_iter().map(|(_, p, n)| (p, n)).collect::<Vec<_>>())
            } else {
                shape.face_eval(fi, &made.iter().map(|m| [m[0], m[1]]).collect::<Vec<_>>())
            };
            let got = match got {
                Ok(g) => g,
                Err(e) => {
                    mesh.skipped.push((fi, e));
                    failed_faces.insert(fi);
                    continue;
                }
            };
            let first = mesh.positions.len() as u32;
            mesh.positions.extend(got.iter().map(|g| g.0));
            let id = |v: Vtx| match v {
                Vtx::Side(k, i) => side_pts[k][i].0,
                Vtx::Inner(i) => first + i as u32,
            };
            // turned outwards: the surface's normal at the side points
            let uvs: Vec<V2> = side_pts.iter().flatten().map(|x| x.1).collect();
            let normals: HashMap<u32, [f64; 3]> = match shape.face_eval(fi, &uvs) {
                Ok(e) => side_pts.iter().flatten().zip(e).map(|(x, (_, n))| (x.0, n)).chain(got.iter().enumerate().map(|(i, g)| (first + i as u32, g.1))).collect(),
                Err(e) => {
                    mesh.skipped.push((fi, e));
                    failed_faces.insert(fi);
                    continue;
                }
            };
            let qs: Vec<[u32; 4]> = qs.iter().map(|q| [id(q[0]), id(q[1]), id(q[2]), id(q[3])]).collect();
            let mut agree = 0.0;
            for q in &qs {
                let p = |k: usize| mesh.positions[q[k] as usize];
                let n = cross(sub(p(2), p(0)), sub(p(3), p(1)));
                let s = q.iter().map(|v| normals.get(v).copied().unwrap_or([0.0; 3])).fold([0.0; 3], |x, y| [x[0] + y[0], x[1] + y[1], x[2] + y[2]]);
                agree += dot(n, s);
            }
            for mut q in qs {
                if agree < 0.0 {
                    q.reverse();
                }
                out_quads.push((q, fi));
            }
            continue;
        }
        if nn == 0 || mm == 0 || side_pts[2].len() - 1 != nn || side_pts[3].len() - 1 != mm {
            mesh.skipped.push((fi, "opposite sides differ".into()));
            failed_faces.insert(fi);
            continue;
        }
        let bottom = |i: usize| side_pts[0][i];
        let right = |j: usize| side_pts[1][j];
        let top = |i: usize| side_pts[2][nn - i];
        let left = |j: usize| side_pts[3][mm - j];
        let (c00, c10, c11, c01) = (bottom(0).1, bottom(nn).1, top(nn).1, top(0).1);
        let mut grid = vec![vec![(0u32, [0.0f64; 2]); mm + 1]; nn + 1];
        let mut inner = vec![];
        for i in 0..=nn {
            for j in 0..=mm {
                let boundary = if j == 0 {
                    Some(bottom(i))
                } else if j == mm {
                    Some(top(i))
                } else if i == 0 {
                    Some(left(j))
                } else if i == nn {
                    Some(right(j))
                } else {
                    None
                };
                grid[i][j] = match boundary {
                    Some(b) => b,
                    None => {
                        let (u, v) = (i as f64 / nn as f64, j as f64 / mm as f64);
                        let (b, t, l, r) = (bottom(i).1, top(i).1, left(j).1, right(j).1);
                        let c = |k: usize| (1.0 - v) * b[k] + v * t[k] + (1.0 - u) * l[k] + u * r[k] - ((1.0 - u) * (1.0 - v) * c00[k] + u * (1.0 - v) * c10[k] + (1.0 - u) * v * c01[k] + u * v * c11[k]);
                        inner.push((i, j));
                        (u32::MAX, [c(0), c(1)])
                    }
                };
            }
        }
        // on the surface: the inner points, and every point's normal (a
        // face closing on a pole: the grid made in space from the sides'
        // points, then put on the surface; its (u, v) squeeze at the pole)
        let all: Vec<V2> = (0..=nn).flat_map(|i| (0..=mm).map(move |j| (i, j))).map(|(i, j)| grid[i][j].1).collect();
        let eval = if pole_faces.contains(&fi) {
            let p3 = |id: u32| mesh.positions[id as usize];
            let (b3, t3, l3, r3) = (|i: usize| p3(bottom(i).0), |i: usize| p3(top(i).0), |j: usize| p3(left(j).0), |j: usize| p3(right(j).0));
            let (q00, q10, q11, q01) = (b3(0), b3(nn), t3(nn), t3(0));
            let pts: Vec<[f64; 3]> = (0..=nn)
                .flat_map(|i| (0..=mm).map(move |j| (i, j)))
                .map(|(i, j)| {
                    if grid[i][j].0 != u32::MAX {
                        return p3(grid[i][j].0);
                    }
                    let (u, v) = (i as f64 / nn as f64, j as f64 / mm as f64);
                    let (b, t, l, r) = (b3(i), t3(i), l3(j), r3(j));
                    let c = |k: usize| (1.0 - v) * b[k] + v * t[k] + (1.0 - u) * l[k] + u * r[k] - ((1.0 - u) * (1.0 - v) * q00[k] + u * (1.0 - v) * q10[k] + (1.0 - u) * v * q01[k] + u * v * q11[k]);
                    [c(0), c(1), c(2)]
                })
                .collect();
            shape.face_project(fi, &pts).map(|v| v.into_iter().map(|(_, p, n)| (p, n)).collect::<Vec<_>>())
        } else {
            shape.face_eval(fi, &all)
        };
        let eval = match eval {
            Ok(e) => e,
            Err(e) => {
                mesh.skipped.push((fi, e));
                failed_faces.insert(fi);
                continue;
            }
        };
        for &(i, j) in &inner {
            mesh.positions.push(eval[i * (mm + 1) + j].0);
            grid[i][j].0 = (mesh.positions.len() - 1) as u32;
        }
        // the quads, turned to face outwards
        let mut qs = vec![];
        let mut agree = 0.0;
        for i in 0..nn {
            for j in 0..mm {
                let q = [grid[i][j].0, grid[i + 1][j].0, grid[i + 1][j + 1].0, grid[i][j + 1].0];
                let p = |k: usize| mesh.positions[q[k] as usize];
                let n = cross(sub(p(2), p(0)), sub(p(3), p(1)));
                let s = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)].iter().map(|(a, b)| eval[a * (mm + 1) + b].1).fold([0.0; 3], |x, y| [x[0] + y[0], x[1] + y[1], x[2] + y[2]]);
                agree += dot(n, s);
                qs.push(q);
            }
        }
        for mut q in qs {
            if agree < 0.0 {
                q.reverse();
            }
            out_quads.push((q, fi));
        }
    }
    for (q, fi) in out_quads {
        if !failed_faces.contains(&fi) {
            mesh.quads.push(q);
            mesh.quad_face.push(fi as u32);
        }
    }
    // 4. smoothed: each point inside a face moved towards the middle of its
    // neighbours, then put back on the surface (the points on the B-rep
    // edges stay); a face that comes out worse keeps its points
    let edge_nodes: HashSet<usize> = cx.lines.iter().filter(|l| l.edge > 0).flat_map(|l| l.splits.iter().map(|x| x.1)).collect();
    let mut fixed = vec![false; mesh.positions.len()];
    for (k, id) in &index {
        let on_edge = match k {
            Key::Node(n) => edge_nodes.contains(n),
            Key::Inner(p, _) => cx.lines[p.line].edge > 0,
        };
        if on_edge {
            fixed[*id as usize] = true;
        }
    }
    // (flat faces, faces closing on a pole and faces with rows ending in
    // T: a curved face's grid in (u, v) is already even)
    let mut want: HashSet<usize> = pole_faces.clone();
    want.extend(t_faces.iter().copied());
    want.extend((0..topo.faces.len()).filter(|f| shape.face_info(*f).map(|i| i.kind == forme_occt::SurfKind::Plane).unwrap_or(false)));
    smooth(&mut mesh, shape, &fixed, &want);

    // faces in face order, so each face's quads are together
    let mut order: Vec<usize> = (0..mesh.quads.len()).collect();
    order.sort_by_key(|k| mesh.quad_face[*k]);
    mesh.quads = order.iter().map(|k| mesh.quads[*k]).collect();
    mesh.quad_face = order.iter().map(|k| mesh.quad_face[*k]).collect();
    mesh.skipped.sort_by_key(|s| s.0);
    mesh.skipped.dedup_by_key(|s| s.0);
    Ok(mesh)
}

/// Smooths the points inside each face (not `fixed`): a few steps half way
/// to the middle of their neighbours, put back on the surface each time.
/// A face whose quads come out worse (more squashed) keeps its points.
fn smooth(mesh: &mut QuadMesh, shape: &Shape, fixed: &[bool], want: &HashSet<usize>) {
    let n = mesh.positions.len();
    let mut nb: Vec<Vec<u32>> = vec![vec![]; n];
    let mut face_of: Vec<Option<usize>> = vec![None; n];
    let mut quads_of: HashMap<usize, Vec<usize>> = HashMap::new();
    for (k, (q, f)) in mesh.quads.iter().zip(&mesh.quad_face).enumerate() {
        quads_of.entry(*f as usize).or_default().push(k);
        for i in 0..4 {
            let (a, b) = (q[i] as usize, q[(i + 1) % 4]);
            if !nb[a].contains(&b) {
                nb[a].push(b);
            }
            let (c, d) = (q[(i + 1) % 4] as usize, q[i]);
            if !nb[c].contains(&d) {
                nb[c].push(d);
            }
            face_of[a] = Some(*f as usize);
        }
    }
    let bad_of = |mesh: &QuadMesh, ks: &[usize]| -> (usize, f64) {
        let sub = QuadMesh { positions: mesh.positions.clone(), quads: ks.iter().map(|k| mesh.quads[*k]).collect(), quad_face: vec![], skipped: vec![] };
        (sub.bad_quads(20.0), sub.min_angle())
    };
    let mut faces: Vec<usize> = quads_of.keys().copied().filter(|f| want.contains(f)).collect();
    faces.sort();
    for f in faces {
        let ks = &quads_of[&f];
        let pts: Vec<usize> = (0..n).filter(|v| !fixed[*v] && face_of[*v] == Some(f) && !nb[*v].is_empty()).collect();
        if pts.is_empty() {
            continue;
        }
        let before = bad_of(mesh, ks);
        let keep: Vec<[f64; 3]> = pts.iter().map(|v| mesh.positions[*v]).collect();
        let mut ok = true;
        for _ in 0..8 {
            let moved: Vec<[f64; 3]> = pts
                .iter()
                .map(|v| {
                    let p = mesh.positions[*v];
                    let m = nb[*v].iter().fold([0.0; 3], |a, w| {
                        let q = mesh.positions[*w as usize];
                        [a[0] + q[0], a[1] + q[1], a[2] + q[2]]
                    });
                    let k = 1.0 / nb[*v].len() as f64;
                    [p[0] + 0.5 * (m[0] * k - p[0]), p[1] + 0.5 * (m[1] * k - p[1]), p[2] + 0.5 * (m[2] * k - p[2])]
                })
                .collect();
            match shape.face_project(f, &moved) {
                Ok(on) => {
                    for (v, (_, p, _)) in pts.iter().zip(on) {
                        mesh.positions[*v] = p;
                    }
                }
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        let after = bad_of(mesh, ks);
        if !ok || after.0 > before.0 || (after.0 == before.0 && after.1 < before.1 - 1e-9) {
            for (v, p) in pts.iter().zip(keep) {
                mesh.positions[*v] = p;
            }
        }
    }
}

impl QuadMesh {
    /// Wavefront OBJ: one group per face.
    pub fn obj(&self, face_name: impl Fn(usize) -> String) -> String {
        let mut s = String::from("# Forme NURBS: quads from the B-rep\n");
        for p in &self.positions {
            s.push_str(&format!("v {:.5} {:.5} {:.5}\n", p[0], p[1], p[2]));
        }
        let mut last = u32::MAX;
        for (q, f) in self.quads.iter().zip(&self.quad_face) {
            if *f != last {
                s.push_str(&format!("g {}\n", face_name(*f as usize)));
                last = *f;
            }
            s.push_str(&format!("f {} {} {} {}\n", q[0] + 1, q[1] + 1, q[2] + 1, q[3] + 1));
        }
        s
    }

    /// Every edge of the quads is used by exactly two of them.
    pub fn is_closed(&self) -> bool {
        let mut uses: HashMap<(u32, u32), i32> = HashMap::new();
        for q in &self.quads {
            for k in 0..4 {
                let (a, b) = (q[k], q[(k + 1) % 4]);
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        uses.values().all(|n| *n == 2)
    }

    /// The smallest angle at a corner of a quad (degrees).
    pub fn min_angle(&self) -> f64 {
        let p = |i: u32| self.positions[i as usize];
        let mut m = 180.0f64;
        for q in &self.quads {
            for k in 0..4 {
                let (a, b, c) = (p(q[(k + 3) % 4]), p(q[k]), p(q[(k + 1) % 4]));
                let (u, v) = (sub(a, b), sub(c, b));
                m = m.min((dot(u, v) / (len(u) * len(v)).max(1e-30)).clamp(-1.0, 1.0).acos().to_degrees());
            }
        }
        m
    }

    /// The quads that are folded or squashed: an angle at a corner under
    /// `min_deg` degrees, or a corner turning backwards.
    pub fn bad_quads(&self, min_deg: f64) -> usize {
        let p = |i: u32| self.positions[i as usize];
        self.quads
            .iter()
            .filter(|q| {
                let n = cross(sub(p(q[2]), p(q[0])), sub(p(q[3]), p(q[1])));
                (0..4).any(|k| {
                    let (a, b, c) = (p(q[(k + 3) % 4]), p(q[k]), p(q[(k + 1) % 4]));
                    let (u, v) = (sub(a, b), sub(c, b));
                    let ang = (dot(u, v) / (len(u) * len(v)).max(1e-30)).clamp(-1.0, 1.0).acos();
                    ang < min_deg.to_radians() || dot(cross(v, u), n) < 0.0
                })
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge_between(s: &Shape, a: [f64; 3], b: [f64; 3]) -> i32 {
        // the edge whose middle sample is nearest the middle of a and b
        let m = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5];
        let d = |e: &forme_occt::Edge| len(sub(e.pts[e.pts.len() / 2], m));
        s.edges(9).iter().min_by(|x, y| d(x).total_cmp(&d(y))).unwrap().index
    }

    fn moved(s: Shape, t: [f64; 3]) -> Shape {
        s.transformed([[1.0, 0.0, 0.0, t[0]], [0.0, 1.0, 0.0, t[1]], [0.0, 0.0, 1.0, t[2]]]).unwrap()
    }

    /// Converts, writes the OBJ, checks it is closed with `holes` holes
    /// through it (V − F = 2 − 2 × holes, since every edge has two quads).
    fn check(name: &str, s: &Shape, holes: i64) -> QuadMesh {
        let q = convert(s, 2.5).unwrap();
        std::fs::write(std::env::temp_dir().join(format!("forme-quads-{name}.obj")), q.obj(|f| format!("face{f}"))).unwrap();
        assert!(q.skipped.is_empty(), "{name}: {:?}", q.skipped);
        assert!(q.is_closed(), "{name}: every quad edge is shared by two quads");
        assert_eq!(q.positions.len() as i64 - q.quads.len() as i64, 2 - 2 * holes, "{name}: V − F");
        q
    }

    #[test]
    fn cube_with_fillets() {
        // a 20 mm cube (centred), one edge filleted at 4 mm
        let cube = Shape::cuboid(20.0, 20.0, 20.0).unwrap();
        let e = edge_between(&cube, [-10.0, 10.0, 10.0], [10.0, 10.0, 10.0]);
        let one = cube.fillet(&[e], 4.0, 99).unwrap();
        let q = check("one", &one, 0);
        // the fillet's quads lie on its circle (radius 4 around its axis)
        let fillet_face = (0..one.face_count()).find(|f| one.face_info(*f).map(|i| i.kind == forme_occt::SurfKind::Cylinder).unwrap_or(false)).unwrap();
        let info = one.face_info(fillet_face).unwrap();
        for (_, c) in q.quads.iter().enumerate().filter(|(k, _)| q.quad_face[*k] as usize == fillet_face) {
            for v in c {
                let d = sub(q.positions[*v as usize], info.axis_origin);
                let along = dot(d, info.axis_dir);
                let r = len(sub(d, [info.axis_dir[0] * along, info.axis_dir[1] * along, info.axis_dir[2] * along]));
                assert!((r - 4.0).abs() < 1e-6, "{d:?} at {r}");
            }
        }
        assert_eq!(q.bad_quads(20.0), 0, "one: no squashed quads");

        // the four upright edges filleted: top and bottom rounded all round
        let ups: Vec<i32> = [(-10.0, -10.0), (10.0, -10.0), (10.0, 10.0), (-10.0, 10.0)].iter().map(|(x, z)| edge_between(&cube, [*x, -10.0, *z], [*x, 10.0, *z])).collect();
        let four = cube.fillet(&ups, 4.0, 99).unwrap();
        let q = check("four", &four, 0);
        assert_eq!(q.bad_quads(20.0), 0, "four: no squashed quads");
    }

    #[test]
    fn holes_and_inner_corners() {
        // a plate with a round hole through it
        let plate = Shape::cuboid(40.0, 10.0, 30.0).unwrap().cut(&Shape::cylinder(5.0, 30.0).unwrap()).unwrap();
        let q = check("hole", &plate, 1);
        assert_eq!(q.bad_quads(15.0), 0, "hole: no squashed quads");
        // an L: a block with a corner taken out
        let l = Shape::cuboid(20.0, 10.0, 20.0).unwrap().cut(&moved(Shape::cuboid(10.0, 20.0, 10.0).unwrap(), [5.0, 0.0, 5.0])).unwrap();
        let q = check("l", &l, 0);
        assert_eq!(q.bad_quads(20.0), 0, "L: no squashed quads");
        // a U: a slot across the top
        let u = Shape::cuboid(40.0, 20.0, 30.0).unwrap().cut(&moved(Shape::cuboid(8.0, 8.0, 40.0).unwrap(), [0.0, 8.0, 0.0])).unwrap();
        let q = check("u", &u, 0);
        assert_eq!(q.bad_quads(20.0), 0, "U: no squashed quads");
        // two holes side by side
        let two = Shape::cuboid(50.0, 10.0, 30.0).unwrap().cut(&moved(Shape::cylinder(4.0, 30.0).unwrap(), [-12.0, 0.0, 3.0])).unwrap().cut(&moved(Shape::cylinder(5.0, 30.0).unwrap(), [12.0, 0.0, -2.0])).unwrap();
        let q = check("two-holes", &two, 2);
        assert_eq!(q.bad_quads(15.0), 0, "two holes: no squashed quads");
    }

    #[test]
    fn discs() {
        // a cylinder: its two discs as a square and four patches round it
        let cyl = Shape::cylinder(6.0, 10.0).unwrap();
        let q = check("cylinder", &cyl, 0);
        assert_eq!(q.bad_quads(20.0), 0, "cylinder: no squashed quads");
    }

    #[test]
    fn several_holes() {
        // holes in a row, in a grid, and side by side on a diagonal: the
        // face is first cut between them, one hole per region
        let cyl = |r: f64, x: f64, z: f64| moved(Shape::cylinder(r, 30.0).unwrap(), [x, 0.0, z]);
        let three = Shape::cuboid(70.0, 10.0, 30.0).unwrap().cut(&cyl(4.0, -22.0, 0.0)).unwrap().cut(&cyl(5.0, 0.0, 3.0)).unwrap().cut(&cyl(4.0, 22.0, -2.0)).unwrap();
        let q = check("three", &three, 3);
        assert_eq!(q.bad_quads(15.0), 0, "three holes: no squashed quads");
        let mut grid = Shape::cuboid(50.0, 10.0, 50.0).unwrap();
        for (x, z) in [(-12.0, -12.0), (12.0, -12.0), (-12.0, 12.0), (12.0, 12.0)] {
            grid = grid.cut(&cyl(5.0, x, z)).unwrap();
        }
        let q = check("grid", &grid, 4);
        assert_eq!(q.bad_quads(15.0), 0, "four holes: no squashed quads");
        let diag = Shape::cuboid(40.0, 10.0, 40.0).unwrap().cut(&cyl(5.0, -4.5, -4.5)).unwrap().cut(&cyl(5.0, 4.5, 4.5)).unwrap();
        check("diag", &diag, 2);
    }

    #[test]
    fn the_test_part() {
        use crate::doc::{Doc, EdgeRef, Op, Prim};
        use crate::model::FaceTag;
        use glam::{DQuat, DVec3};
        let mut d = Doc::default();
        let block = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-10.0, -5.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Add, Prim::Cylinder { radius: 7.0, height: 10.0 }, DVec3::new(10.0, 20.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Box { x: 8.0, y: 8.0, z: 40.0 }, DVec3::new(0.0, 14.0, 0.0), DQuat::IDENTITY);
        for fillet in [false, true] {
            if fillet {
                // the front right upright edge rounded at 3 mm, as in the captures
                let tag = |sub: u16| FaceTag { feature: block, sub }.pack();
                d.add_round(3.0, false, vec![EdgeRef { faces: [tag(0), tag(4)], at: DVec3::new(20.0, 10.0, 15.0) }]);
            }
            let m = crate::build::build(&d, true, true, &[], &mut Default::default());
            let q = m.quads.expect("quads");
            let mesh = QuadMesh { positions: q.positions.iter().map(|p| p.map(|x| x as f64)).collect(), quads: q.quads.clone(), quad_face: q.quad_face.clone(), skipped: vec![] };
            std::fs::write(std::env::temp_dir().join(format!("forme-quads-part{}.obj", if fillet { "-fillet" } else { "" })), mesh.obj(|f| format!("face{f}"))).unwrap();
            let mut faces = std::collections::HashSet::new();
            faces.extend(q.quad_face.iter().copied());
            eprintln!("test part (fillet {fillet}): {} quads on {} faces of {}, left out {:?}, errors {:?}", q.quads.len(), faces.len(), m.faces.len(), q.skipped, m.errors);
            assert!(q.skipped.is_empty(), "fillet {fillet}: {:?}", q.skipped);
        }
    }

    /// A cube with fillets on the edges given by their two ends.
    fn rounded(size: f64, r: f64, ends: &[([f64; 3], [f64; 3])]) -> Shape {
        let h = size * 0.5;
        let cube = Shape::cuboid(size, size, size).unwrap();
        let es: Vec<i32> = ends.iter().map(|(a, b)| edge_between(&cube, [a[0] * h, a[1] * h, a[2] * h], [b[0] * h, b[1] * h, b[2] * h])).collect();
        cube.fillet(&es, r, 99).unwrap()
    }

    /// How the converter does on a shape: (quads, bad quads, skipped).
    fn report(name: &str, s: &Shape) -> (usize, usize, usize) {
        let q = convert(s, 2.5).unwrap();
        std::fs::write(std::env::temp_dir().join(format!("forme-quads-{name}.obj")), q.obj(|f| format!("face{f}"))).unwrap();
        let r = (q.quads.len(), q.bad_quads(20.0), q.skipped.len());
        eprintln!("{name}: {} quads, {} bad, closed {}, skipped {:?}", r.0, r.1, q.is_closed(), q.skipped);
        r
    }

    /// Fillets meeting in a corner: the face closing on a pole (a sphere's
    /// eighth) is three quads round a point, the mesh closed.
    #[test]
    fn fillet_corners() {
        let x = [1.0, 1.0, 1.0];
        let three = [([1.0, 1.0, -1.0], x), ([1.0, -1.0, 1.0], x), ([-1.0, 1.0, 1.0], x)];
        let mut all = vec![];
        for a in [-1.0, 1.0] {
            for b in [-1.0, 1.0] {
                all.push(([-1.0, a, b], [1.0, a, b]));
                all.push(([a, -1.0, b], [a, 1.0, b]));
                all.push(([a, b, -1.0], [a, b, 1.0]));
            }
        }
        for r in [4.0, 8.0] {
            let q = check(&format!("corner-r{r}"), &rounded(20.0, r, &three), 0);
            assert_eq!(q.bad_quads(20.0), 0, "corner r{r}");
        }
        for r in [2.0, 4.0, 8.0] {
            let q = check(&format!("all-r{r}"), &rounded(20.0, r, &all), 0);
            assert_eq!(q.bad_quads(20.0), 0, "all r{r}");
            // 8 corners, each a pole of three in the middle of its patch
            let mut val = vec![0usize; q.positions.len()];
            for f in &q.quads {
                for v in f {
                    val[*v as usize] += 1;
                }
            }
            assert!(val.iter().filter(|v| **v == 3).count() >= 8, "all r{r}: the corners' poles");
        }
    }

    #[test]
    fn part_large_fillet() {
        use crate::doc::{Doc, EdgeRef, Op, Prim};
        use crate::model::FaceTag;
        use glam::{DQuat, DVec3};
        let mut d = Doc::default();
        let block = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-10.0, -5.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Add, Prim::Cylinder { radius: 7.0, height: 10.0 }, DVec3::new(10.0, 20.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Box { x: 8.0, y: 8.0, z: 40.0 }, DVec3::new(0.0, 14.0, 0.0), DQuat::IDENTITY);
        let tag = |sub: u16| FaceTag { feature: block, sub }.pack();
        let edges = [(2u16, 4u16, DVec3::new(12.0, 20.0, 15.0)), (0, 4, DVec3::new(20.0, 10.0, 15.0)), (0, 2, DVec3::new(20.0, 20.0, 0.0))];
        d.add_round(6.0, false, edges.iter().map(|(a, b, at)| EdgeRef { faces: [tag(*a), tag(*b)], at: *at }).collect());
        let m = crate::build::build(&d, true, true, &[], &mut Default::default());
        let q = m.quads.expect("quads");
        eprintln!("part, large fillet: {} quads, left out {:?}", q.quads.len(), q.skipped);
        assert!(q.skipped.is_empty());
    }

    #[test]
    fn corner_survey() {
        let x = [1.0, 1.0, 1.0];
        let three = [([1.0, 1.0, -1.0], x), ([1.0, -1.0, 1.0], x), ([-1.0, 1.0, 1.0], x)];
        for r in [2.0, 4.0, 8.0] {
            report(&format!("corner3-r{r}"), &rounded(20.0, r, &three));
        }
        // all twelve edges
        let mut all = vec![];
        for a in [-1.0, 1.0] {
            for b in [-1.0, 1.0] {
                all.push(([-1.0, a, b], [1.0, a, b]));
                all.push(([a, -1.0, b], [a, 1.0, b]));
                all.push(([a, b, -1.0], [a, b, 1.0]));
            }
        }
        for r in [2.0, 4.0, 8.0] {
            report(&format!("all-r{r}"), &rounded(20.0, r, &all));
        }
        // one large fillet
        report("one-r8", &rounded(20.0, 8.0, &[([-1.0, 1.0, 1.0], x)]));
    }
}
