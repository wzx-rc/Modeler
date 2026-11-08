//! Edges you can pick and fillet, straight from the modelling history:
//! - a junction: where two shapes meet; its fillet is the blend of the
//!   operation that joins them;
//! - a shape's own edges (box edges, cylinder rims); their fillet is the
//!   shape's rounding, shared by all of them.
//!
//! Curves are exact: junctions are traced on the two shapes' distance
//! functions, then laid on the final surface (the middle of the fillet).

use crate::bake::field::{prim_raw, Field};
use crate::program::{compile, compile_single};
use crate::scene::*;
use glam::{DVec3, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EdgeKind {
    /// Where shape `a` meets shape `b`.
    Junction { a: NodeId, b: NodeId },
    /// The shape's own edges.
    Own { node: NodeId },
}

#[derive(Clone, Debug)]
pub struct EdgePick {
    pub kind: EdgeKind,
    /// Node holding the fillet: its blend (junction) or its rounding (own edges).
    pub owner: NodeId,
    /// World polylines on the final surface, the hovered one first.
    pub curves: Vec<Vec<Vec3>>,
    /// Point of the edge nearest to the pick, and the surface normal there.
    pub at: Vec3,
    pub normal: Vec3,
    /// Fillet size (mm) and kind.
    pub radius: f32,
    pub chamfer: bool,
    pub label: String,
    /// Where the pick happened (to rebuild the curves after an edit).
    pub seed: Vec3,
}

/// Distance functions of every visible shape, built once per scene version.
pub struct EdgeIndex {
    shapes: Vec<(NodeId, Field)>,
    all: Field,
    extent: f64,
}

fn visible(scene: &Scene, id: NodeId) -> bool {
    scene.node(id).map(|n| n.visible).unwrap_or(false) && scene.ancestors(id).iter().all(|a| scene.nodes[a].visible)
}

/// Depth-first order of the nodes (later nodes are combined after earlier ones).
fn order(scene: &Scene) -> Vec<NodeId> {
    let mut out = vec![];
    fn walk(s: &Scene, id: NodeId, out: &mut Vec<NodeId>) {
        out.push(id);
        for c in s.nodes[&id].children() {
            walk(s, *c, out);
        }
    }
    walk(scene, scene.root, &mut out);
    out
}

/// The node whose combine creates the junction between `a` and `b`: the
/// later one, or its ancestor that is a sibling of the earlier one's branch.
pub fn junction_owner(scene: &Scene, a: NodeId, b: NodeId) -> NodeId {
    let ord = order(scene);
    let pos = |id: NodeId| ord.iter().position(|x| *x == id).unwrap_or(0);
    let (early, late) = if pos(a) < pos(b) { (a, b) } else { (b, a) };
    let mut anc: Vec<NodeId> = scene.ancestors(early);
    anc.push(early);
    anc.push(scene.root);
    let mut cur = late;
    while let Some(p) = scene.parent_of(cur) {
        if anc.contains(&p) {
            break;
        }
        cur = p;
    }
    cur
}

fn grad(f: &dyn Fn(DVec3) -> f64, p: DVec3) -> DVec3 {
    let h = 1e-5;
    DVec3::new(
        f(p + DVec3::X * h) - f(p - DVec3::X * h),
        f(p + DVec3::Y * h) - f(p - DVec3::Y * h),
        f(p + DVec3::Z * h) - f(p - DVec3::Z * h),
    ) / (2.0 * h)
}

/// Closest point on the curve f1 = f2 = 0 (minimum-norm Gauss–Newton).
fn solve2(f1: &dyn Fn(DVec3) -> f64, f2: &dyn Fn(DVec3) -> f64, p0: DVec3, cap: f64) -> Option<DVec3> {
    let mut p = p0;
    for _ in 0..40 {
        let (a, b) = (f1(p), f2(p));
        if a.abs() < 1e-7 && b.abs() < 1e-7 {
            return Some(p);
        }
        let (g1, g2) = (grad(f1, p), grad(f2, p));
        let (m11, m12, m22) = (g1.dot(g1), g1.dot(g2), g2.dot(g2));
        let det = m11 * m22 - m12 * m12;
        if det < 1e-9 * m11 * m22 {
            return None;
        }
        let l1 = (m22 * a - m12 * b) / det;
        let l2 = (m11 * b - m12 * a) / det;
        let mut dx = -(g1 * l1 + g2 * l2);
        let l = dx.length();
        if l > cap {
            dx *= cap / l;
        }
        p += dx;
    }
    let ok = f1(p).abs() < 1e-4 && f2(p).abs() < 1e-4;
    ok.then_some(p)
}

impl EdgeIndex {
    pub fn new(scene: &Scene) -> Self {
        let mut shapes = vec![];
        for id in order(scene) {
            let n = &scene.nodes[&id];
            if n.shape().is_some() && visible(scene, id) {
                shapes.push((id, Field::new(&compile_single(scene, id))));
            }
        }
        let (lo, hi) = scene.bounds();
        let extent = if lo.x <= hi.x { (hi - lo).max_element() as f64 } else { 20.0 };
        EdgeIndex { shapes, all: Field::new(&compile(scene)), extent: extent.max(1.0) }
    }

    /// The edge nearest to surface point `p`, within `tol` (mm).
    pub fn pick(&self, scene: &Scene, p: Vec3, tol: f32) -> Option<EdgePick> {
        let pd = p.as_dvec3();
        let tol = tol as f64;
        if self.shapes.is_empty() {
            return None;
        }
        let d: Vec<f64> = self.shapes.iter().map(|(_, f)| f.dist(pd)).collect();
        // the shape whose surface this is (cutters own the walls they carve)
        let mut owner = 0;
        let mut best = f64::INFINITY;
        for (i, (id, _)) in self.shapes.iter().enumerate() {
            let mut s = d[i].abs();
            if scene.nodes[id].combine.mode.is_cutter() {
                s *= 0.8;
            }
            if s < best {
                best = s;
                owner = i;
            }
        }
        let (oid, _) = self.shapes[owner];
        let mut cands: Vec<(f64, EdgePick)> = vec![];
        // junctions with the other shapes
        for (j, (jid, _)) in self.shapes.iter().enumerate() {
            if j == owner {
                continue;
            }
            let gov = junction_owner(scene, oid, *jid);
            let k = scene.nodes[&gov].combine.blend.max(0.0) as f64;
            let dj = d[j].abs();
            if dj > tol + k * 1.2 {
                continue;
            }
            if let Some(e) = self.junction(scene, owner, j, pd) {
                let score = (e.at.as_dvec3().distance(pd) - k * 0.7).max(0.0);
                if score <= tol {
                    cands.push((score, e));
                }
            }
        }
        // the shape's own edges
        if let Some((score, e)) = self.own(scene, owner, pd, tol) {
            cands.push((score, e));
        }
        cands.into_iter().min_by(|a, b| a.0.partial_cmp(&b.0).unwrap()).map(|(_, mut e)| {
            e.seed = p;
            e.normal = self.all.grad(e.at.as_dvec3()).normalize_or_zero().as_vec3();
            e
        })
    }

    /// Same edge again after an edit (curves and radius follow the scene).
    pub fn refresh(&self, scene: &Scene, e: &EdgePick) -> Option<EdgePick> {
        let find = |id: NodeId| self.shapes.iter().position(|(n, _)| *n == id);
        let mut out = match e.kind {
            EdgeKind::Junction { a, b } => self.junction(scene, find(a)?, find(b)?, e.seed.as_dvec3()),
            EdgeKind::Own { node } => self.own(scene, find(node)?, e.seed.as_dvec3(), f64::INFINITY).map(|x| x.1),
        }?;
        out.seed = e.seed;
        out.normal = self.all.grad(out.at.as_dvec3()).normalize_or_zero().as_vec3();
        Some(out)
    }

    fn junction(&self, scene: &Scene, a: usize, b: usize, p: DVec3) -> Option<EdgePick> {
        let (aid, fa) = &self.shapes[a];
        let (bid, fb) = &self.shapes[b];
        let sa = |x: DVec3| fa.dist(x);
        let sb = |x: DVec3| fb.dist(x);
        let h = (self.extent / 160.0).max(0.02);
        let c0 = solve2(&sa, &sb, p, h * 4.0)?;
        if c0.distance(p) > self.extent * 0.25 {
            return None;
        }
        // predictor–corrector along the corner curve, both ways
        let trace = |dir: f64| -> (Vec<DVec3>, bool) {
            let mut pts = vec![];
            let mut c = c0;
            let mut t_prev = DVec3::ZERO;
            for i in 0..2400 {
                let t = grad(&sa, c).cross(grad(&sb, c)).normalize_or_zero() * dir;
                if t == DVec3::ZERO {
                    break;
                }
                let t = if i > 0 && t.dot(t_prev) < 0.0 { -t } else { t };
                t_prev = t;
                let Some(n) = solve2(&sa, &sb, c + t * h, h) else { break };
                if n.distance(c) < h * 0.2 {
                    break;
                }
                if i > 3 && n.distance(c0) < h * 0.8 {
                    return (pts, true);
                }
                pts.push(n);
                c = n;
            }
            (pts, false)
        };
        let (fwd, closed) = trace(1.0);
        let mut corner = vec![c0];
        corner.extend(fwd);
        if !closed {
            let (mut back, _) = trace(-1.0);
            back.reverse();
            back.extend(corner);
            corner = back;
        }
        if closed {
            corner.push(c0);
        }

        // on the final surface: the middle of the fillet, where it is visible
        let gov = junction_owner(scene, *aid, *bid);
        let gn = &scene.nodes[&gov];
        let k = gn.combine.blend.max(0.0) as f64;
        let mut curves: Vec<Vec<Vec3>> = vec![];
        let mut cur: Vec<Vec3> = vec![];
        let mut at = c0;
        let mut best = f64::INFINITY;
        for c in &corner {
            let x = self.all.project(*c).filter(|x| x.distance(*c) <= k * 0.6 + h * 0.5 + 1e-3);
            match x {
                Some(x) => {
                    if x.distance(p) < best {
                        best = x.distance(p);
                        at = x;
                    }
                    cur.push(x.as_vec3());
                }
                None => {
                    if cur.len() > 1 {
                        curves.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                }
            }
        }
        if cur.len() > 1 {
            curves.push(cur);
        }

        if curves.is_empty() {
            return None;
        }
        let name = |id: &NodeId| scene.nodes[id].name.clone();
        Some(EdgePick {
            kind: EdgeKind::Junction { a: *aid, b: *bid },
            owner: gov,
            curves,
            at: at.as_vec3(),
            normal: Vec3::Y,
            radius: gn.combine.blend.max(0.0),
            chamfer: gn.combine.kind == BlendKind::Chamfer,
            label: format!("{} × {}", name(aid), name(bid)),
            seed: p.as_vec3(),
        })
    }

    /// Own edges of shape `i` near `p`: box edges and cylinder rims.
    fn own(&self, scene: &Scene, i: usize, p: DVec3, tol: f64) -> Option<(f64, EdgePick)> {
        let (id, f) = &self.shapes[i];
        let node = &scene.nodes[id];
        let shape = node.shape()?;
        let pr = f.prims.first()?;
        if pr.taper.abs() > 1e-4 || !matches!(pr.kind, 0 | 1) {
            return None;
        }
        let r = if shape.chamfer > 0.0 { shape.chamfer as f64 } else { pr.round };
        let insts = f.instances(0);
        // nearest copy and its local point
        let (inst, lp) = insts
            .iter()
            .map(|c| (c, c.to_local.transform_point3(p)))
            .min_by(|a, b| prim_raw(pr, a.1).abs().partial_cmp(&prim_raw(pr, b.1).abs()).unwrap())?;
        let tl = tol / pr.sc.max(1e-6);
        let zone = r.max(0.0) + tl;
        let s = pr.size;
        let d45 = std::f64::consts::FRAC_1_SQRT_2;
        // local polylines of every edge (middle of the rounding), and the distance to the hovered one
        let mut local: Vec<Vec<DVec3>> = vec![];
        let near: f64;
        match pr.kind {
            0 => {
                let rr = r.min(s.min_element());
                let inner = s - DVec3::splat(rr);
                let q = lp.abs() - inner;
                let mut idx = [0usize, 1, 2];
                idx.sort_by(|a, b| q[*b].partial_cmp(&q[*a]).unwrap());
                if q[idx[1]] < -zone {
                    return None;
                }
                // distance to the edge between the two dominant faces
                let (u, v) = (idx[0], idx[1]);
                let e = DVec3::new(
                    if u == 0 || v == 0 { inner.x + rr * d45 } else { 0.0 },
                    if u == 1 || v == 1 { inner.y + rr * d45 } else { 0.0 },
                    if u == 2 || v == 2 { inner.z + rr * d45 } else { 0.0 },
                );
                let a = lp.abs();
                let du = a[u] - e[u];
                let dv = a[v] - e[v];
                near = (du * du + dv * dv).sqrt() * pr.sc;
                for axis in 0..3 {
                    let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
                    for (sb, sc) in [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
                        // straight along the edge, curving into the rounded corners
                        let ext = inner[axis] + rr * 0.62;
                        let mut line = vec![];
                        for t in 0..=20 {
                            let mut x = DVec3::ZERO;
                            x[axis] = -ext + 2.0 * ext * t as f64 / 20.0;
                            x[b] = sb * (inner[b] + rr * d45);
                            x[c] = sc * (inner[c] + rr * d45);
                            line.push(if x[axis].abs() > inner[axis] { crate::bake::field::prim_project(pr, x, 0.0) } else { x });
                        }
                        local.push(line);
                    }
                }
            }
            _ => {
                let rr = r.min(s.x).min(s.y);
                let rad = DVec3::new(lp.x, 0.0, lp.z).length();
                let (dx, dy) = (rad - (s.x - rr), lp.y.abs() - (s.y - rr));
                if dx < -zone || dy < -zone {
                    return None;
                }
                let (er, ey) = (s.x - rr + rr * d45, s.y - rr + rr * d45);
                near = ((rad - er).powi(2) + (lp.y.abs() - ey).powi(2)).sqrt() * pr.sc;
                for sy in [1.0, -1.0] {
                    local.push((0..=96).map(|j| {
                        let a = std::f64::consts::TAU * j as f64 / 96.0;
                        DVec3::new(er * a.cos(), sy * ey, er * a.sin())
                    }).collect());
                }
            }
        }
        if near > tol + r * pr.sc {
            return None;
        }
        // every copy shares the rounding: show them all, cut where they are not visible
        let mut curves: Vec<Vec<Vec3>> = vec![];
        let keep = (r * pr.sc * 0.5).max(self.extent * 2e-3);
        let mut ordered: Vec<&crate::bake::field::Instance> = vec![inst];
        ordered.extend(insts.iter().filter(|c| c.copy != inst.copy));
        for c in ordered {
            for line in &local {
                let mut cur: Vec<Vec3> = vec![];
                for x in line {
                    let w = c.to_world.transform_point3(*x);
                    if self.all.dist(w).abs() <= keep {
                        cur.push(w.as_vec3());
                    } else {
                        if cur.len() > 1 {
                            curves.push(std::mem::take(&mut cur));
                        }
                        cur.clear();
                    }
                }
                if cur.len() > 1 {
                    curves.push(cur);
                }
            }
        }
        if curves.is_empty() {
            return None;
        }
        let at = curves.iter().flatten().min_by(|a, b| a.as_dvec3().distance(p).partial_cmp(&b.as_dvec3().distance(p)).unwrap()).copied()?;
        let what = if pr.kind == 0 { "Arêtes" } else { "Bords" };
        Some((
            (near - r * pr.sc * 0.7).max(0.0),
            EdgePick {
                kind: EdgeKind::Own { node: *id },
                owner: *id,
                curves,
                at,
                normal: Vec3::Y,
                radius: if shape.chamfer > 0.0 { shape.chamfer } else { shape.round },
                chamfer: shape.chamfer > 0.0,
                label: format!("{what} de {}", node.name),
                seed: p.as_vec3(),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube_hole() -> Scene {
        let mut s = Scene::new();
        let root = s.root;
        let mut b = Shape::new(PrimKind::Box);
        b.round = 1.0;
        s.add(root, NodeSpec::shape("Cube", b));
        let mut c = Shape::new(PrimKind::Cylinder);
        c.size = Vec3::new(4.0, 14.0, 0.0);
        s.add(root, NodeSpec::shape("Trou", c).mode(CombineMode::Carve).blend(0.8));
        s
    }

    #[test]
    fn picks_junction_and_box_edges() {
        let s = cube_hole();
        let idx = EdgeIndex::new(&s);
        // on the top face, next to the hole rim
        let e = idx.pick(&s, Vec3::new(4.9, 10.0, 0.0), 0.5).expect("junction");
        assert!(matches!(e.kind, EdgeKind::Junction { .. }), "{:?}", e.kind);
        assert!((e.radius - 0.8).abs() < 1e-6);
        let n: usize = e.curves.iter().map(|c| c.len()).sum();
        assert!(n > 50, "closed loop around the hole: {n} points");
        // on the top face, next to a box edge
        let e = idx.pick(&s, Vec3::new(9.4, 10.0, 3.0), 0.5).expect("box edge");
        assert!(matches!(e.kind, EdgeKind::Own { .. }), "{:?}", e.kind);
        assert_eq!(e.curves.len(), 12);
        // in the middle of a face: nothing
        assert!(idx.pick(&s, Vec3::new(-6.0, 10.0, -6.0), 0.5).is_none());
    }
}

