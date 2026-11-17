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
        if wires.iter().flatten().any(|(e, _)| self.topo.edges[(*e - 1) as usize].degenerated) {
            return Err("a pole (degenerated edge)".into());
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
        patches.push(Patch { face: fi, sides });
    }

    // 2. segments per piece: by length (an arc by angle), then raised until
    // opposite sides match; a face whose counts cannot settle is left out
    let mut count: HashMap<Piece, usize> = HashMap::new();
    for _round in 0..4 {
        count.clear();
        for pt in &patches {
            for (p, _, _) in pt.sides.iter().flatten() {
                count.entry(*p).or_insert_with(|| {
                    let l = cx.piece_len(*p);
                    let line = &cx.lines[p.line];
                    let by_len = (l / target).round() as usize;
                    let n = if line.edge > 0 {
                        let e = &topo.edges[(line.edge - 1) as usize];
                        if e.kind == 1 && e.radius > 0.0 {
                            by_len.max((l / e.radius / ARC_STEP).round() as usize)
                        } else {
                            by_len
                        }
                    } else {
                        by_len
                    };
                    n.max(1)
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
                for (a, b) in [(0, 2), (1, 3)] {
                    let sum = |k: usize| pt.sides[k].iter().map(|(p, _, _)| count[p]).sum::<usize>();
                    let (na, nb) = (sum(a), sum(b));
                    if na != nb {
                        let short = if na < nb { a } else { b };
                        // the coarsest piece of the short side gets one more
                        let (p, _, _) = pt.sides[short].iter().max_by(|x, y| (cx.piece_len(x.0) / count[&x.0] as f64).total_cmp(&(cx.piece_len(y.0) / count[&y.0] as f64))).unwrap();
                        *count.get_mut(p).unwrap() += 1;
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
            break;
        }
        // the faces still off are left out, and the counts made again
        let mut off: HashSet<usize> = HashSet::new();
        for pt in &patches {
            let sum = |k: usize| pt.sides[k].iter().map(|(p, _, _)| count[p]).sum::<usize>();
            if sum(0) != sum(2) || sum(1) != sum(3) {
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
        // on the surface: the inner points, and every point's normal
        let all: Vec<V2> = (0..=nn).flat_map(|i| (0..=mm).map(move |j| (i, j))).map(|(i, j)| grid[i][j].1).collect();
        let eval = match shape.face_eval(fi, &all) {
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
    // faces in face order, so each face's quads are together
    let mut order: Vec<usize> = (0..mesh.quads.len()).collect();
    order.sort_by_key(|k| mesh.quad_face[*k]);
    mesh.quads = order.iter().map(|k| mesh.quads[*k]).collect();
    mesh.quad_face = order.iter().map(|k| mesh.quad_face[*k]).collect();
    mesh.skipped.sort_by_key(|s| s.0);
    mesh.skipped.dedup_by_key(|s| s.0);
    Ok(mesh)
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
}
