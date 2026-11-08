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
    /// One of the shape's own edges (box edge, cylinder rim).
    Own { node: NodeId, edge: u32 },
}

impl EdgeKind {
    /// Same edge (a junction is the same from either side).
    pub fn same(&self, o: &EdgeKind) -> bool {
        match (self, o) {
            (EdgeKind::Junction { a, b }, EdgeKind::Junction { a: c, b: d }) => (a == c && b == d) || (a == d && b == c),
            _ => self == o,
        }
    }
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
        if let Some((score, e)) = self.own(scene, owner, pd, tol, None) {
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
            EdgeKind::Own { node, edge } => self.own(scene, find(node)?, e.seed.as_dvec3(), f64::INFINITY, Some(edge as usize)).map(|x| x.1),
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

    /// Fillet of one own edge (negative: chamfer), as the evaluator sees it.
    fn edge_r(pr: &crate::bake::field::PrimDef, e: usize) -> f64 {
        match pr.edges.get(e) {
            Some(r) => *r,
            None if pr.chamfer > 0.0 => -pr.chamfer,
            None => pr.round,
        }
    }

    /// Local midline of own edge `e` (box: 12 edges, cylinder: 2 rims).
    fn edge_line(pr: &crate::bake::field::PrimDef, e: usize) -> Vec<DVec3> {
        let s = pr.size;
        let r = Self::edge_r(pr, e);
        let d45 = std::f64::consts::FRAC_1_SQRT_2;
        if pr.kind == 0 {
            let (axis, sb, sc) = crate::bake::field::box_edge_signs(e);
            let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
            let cap = s[b].min(s[c]);
            // middle of the rounding (or of the chamfer face), set in from the sharp edge
            let m = if r >= 0.0 { r.min(cap) * (1.0 - d45) } else { (-r).min(cap) * 0.5 };
            let ext = s[axis] - m;
            (0..=24)
                .map(|t| {
                    let mut x = DVec3::ZERO;
                    x[axis] = -ext + 2.0 * ext * t as f64 / 24.0;
                    x[b] = sb * (s[b] - m);
                    x[c] = sc * (s[c] - m);
                    // into the corners: onto the surface
                    if t == 0 || t == 24 || x[axis].abs() > s[axis] - r.abs() {
                        crate::bake::field::prim_project(pr, x, 0.0)
                    } else {
                        x
                    }
                })
                .collect()
        } else {
            let cap = s.x.min(s.y);
            let m = if r >= 0.0 { r.min(cap) * (1.0 - d45) } else { (-r).min(cap) * 0.5 };
            let sy = if e == 0 { 1.0 } else { -1.0 };
            (0..=96)
                .map(|j| {
                    let a = std::f64::consts::TAU * j as f64 / 96.0;
                    DVec3::new((s.x - m) * a.cos(), sy * (s.y - m), (s.x - m) * a.sin())
                })
                .collect()
        }
    }

    /// The own edge of shape `i` nearest to `p` (or edge `force`), with its
    /// distance and pick: box edges and cylinder rims, one at a time.
    fn own(&self, scene: &Scene, i: usize, p: DVec3, tol: f64, force: Option<usize>) -> Option<(f64, EdgePick)> {
        let (id, f) = &self.shapes[i];
        let node = &scene.nodes[id];
        node.shape()?;
        let pr = f.prims.first()?;
        if pr.taper.abs() > 1e-4 || !matches!(pr.kind, 0 | 1) {
            return None;
        }
        let n_edges = if pr.kind == 0 { 12 } else { 2 };
        let insts = f.instances(0);
        // nearest copy and its local point
        let (inst, lp) = insts
            .iter()
            .map(|c| (c, c.to_local.transform_point3(p)))
            .min_by(|a, b| prim_raw(pr, a.1).abs().partial_cmp(&prim_raw(pr, b.1).abs()).unwrap())?;
        let seg_dist = |line: &[DVec3]| -> f64 {
            let mut d = f64::INFINITY;
            for w in line.windows(2) {
                let e = w[1] - w[0];
                let t = ((lp - w[0]).dot(e) / e.length_squared().max(1e-18)).clamp(0.0, 1.0);
                d = d.min(lp.distance(w[0] + e * t));
            }
            d
        };
        let (edge, line, near) = match force {
            Some(e) if e < n_edges => {
                let l = Self::edge_line(pr, e);
                let d = seg_dist(&l);
                (e, l, d)
            }
            Some(_) => return None,
            None => (0..n_edges)
                .map(|e| {
                    let l = Self::edge_line(pr, e);
                    let d = seg_dist(&l);
                    (e, l, d)
                })
                .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())?,
        };
        let r = Self::edge_r(pr, edge);
        let near = near * pr.sc;
        if near > tol + r.abs() * pr.sc {
            return None;
        }
        // every copy shares the edge's fillet: show them all, cut where hidden
        let mut curves: Vec<Vec<Vec3>> = vec![];
        let keep = (r.abs() * pr.sc * 0.5).max(self.extent * 2e-3);
        let mut ordered: Vec<&crate::bake::field::Instance> = vec![inst];
        ordered.extend(insts.iter().filter(|c| c.copy != inst.copy));
        for c in ordered {
            let mut cur: Vec<Vec3> = vec![];
            for x in &line {
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
        if curves.is_empty() {
            return None;
        }
        let at = curves[0].iter().min_by(|a, b| a.as_dvec3().distance(p).partial_cmp(&b.as_dvec3().distance(p)).unwrap()).copied()?;
        let label = match (pr.kind, edge) {
            (0, _) => format!("Arête de {}", node.name),
            (_, 0) => format!("Bord haut de {}", node.name),
            _ => format!("Bord bas de {}", node.name),
        };
        Some((
            (near - r.abs() * pr.sc * 0.7).max(0.0),
            EdgePick {
                kind: EdgeKind::Own { node: *id, edge: edge as u32 },
                owner: *id,
                curves,
                at,
                normal: Vec3::Y,
                radius: (r.abs() as f32),
                chamfer: r < 0.0,
                label,
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
        assert_eq!(e.curves.len(), 1, "one edge at a time");
        // in the middle of a face: nothing
        assert!(idx.pick(&s, Vec3::new(-6.0, 10.0, -6.0), 0.5).is_none());
    }

    #[test]
    fn one_edge_fillet() {
        let mut s = cube_hole();
        let idx = EdgeIndex::new(&s);
        let e = idx.pick(&s, Vec3::new(9.6, 9.6, 3.0), 0.5).expect("edge");
        let EdgeKind::Own { node, edge } = e.kind else { panic!("{:?}", e.kind) };
        let sh = s.node_mut(node).unwrap().shape_mut().unwrap();
        sh.set_edge(edge as usize, 3.0);
        assert_eq!(sh.edges.len(), 12);
        let prog = compile(&s);
        let f = Field::new(&prog);
        // that edge is rounded by 3 mm, the opposite one keeps 1 mm
        let d45 = std::f64::consts::FRAC_1_SQRT_2;
        let on = |r: f64, sx: f64| DVec3::new(sx * (10.0 - r + r * d45), 10.0 - r + r * d45, 3.0);
        assert!(f.dist(on(3.0, 1.0)).abs() < 1e-4, "{}", f.dist(on(3.0, 1.0)));
        assert!(f.dist(on(1.0, -1.0)).abs() < 1e-4, "{}", f.dist(on(1.0, -1.0)));
        // f32 evaluator agrees
        assert!((prog.dist(on(3.0, 1.0).as_vec3())).abs() < 1e-3);
        // and the edge is picked again with its new radius
        let idx = EdgeIndex::new(&s);
        let e2 = idx.refresh(&s, &e).expect("refresh");
        assert!((e2.radius - 3.0).abs() < 1e-6);
    }
}

