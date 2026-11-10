//! Construction lines of a scene: the lines a modeller would start the
//! topology from, read straight from the modelling history.
//!
//! - a shape's own edges (box edges, cylinder rims), sharp, or the two rails
//!   of their rounding / chamfer;
//! - junctions, where two shapes meet: the sharp corner curve, or with a
//!   blend the two rails where the fillet starts on each shape.
//!
//! Only the parts that lie on the final surface are kept. The Bake builds
//! its fillet strips on the rails first, then fills the faces between the
//! lines (see docs/BAKE.md); the Bake panel draws them for diagnosis.

use super::build::contour;
use super::field::{box_edge_signs, Field, PrimDef};
use super::gen::{self, GenSettings};
use crate::edges::{junction_owner, solve2};
use crate::program::{compile, compile_single};
use crate::scene::*;
use glam::DVec3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// Sharp edge of a shape.
    Edge,
    /// Rail of a shape's own rounding or chamfer (edge of its fillet strip).
    EdgeRail,
    /// Sharp junction between two shapes (the contour between them).
    Junction,
    /// Rail of a junction fillet (blend), on one of the two shapes.
    JunctionRail,
}

impl LineKind {
    /// Rails bound a fillet strip (drawn orange); the others are sharp.
    pub fn is_rail(self) -> bool {
        matches!(self, LineKind::EdgeRail | LineKind::JunctionRail)
    }
}

#[derive(Clone, Debug)]
pub struct CLine {
    pub kind: LineKind,
    /// World polyline on the final surface.
    pub pts: Vec<DVec3>,
    /// Fillet size (mm, 0 when sharp; negative = chamfer).
    pub radius: f64,
    pub label: String,
}

#[derive(Clone, Debug, Default)]
pub struct Lines {
    pub lines: Vec<CLine>,
    pub seconds: f64,
}

impl Lines {
    pub fn count(&self, junction: bool) -> usize {
        self.lines.iter().filter(|l| matches!(l.kind, LineKind::Junction | LineKind::JunctionRail) == junction).count()
    }
}

fn visible(scene: &Scene, id: NodeId) -> bool {
    scene.node(id).map(|n| n.visible).unwrap_or(false) && scene.ancestors(id).iter().all(|a| scene.nodes[a].visible)
}

fn order(scene: &Scene) -> Vec<NodeId> {
    fn walk(s: &Scene, id: NodeId, out: &mut Vec<NodeId>) {
        out.push(id);
        for c in s.nodes[&id].children() {
            walk(s, *c, out);
        }
    }
    let mut out = vec![];
    walk(scene, scene.root, &mut out);
    out
}

/// Fillet of one own edge (negative: chamfer).
pub(crate) fn edge_r(pr: &PrimDef, e: usize) -> f64 {
    match pr.edges.get(e) {
        Some(r) => *r,
        None if pr.chamfer > 0.0 => -pr.chamfer,
        None => pr.round,
    }
}

/// Local lines of own edge `e`: the sharp edge, or the two rails of its
/// rounding (box: 12 edges, cylinder: 2 rims). Ends stop where the
/// neighbouring edges' rounding starts.
fn own_lines(pr: &PrimDef, e: usize, n: usize) -> Vec<(LineKind, Vec<DVec3>)> {
    let s = pr.size;
    let r = edge_r(pr, e);
    let ra = r.abs();
    if pr.kind == 0 {
        let (axis, sb, sc) = box_edge_signs(e);
        let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
        let ra = ra.min(s[b]).min(s[c]);
        // the neighbouring edges' rounding at both ends
        let end = (0..12).filter(|&k| k / 4 != axis).map(|k| edge_r(pr, k).abs()).fold(0.0, f64::max).min(s[axis]);
        let ext = s[axis] - end;
        let line = |ob: f64, oc: f64| -> Vec<DVec3> {
            (0..=n)
                .map(|t| {
                    let mut x = DVec3::ZERO;
                    x[axis] = -ext + 2.0 * ext * t as f64 / n as f64;
                    x[b] = sb * (s[b] - ob);
                    x[c] = sc * (s[c] - oc);
                    x
                })
                .collect()
        };
        if ra < 1e-9 {
            vec![(LineKind::Edge, line(0.0, 0.0))]
        } else {
            vec![(LineKind::EdgeRail, line(0.0, ra)), (LineKind::EdgeRail, line(ra, 0.0))]
        }
    } else {
        let ra = ra.min(s.x).min(s.y);
        let sy = if e == 0 { 1.0 } else { -1.0 };
        let circle = |rad: f64, y: f64| -> Vec<DVec3> {
            (0..=n)
                .map(|j| {
                    let a = std::f64::consts::TAU * j as f64 / n as f64;
                    DVec3::new(rad * a.cos(), y, rad * a.sin())
                })
                .collect()
        };
        if ra < 1e-9 {
            vec![(LineKind::Edge, circle(s.x, sy * s.y))]
        } else {
            vec![(LineKind::EdgeRail, circle(s.x - ra, sy * s.y)), (LineKind::EdgeRail, circle(s.x, sy * (s.y - ra)))]
        }
    }
}

/// Splits a polyline into the runs where `keep` holds, with the cut points
/// found by bisection.
fn runs(pts: &[DVec3], keep: &dyn Fn(DVec3) -> bool) -> Vec<Vec<DVec3>> {
    let cut = |a: DVec3, b: DVec3| -> DVec3 {
        // a kept, b not
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..24 {
            let m = (lo + hi) * 0.5;
            if keep(a.lerp(b, m)) {
                lo = m;
            } else {
                hi = m;
            }
        }
        a.lerp(b, lo)
    };
    let mut out = vec![];
    let mut cur: Vec<DVec3> = vec![];
    let mut prev: Option<(DVec3, bool)> = None;
    for &p in pts {
        let k = keep(p);
        if let Some((q, kq)) = prev {
            if kq && !k {
                cur.push(cut(q, p));
                if cur.len() > 1 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
            } else if !kq && k {
                cur.push(cut(p, q));
            }
        }
        if k {
            cur.push(p);
        }
        prev = Some((p, k));
    }
    if cur.len() > 1 {
        out.push(cur);
    }
    // a closed line kept all the way round: one run
    out
}

/// Every construction line of `scene` on its final surface.
pub fn construction_lines(scene: &Scene) -> Lines {
    let t0 = std::time::Instant::now();
    let all = Field::new(&compile(scene));
    let (lo, hi) = scene.bounds();
    let extent = if lo.x <= hi.x { ((hi - lo).max_element() as f64).max(1.0) } else { 20.0 };
    let h = extent / 240.0;
    let eps = extent * 2e-5;
    let on_surface = |p: DVec3| all.dist(p).abs() <= eps;
    let shapes: Vec<(NodeId, Field)> = order(scene)
        .into_iter()
        .filter(|id| scene.nodes[id].shape().is_some() && visible(scene, *id))
        .map(|id| (id, Field::new(&compile_single(scene, id))))
        .collect();
    let name = |id: &NodeId| scene.nodes[id].name.clone();
    let mut out = Lines::default();

    // own edges
    for (id, f) in &shapes {
        let Some(pr) = f.prims.first() else { continue };
        if pr.taper.abs() > 1e-4 || !matches!(pr.kind, 0 | 1) {
            continue;
        }
        let n_edges = if pr.kind == 0 { 12 } else { 2 };
        for inst in f.instances(0) {
            for e in 0..n_edges {
                let len = if pr.kind == 0 { 2.0 * pr.size[e / 4] } else { std::f64::consts::TAU * pr.size.x } * pr.sc;
                let n = ((len / h).ceil() as usize).clamp(8, 2000);
                for (kind, line) in own_lines(pr, e, n) {
                    let w: Vec<DVec3> = line.iter().map(|x| inst.to_world.transform_point3(*x)).collect();
                    for run in runs(&w, &on_surface) {
                        out.lines.push(CLine { kind, pts: run, radius: edge_r(pr, e) * pr.sc, label: format!("{} · arête {}", name(id), e + 1) });
                    }
                }
            }
        }
    }

    // junctions between two shapes
    let gs = GenSettings { tol: extent / 600.0, max_edge: extent / 40.0, segs: 0 };
    for i in 0..shapes.len() {
        let (aid, fa) = &shapes[i];
        let Some(pa) = fa.prims.first() else { continue };
        // the first shape's native mesh carries the seeds
        let m = gen::generate(pa, 0.0, &gs);
        for j in (i + 1)..shapes.len() {
            let (bid, fb) = &shapes[j];
            let sa = |x: DVec3| fa.dist(x);
            let sb = |x: DVec3| fb.dist(x);
            let gov = junction_owner(scene, *aid, *bid);
            let gn = &scene.nodes[&gov];
            let k = gn.combine.blend.max(0.0) as f64;
            let carve = gn.combine.mode == CombineMode::Carve;
            let label = format!("{} × {}", name(aid), name(bid));
            for inst in fa.instances(0) {
                let pos: Vec<DVec3> = m.pos.iter().map(|p| inst.to_world.transform_point3(*p)).collect();
                let set: Vec<usize> = (0..m.faces.len()).collect();
                for lp in contour(&pos, &m.faces, &set, &sb) {
                    // exact corner curve, sampled every h
                    let mut corner: Vec<DVec3> = vec![];
                    let refine = |p: DVec3| solve2(&sa, &sb, p, h * 2.0);
                    let closed: Vec<DVec3> = lp.iter().chain(lp.first()).copied().collect();
                    let mut last: Option<DVec3> = None;
                    for p in closed {
                        let Some(c) = refine(p) else { continue };
                        if let Some(q) = last {
                            let steps = (q.distance(c) / h).ceil() as usize;
                            for s in 1..steps {
                                if let Some(x) = refine(q.lerp(c, s as f64 / steps as f64)) {
                                    corner.push(x);
                                }
                            }
                        }
                        corner.push(c);
                        last = Some(c);
                    }
                    if corner.len() < 2 {
                        continue;
                    }
                    if k < 1e-9 {
                        for run in runs(&corner, &on_surface) {
                            out.lines.push(CLine { kind: LineKind::Junction, pts: run, radius: 0.0, label: label.clone() });
                        }
                    } else {
                        // the fillet starts on each shape where the other one is k away
                        let ka = if carve { -k } else { k };
                        let rail_a: Vec<DVec3> = corner.iter().filter_map(|c| solve2(&sa, &|x| sb(x) - k, *c, h * 2.0)).collect();
                        let rail_b: Vec<DVec3> = corner.iter().filter_map(|c| solve2(&sb, &|x| sa(x) - ka, *c, h * 2.0)).collect();
                        for rail in [rail_a, rail_b] {
                            for run in runs(&rail, &on_surface) {
                                out.lines.push(CLine { kind: LineKind::JunctionRail, pts: run, radius: k, label: label.clone() });
                            }
                        }
                    }
                }
            }
        }
    }
    out.seconds = t0.elapsed().as_secs_f64();
    out
}
