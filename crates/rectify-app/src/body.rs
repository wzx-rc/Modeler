//! A shape drawn on the scan (the Pen, the Circle): a flat outline on a
//! plane (closed, a filled face; open, a line), its corners rounded, then
//! the steps done to it in order (an open edge pulled into a new face, a
//! thickness, edges filleted), built as an exact B-rep by OpenCascade
//! (Romain, 18 nov. 2026, 22 h 30). The scan is never touched.
//!
//! Faces carry tags (`forme_occt::Shape::set_tags`): the outline's face
//! `BASE`, a step's new faces `step_tag(k, …)`, a thickness's top and side
//! faces a mix of the face they come from and the step. Edges are never
//! named by their index in OpenCascade (it changes with every build): an
//! `EdgeRef` holds the tags of its two faces and its middle, and is found
//! again in each build (`resolve`).

use forme_occt::{Shape, Topo};
use glam::{DVec2, DVec3};
use std::sync::Arc;

/// A plane with its axes: `o` the origin, `x` and `y` in it, `n` = x × y.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub o: DVec3,
    pub x: DVec3,
    pub y: DVec3,
    pub n: DVec3,
}

impl Frame {
    /// The plane through `o` facing `n`, its x along `hint` laid on it
    /// (else any direction in it).
    pub fn new(o: DVec3, n: DVec3, hint: DVec3) -> Frame {
        let n = n.normalize_or_zero();
        let n = if n == DVec3::ZERO { DVec3::Y } else { n };
        let mut x = hint - n * hint.dot(n);
        if x.length() < 1e-6 {
            x = n.any_orthonormal_vector();
        }
        let x = x.normalize();
        let y = n.cross(x);
        Frame { o, x, y, n }
    }

    /// The world's X laid on the plane (Z when the plane faces X).
    pub fn level(o: DVec3, n: DVec3) -> Frame {
        Frame::new(o, n, if n.normalize_or_zero().x.abs() < 0.9 { DVec3::X } else { DVec3::Z })
    }

    pub fn to3(&self, q: DVec2) -> DVec3 {
        self.o + self.x * q.x + self.y * q.y
    }

    pub fn to2(&self, p: DVec3) -> DVec2 {
        let d = p - self.o;
        DVec2::new(d.dot(self.x), d.dot(self.y))
    }

    /// Where a ray meets the plane, in front of its origin.
    pub fn hit(&self, o: DVec3, d: DVec3) -> Option<DVec3> {
        let dn = d.dot(self.n);
        if dn.abs() < 1e-9 {
            return None;
        }
        let s = (self.o - o).dot(self.n) / dn;
        (s > 0.0).then(|| o + d * s)
    }
}

/// The outline a body starts from.
#[derive(Clone, Debug, PartialEq)]
pub enum Base {
    /// Corners in the plane (its x, y): closed, a filled face; open, a line.
    Poly { pts: Vec<DVec2>, closed: bool },
    /// A disc round the frame's origin.
    Circle { r: f64 },
}

/// An edge found again after the body is rebuilt: the tags of the faces on
/// its two sides (-1: none, an open edge) and its middle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeRef {
    pub faces: [i64; 2],
    pub at: DVec3,
}

/// What is done to the outline, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// An open edge pulled along `v` into a new face.
    Extrude { edge: EdgeRef, v: DVec3 },
    /// The faces made a solid `t` thick, along their normal (< 0: the
    /// other way).
    Thick { t: f64 },
    /// Edges between two faces rounded.
    Fillet { edges: Vec<EdgeRef>, r: f64 },
}

impl Step {
    pub fn label(&self) -> &'static str {
        match self {
            Step::Extrude { .. } => "Extrude edge",
            Step::Thick { .. } => "Thickness",
            Step::Fillet { .. } => "Fillet",
        }
    }
}

/// The outline's face.
pub const BASE: i64 = 1;

/// The tag of step `k`'s new faces.
pub fn step_tag(k: usize, sub: i64) -> i64 {
    ((k as i64 + 1) << 20) | sub
}

/// Which step made a face with this tag (None: the outline, or a
/// thickness's faces, mixed).
pub fn step_of(tag: i64) -> Option<usize> {
    let k = tag >> 20;
    (tag > 0 && (1..=4096).contains(&k) && (tag & 0xFFFFF) < 64).then(|| (k - 1) as usize)
}

/// What OpenCascade made of a body.
#[derive(Clone, Debug, Default)]
pub struct Built {
    /// Faces (with their tags), triangles (each with its face) and edges.
    pub topo: Topo,
    pub verts: Vec<DVec3>,
    /// The first step that could not be made and why (the ones after it
    /// are left out too; `usize::MAX`: the outline itself).
    pub failed: Option<(usize, String)>,
    /// Fillets too big: (step, the largest radius that worked, built at it).
    pub limits: Vec<(usize, f64)>,
    /// Its size (the largest side of its box, mm).
    pub size: f64,
}

impl Built {
    /// An edge's middle point and its direction there.
    pub fn edge_mid(&self, e: usize) -> (DVec3, DVec3) {
        let pts = &self.topo.edges[e].pts;
        let p = |i: usize| DVec3::from_array(pts[i]);
        let n = pts.len();
        if n < 2 {
            return (p(0), DVec3::X);
        }
        // the point halfway along it
        let len: f64 = (1..n).map(|i| (p(i) - p(i - 1)).length()).sum();
        let mut acc = 0.0;
        for i in 1..n {
            let l = (p(i) - p(i - 1)).length();
            if acc + l >= len * 0.5 && l > 0.0 {
                let t = (len * 0.5 - acc) / l;
                return (p(i - 1).lerp(p(i), t), (p(i) - p(i - 1)) / l);
            }
            acc += l;
        }
        (p(n / 2), (p(n - 1) - p(0)).normalize_or_zero())
    }

    /// The tags of an edge's faces (-1: none).
    pub fn edge_tags(&self, e: usize) -> [i64; 2] {
        let f = self.topo.edges[e].faces;
        let t = |i: i32| if i >= 0 { self.topo.faces[i as usize].tag } else { -1 };
        [t(f[0]), t(f[1])]
    }

    /// An edge with a face on one side only (or none: a line's).
    pub fn edge_open(&self, e: usize) -> bool {
        let f = self.topo.edges[e].faces;
        (f[0] < 0 || f[1] < 0) && !self.topo.edges[e].seam
    }

    pub fn edge_ref(&self, e: usize) -> EdgeRef {
        EdgeRef { faces: self.edge_tags(e), at: self.edge_mid(e).0 }
    }

    /// The edge an `EdgeRef` names in this build.
    pub fn find_edge(&self, r: &EdgeRef) -> Option<usize> {
        let list: Vec<([i64; 2], DVec3)> = (0..self.topo.edges.len()).map(|e| (self.edge_tags(e), self.edge_mid(e).0)).collect();
        resolve(r, &list, self.size)
    }

    /// The face with this tag nearest to `at`.
    pub fn find_face(&self, tag: i64, at: DVec3) -> Option<usize> {
        self.topo
            .faces
            .iter()
            .enumerate()
            .filter(|(_, f)| f.tag == tag)
            .min_by(|a, b| (DVec3::from_array(a.1.info.point) - at).length().partial_cmp(&(DVec3::from_array(b.1.info.point) - at).length()).unwrap())
            .map(|(i, _)| i)
    }

    /// The vertex at `at` (within a hair).
    pub fn find_vertex(&self, at: DVec3) -> Option<usize> {
        let tol = (self.size * 1e-4).max(1e-4);
        self.verts.iter().position(|v| (*v - at).length() <= tol)
    }

    pub fn has_faces(&self) -> bool {
        !self.topo.faces.is_empty()
    }

    /// A solid (closed: every edge has two faces).
    pub fn solid(&self) -> bool {
        self.has_faces() && (0..self.topo.edges.len()).all(|e| !self.edge_open(e))
    }
}

/// Which edge of a list (each: its faces' tags, its middle) an `EdgeRef`
/// names: the nearest of those with the same faces, else of those sharing
/// one face, else the nearest of all (within a tenth of the body's size).
pub fn resolve(r: &EdgeRef, list: &[([i64; 2], DVec3)], size: f64) -> Option<usize> {
    let same = |t: &[i64; 2]| (t[0] == r.faces[0] && t[1] == r.faces[1]) || (t[0] == r.faces[1] && t[1] == r.faces[0]);
    let one = |t: &[i64; 2]| r.faces.iter().any(|&f| f > 0 && t.contains(&f));
    let nearest = |keep: &dyn Fn(&[i64; 2]) -> bool| {
        list.iter().enumerate().filter(|(_, (t, _))| keep(t)).min_by(|a, b| (a.1 .1 - r.at).length().partial_cmp(&(b.1 .1 - r.at).length()).unwrap()).map(|(i, _)| i)
    };
    nearest(&same).or_else(|| nearest(&one)).or_else(|| {
        let i = nearest(&|_| true)?;
        ((list[i].1 - r.at).length() <= (size * 0.1).max(0.5)).then_some(i)
    })
}

/// A body: its plane, its outline, its corners' radii, its steps, and what
/// OpenCascade made of them (shared by the undo copies).
#[derive(Clone, Debug)]
pub struct Body {
    pub frame: Frame,
    pub base: Base,
    /// Each corner of a `Poly` outline rounded by this radius (0: sharp).
    pub corners: Vec<f64>,
    pub steps: Vec<Step>,
    pub built: Arc<Built>,
}

impl Body {
    pub fn new(frame: Frame, base: Base) -> Body {
        let n = match &base {
            Base::Poly { pts, .. } => pts.len(),
            Base::Circle { .. } => 0,
        };
        let mut b = Body { frame, base, corners: vec![0.0; n], steps: vec![], built: Arc::new(Built::default()) };
        b.rebuild();
        b
    }

    pub fn rebuild(&mut self) {
        self.built = Arc::new(build(self));
    }

    pub fn closed(&self) -> bool {
        match &self.base {
            Base::Poly { closed, .. } => *closed,
            Base::Circle { .. } => true,
        }
    }

    /// The outline's corners (3D).
    pub fn corner_points(&self) -> Vec<DVec3> {
        match &self.base {
            Base::Poly { pts, .. } => pts.iter().map(|q| self.frame.to3(*q)).collect(),
            Base::Circle { .. } => vec![],
        }
    }

    /// Corner `i` cut: its start, middle and end on the outline (2D), and
    /// the radius it got (at most half of each side next to it).
    pub fn corner_cut(&self, i: usize) -> Option<([DVec2; 3], f64)> {
        let Base::Poly { pts, closed } = &self.base else { return None };
        corner_cut(pts, *closed, i, self.corners.get(i).copied().unwrap_or(0.0))
    }

    /// The largest radius corner `i` can take.
    pub fn corner_max(&self, i: usize) -> f64 {
        let Base::Poly { pts, closed } = &self.base else { return 0.0 };
        corner_cut(pts, *closed, i, 1e9).map(|c| c.1).unwrap_or(0.0)
    }

    /// The outline's corner at a vertex: the corner itself, or an end of
    /// its arc when it is rounded (seen along the plane's normal: a
    /// thickness's top corners too).
    pub fn corner_at(&self, p: DVec3) -> Option<usize> {
        let Base::Poly { pts, .. } = &self.base else { return None };
        let q = self.frame.to2(p);
        let tol = (self.built.size * 1e-4).max(1e-4);
        // on the outline's plane, or on the thickness's top
        let h = self.frame.n.dot(p - self.frame.o);
        let t = self.thick().map(|s| s.1).unwrap_or(0.0);
        if h.abs() > tol * 10.0 && (h - t).abs() > tol * 10.0 {
            return None;
        }
        (0..pts.len()).find(|&i| match self.corner_cut(i) {
            Some((c, _)) => (c[0] - q).length() <= tol || (c[2] - q).length() <= tol,
            None => (pts[i] - q).length() <= tol,
        })
    }

    /// Where corner `i` shows (3D): the corner, or the middle of its arc.
    pub fn corner_show(&self, i: usize) -> Option<DVec3> {
        let Base::Poly { pts, .. } = &self.base else { return None };
        Some(match self.corner_cut(i) {
            Some((c, _)) => self.frame.to3(c[1]),
            None => self.frame.to3(*pts.get(i)?),
        })
    }

    /// The thickness step, if any (its index and value).
    pub fn thick(&self) -> Option<(usize, f64)> {
        self.steps.iter().enumerate().find_map(|(k, s)| match s {
            Step::Thick { t } => Some((k, *t)),
            _ => None,
        })
    }

    /// In a few words.
    pub fn describe(&self) -> String {
        let mut parts = vec![];
        match &self.base {
            Base::Poly { pts, closed } => {
                let rounded = self.corners.iter().filter(|r| **r > 0.0).count();
                parts.push(format!("{} {}{}", pts.len(), if *closed { "corners" } else { "points, open" }, if rounded > 0 { format!(" ({rounded} rounded)") } else { String::new() }));
            }
            Base::Circle { r } => parts.push(format!("circle Ø {}", fmt(r * 2.0, 2))),
        }
        for s in &self.steps {
            parts.push(match s {
                Step::Extrude { v, .. } => format!("edge pulled {} mm", fmt(v.length(), 1)),
                Step::Thick { t } => format!("{} mm thick", fmt(t.abs(), 2)),
                Step::Fillet { edges, r } => format!("fillet {} mm on {} edge{}", fmt(*r, 2), edges.len(), if edges.len() == 1 { "" } else { "s" }),
            });
        }
        if let Some((k, e)) = &self.built.failed {
            parts.push(if *k == usize::MAX { format!("not made: {e}") } else { format!("step {} not made", k + 1) });
        }
        parts.join(" · ")
    }
}

/// A number with a decimal comma.
fn fmt(v: f64, digits: usize) -> String {
    format!("{:.*}", digits, v).replace('.', ",")
}

/// Corner `i` of an outline rounded by `r`: the arc's start, middle and
/// end, and the radius it got (each side gives at most half of itself to
/// the corner at either end). None: a sharp corner (r = 0, a straight
/// corner, an open line's end).
pub fn corner_cut(pts: &[DVec2], closed: bool, i: usize, r: f64) -> Option<([DVec2; 3], f64)> {
    let n = pts.len();
    if r <= 1e-9 || n < 3 || (!closed && (i == 0 || i + 1 >= n)) {
        return None;
    }
    let p = pts[i];
    let a = pts[(i + n - 1) % n];
    let b = pts[(i + 1) % n];
    let (la, lb) = ((a - p).length(), (b - p).length());
    if la < 1e-9 || lb < 1e-9 {
        return None;
    }
    let (u, w) = ((a - p) / la, (b - p) / lb);
    let cos = u.dot(w).clamp(-1.0, 1.0);
    let th = cos.acos();
    if th > std::f64::consts::PI - 1e-3 || th < 1e-3 {
        return None;
    }
    let tan = (th * 0.5).tan();
    let d = (r / tan).min(la * 0.5).min(lb * 0.5);
    let r = d * tan;
    let bis = (u + w).normalize();
    let mid = p + bis * (r / (th * 0.5).sin() - r);
    Some(([p + u * d, mid, p + w * d], r))
}

fn v3(p: DVec3) -> [f64; 3] {
    p.to_array()
}

/// The outline as OpenCascade takes it.
fn base_shape(b: &Body) -> Result<Shape, String> {
    match &b.base {
        Base::Circle { r } => {
            if *r <= 1e-6 {
                return Err("the circle has no size".into());
            }
            let mut s = Shape::disc(v3(b.frame.o), v3(b.frame.n), *r)?;
            s.set_tags(&[BASE]);
            Ok(s)
        }
        Base::Poly { pts, closed } => {
            let p3: Vec<[f64; 3]> = pts.iter().map(|q| v3(b.frame.to3(*q))).collect();
            let cuts: Vec<Option<[[f64; 3]; 3]>> = (0..pts.len()).map(|i| b.corner_cut(i).map(|(c, _)| [v3(b.frame.to3(c[0])), v3(b.frame.to3(c[1])), v3(b.frame.to3(c[2]))])).collect();
            let mut s = Shape::outline(v3(b.frame.o), v3(b.frame.n), &p3, *closed, &cuts)?;
            if *closed {
                s.set_tags(&[BASE]);
            }
            Ok(s)
        }
    }
}

/// Every edge of a shape: its index, its faces' tags, its middle.
fn edge_list(s: &Shape) -> Vec<(i32, [i64; 2], DVec3)> {
    let ef = s.edge_faces();
    let tag = |f: i32| if f >= 0 { s.face_tag(f as usize) } else { -1 };
    s.edges(9)
        .into_iter()
        .filter_map(|e| {
            let f = ef.get((e.index - 1) as usize).copied().unwrap_or([-1, -1]);
            let mid = DVec3::from_array(e.pts[e.pts.len() / 2]);
            Some((e.index, [tag(f[0]), tag(f[1])], mid))
        })
        .collect()
}

fn find(s: &Shape, r: &EdgeRef, size: f64) -> Option<i32> {
    let list = edge_list(s);
    let l: Vec<([i64; 2], DVec3)> = list.iter().map(|e| (e.1, e.2)).collect();
    resolve(r, &l, size).map(|i| list[i].0)
}

/// The body's outline, then its steps, by OpenCascade.
fn build(b: &Body) -> Built {
    let mut out = Built::default();
    let mut shape = match base_shape(b) {
        Ok(s) => s,
        Err(e) => {
            out.failed = Some((usize::MAX, e));
            return out;
        }
    };
    let size_of = |s: &Shape| {
        let pts = s.vertex_points();
        let (mut lo, mut hi) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
        for p in &pts {
            lo = lo.min(DVec3::from_array(*p));
            hi = hi.max(DVec3::from_array(*p));
        }
        if pts.is_empty() {
            1.0
        } else {
            (hi - lo).max_element().max(0.1)
        }
    };
    let mut size = match &b.base {
        Base::Circle { r } => r * 2.0,
        _ => size_of(&shape),
    };
    for (k, st) in b.steps.iter().enumerate() {
        let made = match st {
            Step::Extrude { v, .. } if v.length() < 1e-6 => continue,
            Step::Extrude { edge, v } => match find(&shape, edge, size) {
                Some(i) => shape.extrude_edge(i, v3(*v), step_tag(k, 1)),
                None => Err("its edge is gone".into()),
            },
            Step::Thick { t } => {
                if t.abs() < 1e-4 {
                    continue;
                }
                shape.thicken(*t, k as i64 + 1)
            }
            Step::Fillet { edges, r } => {
                let idx: Vec<i32> = edges.iter().filter_map(|e| find(&shape, e, size)).collect();
                if idx.is_empty() {
                    Err("its edges are gone".into())
                } else if *r <= 1e-6 {
                    continue;
                } else {
                    match shape.round(&idx, *r, step_tag(k, 1)) {
                        Ok(s) => Ok(s),
                        // too big: the largest that works (by halves)
                        Err(e) => {
                            let (mut lo, mut hi) = (0.0, *r);
                            let mut best: Option<Shape> = None;
                            for _ in 0..7 {
                                let m = (lo + hi) * 0.5;
                                match shape.round(&idx, m, step_tag(k, 1)) {
                                    Ok(s) => {
                                        lo = m;
                                        best = Some(s);
                                    }
                                    Err(_) => hi = m,
                                }
                            }
                            match best {
                                Some(s) => {
                                    out.limits.push((k, lo));
                                    Ok(s)
                                }
                                None => Err(e),
                            }
                        }
                    }
                }
            }
        };
        match made {
            Ok(s) => {
                shape = s;
                size = size_of(&shape);
            }
            Err(e) => {
                out.failed = Some((k, e));
                break;
            }
        }
    }
    out.size = size;
    let defl = (size * 4e-4).clamp(0.002, 0.05);
    match shape.topo(defl, 0.2) {
        Ok(t) => out.topo = t,
        Err(e) => out.failed = Some((usize::MAX, e)),
    }
    out.verts = shape.vertex_points().into_iter().map(DVec3::from_array).collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(n: f64) -> Body {
        let f = Frame::new(DVec3::ZERO, DVec3::Y, DVec3::X);
        Body::new(f, Base::Poly { pts: vec![DVec2::new(0.0, 0.0), DVec2::new(n, 0.0), DVec2::new(n, n), DVec2::new(0.0, n)], closed: true })
    }

    #[test]
    fn a_square_face() {
        let b = square(10.0);
        assert!(b.built.failed.is_none(), "{:?}", b.built.failed);
        assert_eq!(b.built.topo.faces.len(), 1);
        assert_eq!(b.built.topo.edges.len(), 4);
        assert!(b.built.topo.edges.iter().enumerate().all(|(e, _)| b.built.edge_open(e)));
        // it faces the plane's normal
        let n = b.built.topo.faces[0].info.normal;
        assert!(n[1] > 0.99, "{n:?}");
    }

    #[test]
    fn corners_thickness_fillet() {
        let mut b = square(10.0);
        b.corners[0] = 2.0;
        b.rebuild();
        assert_eq!(b.built.topo.edges.len(), 5);
        assert_eq!(b.corner_at(b.frame.to3(DVec2::new(2.0, 0.0))), Some(0));
        b.steps.push(Step::Thick { t: 3.0 });
        b.rebuild();
        assert!(b.built.failed.is_none(), "{:?}", b.built.failed);
        assert!(b.built.solid());
        assert_eq!(b.built.topo.faces.len(), 7);
        // a top edge filleted
        let e = (0..b.built.topo.edges.len()).find(|&e| {
            let (m, d) = b.built.edge_mid(e);
            (m.y - 3.0).abs() < 1e-6 && d.x.abs() > 0.99 && m.z.abs() < 1e-6
        });
        let e = e.expect("the top front edge");
        let r = b.built.edge_ref(e);
        b.steps.push(Step::Fillet { edges: vec![r], r: 1.0 });
        b.rebuild();
        assert!(b.built.failed.is_none(), "{:?}", b.built.failed);
        // it runs on round the rounded corner and along the next side
        // (tangent edges go with it): three fillet faces
        assert_eq!(b.built.topo.faces.len(), 10);
        // thicker: the fillet stays on the top edge
        b.steps[0] = Step::Thick { t: 5.0 };
        b.rebuild();
        assert!(b.built.failed.is_none(), "{:?}", b.built.failed);
        assert!(b.built.topo.faces.iter().any(|f| f.info.kind == forme_occt::SurfKind::Cylinder && f.info.point[1] > 4.0));
    }

    #[test]
    fn an_edge_pulled_out() {
        let mut b = square(10.0);
        let e = (0..b.built.topo.edges.len()).find(|&e| b.built.edge_mid(e).0.z.abs() < 1e-6).unwrap();
        let r = b.built.edge_ref(e);
        b.steps.push(Step::Extrude { edge: r, v: DVec3::new(0.0, 5.0, -1.0) });
        b.rebuild();
        assert!(b.built.failed.is_none(), "{:?}", b.built.failed);
        assert_eq!(b.built.topo.faces.len(), 2);
        // the fold between them has a face on each side
        let fold = (0..b.built.topo.edges.len()).filter(|&e| !b.built.edge_open(e)).count();
        assert_eq!(fold, 1);
        // filleted, then thickened
        let f = (0..b.built.topo.edges.len()).find(|&e| !b.built.edge_open(e)).unwrap();
        let r = b.built.edge_ref(f);
        b.steps.push(Step::Fillet { edges: vec![r], r: 1.0 });
        b.rebuild();
        assert!(b.built.failed.is_none(), "{:?}", b.built.failed);
        assert_eq!(b.built.topo.faces.len(), 3);
        b.steps.push(Step::Thick { t: 1.0 });
        b.rebuild();
        assert!(b.built.failed.is_none(), "{:?}", b.built.failed);
        assert!(b.built.solid());
    }

    #[test]
    fn an_open_line() {
        let f = Frame::new(DVec3::ZERO, DVec3::Y, DVec3::X);
        let mut b = Body::new(f, Base::Poly { pts: vec![DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0), DVec2::new(10.0, 10.0)], closed: false });
        assert!(b.built.failed.is_none(), "{:?}", b.built.failed);
        assert_eq!(b.built.topo.faces.len(), 0);
        assert_eq!(b.built.topo.edges.len(), 2);
        b.corners[1] = 3.0;
        b.rebuild();
        assert_eq!(b.built.topo.edges.len(), 3);
        let e = 0;
        let r = b.built.edge_ref(e);
        b.steps.push(Step::Extrude { edge: r, v: DVec3::new(0.0, 4.0, 0.0) });
        b.rebuild();
        assert!(b.built.failed.is_none(), "{:?}", b.built.failed);
        assert_eq!(b.built.topo.faces.len(), 1);
    }

    #[test]
    fn a_cylinder() {
        let f = Frame::new(DVec3::ZERO, DVec3::Y, DVec3::X);
        let mut b = Body::new(f, Base::Circle { r: 4.0 });
        b.steps.push(Step::Thick { t: 6.0 });
        b.rebuild();
        assert!(b.built.failed.is_none(), "{:?}", b.built.failed);
        assert!(b.built.solid());
        assert_eq!(b.built.topo.faces.len(), 3);
        let rim = (0..b.built.topo.edges.len()).find(|&e| b.built.edge_mid(e).0.y > 5.9 && !b.built.topo.edges[e].seam).unwrap();
        let r = b.built.edge_ref(rim);
        b.steps.push(Step::Fillet { edges: vec![r], r: 1.0 });
        b.rebuild();
        assert!(b.built.failed.is_none(), "{:?}", b.built.failed);
        assert_eq!(b.built.topo.faces.len(), 4);
    }
}
