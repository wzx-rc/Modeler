//! The Knife (K, Romain 19 nov.: "like the Maya Multicut tool") and Point
//! on edge (J), from Forme Rectify: clicks on the part's edges put points
//! in them; with the Knife, each point after the first is joined to the
//! one before by a line cut into the face holding both (straight on a flat
//! face; across a curved one, straight in the face's own grid). One
//! history step per cut (`Feature::cut`), ended by Enter, Esc, a right
//! click or a click off the edges.
//!
//! Loop cuts (Romain 19 nov., 23 h, then 23 h 18: "Click normal comme
//! actuellement, CTRL coupe à travers face, CTRL + SHIFT : loop"): with the
//! Knife, Ctrl + click on an edge cuts straight across the face under the
//! pointer (a four-sided face: to its opposite edge at the same fraction,
//! as Maya's edge loop; a flat face: square to the edge, to the first
//! border met); Ctrl + Shift, the cut runs on from face to face the same
//! way all round the part (it stops at a curved face that is not
//! four-sided, or a closed edge). The pointer shows it before.

use crate::state::*;
use glam::{DVec3, Vec3};
use nurbs_kernel::model::{Model, SurfKind};
use nurbs_kernel::{FeatureId, Knife, KnifePoint};

impl AppState {
    /// The edge under the pointer for the Knife: its index, the point on it
    /// and how far along it (0 to 1, from its first point); its middle
    /// catches the pointer within 2.5 points.
    pub fn knife_pick(&self, p: [f32; 2]) -> Option<(usize, DVec3, f64)> {
        let e = self.pick_edge(p)?;
        let m = self.model.as_ref()?;
        let pts: Vec<DVec3> = m.edges[e].pts.iter().map(|q| Vec3::from_array(*q).as_dvec3()).collect();
        if pts.len() < 2 {
            return None;
        }
        let len: f64 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        if len < 1e-9 {
            return None;
        }
        let (ro, rd) = self.ray(p);
        // the point of the edge nearest the pointer's ray
        let mut best = (f64::MAX, 0.0, pts[0]);
        let mut run = 0.0;
        for w in pts.windows(2) {
            let d = w[1] - w[0];
            let l = d.length();
            for k in 0..=16 {
                let t = k as f64 / 16.0;
                let q = w[0] + d * t;
                let dist = (q - ro).cross(rd).length();
                if dist < best.0 {
                    best = (dist, (run + l * t) / len, q);
                }
            }
            run += l;
        }
        // the middle: by length along the polyline
        let mut half = len * 0.5;
        let mut mid = pts[0];
        for w in pts.windows(2) {
            let l = (w[1] - w[0]).length();
            if half <= l {
                mid = w[0] + (w[1] - w[0]) * (half / l.max(1e-12));
                break;
            }
            half -= l;
        }
        // the middle catches only when the pointer is right on it (Romain,
        // 19 nov. 23 h: a point placed seemed to snap)
        if let Some(s) = self.view().project(mid.as_vec3()) {
            if (s[0] - p[0]).hypot(s[1] - p[1]) <= 2.5 {
                return Some((e, mid, 0.5));
            }
        }
        Some((e, best.2, best.1))
    }

    /// The loop cut a click at `p` would make (with the Knife, no cut
    /// under way): the points on the edges in order and whether it closes.
    /// `all`: through every four-sided face round the part (Ctrl), else
    /// across the face under the pointer only.
    pub fn knife_loop_at(&self, p: [f32; 2], all: bool) -> Option<(Vec<LoopPt>, bool)> {
        let (e, at, f) = self.knife_pick(p)?;
        let m = self.model.as_ref()?;
        let size = m.bounds().map(|(lo, hi)| (Vec3::from(hi) - Vec3::from(lo)).length() as f64).unwrap_or(100.0);
        let tol = (size * 1e-4).max(1e-4) * 10.0;
        let start = LoopPt { e, f, at };
        let faces: Vec<i32> = m.edges[e].faces.iter().copied().filter(|&fi| fi >= 0).collect();
        if faces.is_empty() || m.edges[e].seam {
            return None;
        }
        if !all {
            // the face under the pointer, else the one facing the eye most
            let (_, rd) = self.ray(p);
            let under = self.pick_at(p).map(|h| h.face as i32).filter(|fi| faces.contains(fi));
            let fi = under.unwrap_or_else(|| {
                *faces
                    .iter()
                    .min_by(|a, b| {
                        let d = |fi: i32| DVec3::from_array(m.faces[fi as usize].normal).dot(rd);
                        d(**a).total_cmp(&d(**b))
                    })
                    .unwrap()
            });
            if let Some((o, f2)) = quad_step(m, fi, e, f, tol) {
                let q = LoopPt { e: o, f: f2, at: at_frac(&poly(m, o), f2) };
                return Some((vec![start, q], false));
            }
            return across_flat(m, fi, e, f, at, tol).map(|q| (vec![start, q], false));
        }
        // Ctrl: on round the part, one way then (if open) the other
        let walk = |first: i32| -> (Vec<LoopPt>, bool) {
            let mut out = vec![];
            let (mut cur, mut face) = (start, first);
            let mut seen = vec![e];
            for _ in 0..400 {
                // a four-sided face: to its opposite edge; a flat one: straight across
                let step = quad_step(m, face, cur.e, cur.f, tol)
                    .map(|(o, f2)| LoopPt { e: o, f: f2, at: at_frac(&poly(m, o), f2) })
                    .or_else(|| across_flat(m, face, cur.e, cur.f, cur.at, tol));
                let Some(q) = step else { break };
                if q.e == e {
                    return (out, true);
                }
                if seen.contains(&q.e) {
                    break;
                }
                seen.push(q.e);
                out.push(q);
                let ef = m.edges[q.e].faces;
                let next = if ef[0] == face { ef[1] } else { ef[0] };
                if next < 0 || next == face || m.edges[q.e].seam {
                    break;
                }
                (cur, face) = (q, next);
            }
            (out, false)
        };
        let (a, closed) = walk(faces[0]);
        if closed {
            let mut v = vec![start];
            v.extend(a);
            return (v.len() >= 3).then_some((v, true));
        }
        let b = if faces.len() > 1 && faces[1] != faces[0] { walk(faces[1]).0 } else { vec![] };
        let mut v: Vec<LoopPt> = b.into_iter().rev().collect();
        v.push(start);
        v.extend(a);
        (v.len() >= 2).then_some((v, false))
    }

    /// Makes the loop cut a click at `p` shows (see `knife_loop_at`): one
    /// history step. False when there is none there.
    pub fn knife_cut_loop(&mut self, p: [f32; 2], all: bool) -> bool {
        let Some((pts, closed)) = self.knife_loop_at(p, all) else { return false };
        let mut kps = vec![];
        for q in &pts {
            let Some(edge) = self.edge_ref(q.e) else { return false };
            kps.push(KnifePoint { edge, f: q.f, at: q.at });
        }
        let n = kps.len();
        let mut segs: Vec<[usize; 2]> = (1..n).map(|i| [i - 1, i]).collect();
        if closed && n >= 3 {
            segs.push([n - 1, 0]);
        }
        let name = if all { "Loop cut" } else { "Face cut" };
        self.edit(name, |d| {
            d.add_knife(Knife { pts: kps, segs, remove_faces: vec![], remove_edges: vec![] }, name);
        });
        self.knife_end();
        self.knife_preview = None;
        true
    }

    /// The Knife (K) or Point on edge (J) armed.
    pub fn arm_knife(&mut self, t: Tool) {
        self.abort_gestures();
        self.tool = t;
        self.edge_sel.clear();
        self.face_sel.clear();
        self.vert_sel.clear();
    }

    pub fn knife_hover_at(&mut self, p: [f32; 2]) {
        self.knife_hover = self.knife_pick(p).map(|k| k.1);
    }

    /// The cut under way, if its step is still the last one made (not the
    /// last in the list: with an element's history marker back, a new step
    /// goes in at the marker, before those left out; each click then made
    /// a Knife of its own, 20 nov.).
    fn knife_going(&self) -> Option<(FeatureId, usize)> {
        let (id, last) = self.knife?;
        let f = self.doc.feature(id)?;
        (id + 1 == self.doc.next_id && f.cut.is_some()).then_some((id, last))
    }

    /// A click with the Knife or Point on edge: a point cut into the edge
    /// under the pointer; with the Knife, a line from the point before when
    /// both lie on one face (across a curved one it follows the face). Off
    /// the edges: the cut ends.
    pub fn knife_click(&mut self, p: [f32; 2]) {
        self.knife_click_with(p, false, false)
    }

    /// A click with the Knife: a plain click a point (joined to the one
    /// before), Ctrl a cut across the face, Ctrl + Shift a loop round the
    /// part (Romain, 19 nov. 23 h 18).
    pub fn knife_click_with(&mut self, p: [f32; 2], shift: bool, ctrl: bool) {
        if self.tool == Tool::Knife && ctrl {
            if !self.knife_cut_loop(p, shift) {
                self.toast("Nothing to cut across there", true);
            }
            return;
        }
        let Some((e, at, f)) = self.knife_pick(p) else {
            self.knife_end();
            return;
        };
        let Some(edge) = self.edge_ref(e) else { return };
        let kp = KnifePoint { edge, f, at };
        let lines = self.tool == Tool::Knife;
        match self.knife_going() {
            None => {
                let name = if lines { "Knife" } else { "Points" };
                let mut id = 0;
                self.edit(name, |d| id = d.add_knife(Knife { pts: vec![kp], segs: vec![], remove_faces: vec![], remove_edges: vec![] }, name));
                self.knife = Some((id, 0));
            }
            Some((id, last)) => {
                let Some(k) = self.doc.feature(id).and_then(|f| f.cut.clone()) else { return };
                let from = k.pts[last].at;
                let line = lines && from.distance(at) > 1e-6 && self.on_one_face(e, from, at);
                let i = k.pts.len();
                self.edit(if lines { "Knife" } else { "Point on edge" }, |d| {
                    if let Some(c) = d.feature_mut(id).and_then(|f| f.cut.as_mut()) {
                        c.pts.push(kp);
                        if line {
                            c.segs.push([last, i]);
                        }
                    }
                });
                if lines && !line {
                    self.toast("Not on one face with the point before: a point only", true);
                }
                self.knife = Some((id, i));
            }
        }
    }

    /// Both points lie on one face of edge `e`: a flat one (anywhere on
    /// its plane), or a curved one whose border passes through `a` (the
    /// line then follows the face, straight in its own grid).
    fn on_one_face(&self, e: usize, a: DVec3, b: DVec3) -> bool {
        let Some(m) = self.model.as_ref() else { return false };
        let size = m.bounds().map(|(lo, hi)| (Vec3::from(hi) - Vec3::from(lo)).length() as f64).unwrap_or(100.0);
        let tol = (size * 1e-4).max(1e-4) * 10.0;
        // the faces round `a`: those of the edges passing through it
        let near = |pts: &[[f32; 3]], a: DVec3| {
            pts.windows(2).any(|w| {
                let (p, q) = (DVec3::from(Vec3::from(w[0])), DVec3::from(Vec3::from(w[1])));
                let d = q - p;
                let t = if d.length_squared() > 0.0 { ((a - p).dot(d) / d.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
                (p + d * t).distance(a) < tol
            })
        };
        let round_a: Vec<i32> = m.edges.iter().filter(|g| near(&g.pts, a)).flat_map(|g| g.faces).filter(|&f| f >= 0).collect();
        // the faces `b` may lie on: its edge's, and at a corner those of
        // every edge leaving it (a click there picks any one of them)
        let mut faces: Vec<i32> = m.edges[e].faces.to_vec();
        faces.extend(m.edges.iter().filter(|g| near(&g.pts, b)).flat_map(|g| g.faces));
        faces.sort();
        faces.dedup();
        faces.iter().filter(|&&fi| fi >= 0).any(|&fi| {
            let fc = &m.faces[fi as usize];
            if fc.kind != SurfKind::Plane {
                return round_a.contains(&fi);
            }
            let n = DVec3::from_array(fc.normal).normalize_or_zero();
            let o = DVec3::from_array(fc.point);
            (a - o).dot(n).abs() < tol && (b - o).dot(n).abs() < tol
        })
    }

    /// The cut ends (Enter, Esc, a right click, a click off the edges):
    /// the next click starts another.
    pub fn knife_end(&mut self) {
        self.knife = None;
    }

    /// The last point of the cut under way (the line to the pointer starts
    /// there).
    pub fn knife_last(&self) -> Option<DVec3> {
        let (id, last) = self.knife_going()?;
        self.doc.feature(id)?.cut.as_ref()?.pts.get(last).map(|p| p.at)
    }
}

/// A point of a loop cut: the edge (index in `Model::edges`), how far along
/// it by length from its first point, the point.
#[derive(Clone, Copy, Debug)]
pub struct LoopPt {
    pub e: usize,
    pub f: f64,
    pub at: DVec3,
}

fn poly(m: &Model, e: usize) -> Vec<DVec3> {
    m.edges[e].pts.iter().map(|q| Vec3::from_array(*q).as_dvec3()).collect()
}

/// The point at fraction `f` of a polyline's length.
fn at_frac(pts: &[DVec3], f: f64) -> DVec3 {
    let len: f64 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    let mut left = len * f.clamp(0.0, 1.0);
    for w in pts.windows(2) {
        let l = (w[1] - w[0]).length();
        if left <= l {
            return w[0] + (w[1] - w[0]) * (left / l.max(1e-12));
        }
        left -= l;
    }
    *pts.last().unwrap_or(&DVec3::ZERO)
}

/// The edges round face `fi` (seams aside).
fn face_edges(m: &Model, fi: i32) -> Vec<usize> {
    (0..m.edges.len()).filter(|&i| !m.edges[i].seam && m.edges[i].faces.contains(&fi) && m.edges[i].pts.len() >= 2).collect()
}

/// Across a four-sided face `fi` from edge `e` at fraction `f`: the
/// opposite edge and the same fraction on it (counted from the end on the
/// same side). None when the face is not four-sided.
fn quad_step(m: &Model, fi: i32, e: usize, f: f64, tol: f64) -> Option<(usize, f64)> {
    let es = face_edges(m, fi);
    if es.len() != 4 || !es.contains(&e) {
        return None;
    }
    let ends = |i: usize| {
        let p = poly(m, i);
        (p[0], *p.last().unwrap())
    };
    if es.iter().any(|&i| {
        let (a, b) = ends(i);
        a.distance(b) < tol
    }) {
        return None;
    }
    let (c0, c1) = ends(e);
    let touches = |i: usize, q: DVec3| {
        let (a, b) = ends(i);
        a.distance(q) < tol || b.distance(q) < tol
    };
    let others: Vec<usize> = es.iter().copied().filter(|&i| i != e).collect();
    let opp: Vec<usize> = others.iter().copied().filter(|&i| !touches(i, c0) && !touches(i, c1)).collect();
    let [o] = opp[..] else { return None };
    // the side from c0: its other end is where the opposite's fraction 0 is
    let side = others.iter().copied().find(|&i| i != o && touches(i, c0))?;
    let (s0, s1) = ends(side);
    let x = if s0.distance(c0) < tol { s1 } else { s0 };
    let (o0, o1) = ends(o);
    if o0.distance(x) < tol {
        Some((o, f))
    } else if o1.distance(x) < tol {
        Some((o, 1.0 - f))
    } else {
        None
    }
}

/// Across a flat face `fi` from the point `at` of edge `e`: square to the
/// edge in the face's plane, to the first border edge met inside the face.
fn across_flat(m: &Model, fi: i32, e: usize, f: f64, at: DVec3, tol: f64) -> Option<LoopPt> {
    let fc = &m.faces[fi as usize];
    if fc.kind != SurfKind::Plane {
        return None;
    }
    let n = DVec3::from_array(fc.normal).normalize_or_zero();
    let pe = poly(m, e);
    // the edge's direction at the point
    let len: f64 = pe.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    let mut left = len * f;
    let mut t = pe[pe.len() - 1] - pe[0];
    for w in pe.windows(2) {
        let l = (w[1] - w[0]).length();
        if left <= l {
            t = w[1] - w[0];
            break;
        }
        left -= l;
    }
    let d = n.cross(t).normalize_or_zero();
    if d == DVec3::ZERO {
        return None;
    }
    let tris: Vec<[DVec3; 3]> = m
        .triangles
        .iter()
        .zip(&m.tri_face)
        .filter(|(_, &tf)| tf as i32 == fi)
        .map(|(t, _)| t.map(|i| Vec3::from_array(m.positions[i as usize]).as_dvec3()))
        .collect();
    let inside = |q: DVec3| tris.iter().any(|tr| in_tri(q, tr, n, tol));
    let mut best: Option<(f64, LoopPt)> = None;
    for dir in [d, -d] {
        let mut hit: Option<(f64, usize, DVec3)> = None;
        for i in face_edges(m, fi) {
            if i == e {
                continue;
            }
            let p = poly(m, i);
            for w in p.windows(2) {
                // the ray at + s dir against the piece w0 + u (w1 − w0), in the plane
                let g = w[1] - w[0];
                let a = dir.cross(g).dot(n);
                if a.abs() < 1e-12 {
                    continue;
                }
                let r = w[0] - at;
                let s = r.cross(g).dot(n) / a;
                let u = r.cross(dir).dot(n) / a;
                if s > tol && (-1e-6..=1.0 + 1e-6).contains(&u) && hit.map(|h| s < h.0).unwrap_or(true) {
                    hit = Some((s, i, w[0] + g * u.clamp(0.0, 1.0)));
                }
            }
        }
        let Some((s, i, q)) = hit else { continue };
        if !inside(at + dir * (s * 0.5)) {
            continue;
        }
        // its fraction along that edge by length
        let p = poly(m, i);
        let tot: f64 = p.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let mut run = 0.0;
        let mut fq = 0.0;
        for w in p.windows(2) {
            let g = w[1] - w[0];
            let l = g.length();
            let u = if l > 0.0 { ((q - w[0]).dot(g) / (l * l)).clamp(0.0, 1.0) } else { 0.0 };
            if (w[0] + g * u).distance(q) < tol {
                fq = (run + l * u) / tot.max(1e-12);
                break;
            }
            run += l;
        }
        if best.as_ref().map(|b| s < b.0).unwrap_or(true) {
            best = Some((s, LoopPt { e: i, f: fq, at: q }));
        }
    }
    best.map(|b| b.1)
}

/// `q` lies in the triangle (seen along the face's normal `n`).
fn in_tri(q: DVec3, t: &[DVec3; 3], n: DVec3, tol: f64) -> bool {
    let side = |a: DVec3, b: DVec3| (b - a).cross(q - a).dot(n);
    let (x, y, z) = (side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0]));
    let e = tol * tol;
    (x >= -e && y >= -e && z >= -e) || (x <= e && y <= e && z <= e)
}
