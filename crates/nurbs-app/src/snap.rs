//! Snapping to the part, Plasticity style: while a shape is created, a
//! sketch drawn, a shape moved or sized, the pointer catches the points of
//! interest of the B-rep the kernel sent (the ends and middles of edges,
//! the centres of circles and of flat faces, the world origin), or lines
//! up with them (dotted guides). Everything comes from the kernel's model:
//! no guess from the feature list.
//!
//! Ctrl held: no snapping to points (the grid step still applies).

use crate::state::{polyline_middle, AppState, BasePlane};
use glam::{DVec3, Vec3};
use nurbs_kernel::model::{EdgeKind, SurfKind};
use nurbs_kernel::{FeatureId, Model};

/// What a snap caught.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    /// The end of an edge (a corner).
    End,
    /// The middle of an edge.
    Mid,
    /// The centre of a circle or an arc.
    Center,
    /// The centre of a flat face.
    FaceCenter,
    /// The world origin.
    Origin,
    /// A point on an edge.
    OnEdge,
    /// Lined up with points of interest (one or two guides).
    Aligned,
}

impl SnapKind {
    pub fn label(self) -> &'static str {
        match self {
            SnapKind::End => "End",
            SnapKind::Mid => "Midpoint",
            SnapKind::Center => "Center",
            SnapKind::FaceCenter => "Face center",
            SnapKind::Origin => "Origin",
            SnapKind::OnEdge => "On edge",
            SnapKind::Aligned => "Aligned",
        }
    }
    /// Lower wins when two points are as close on screen.
    fn rank(self) -> f32 {
        match self {
            SnapKind::End | SnapKind::Center | SnapKind::Origin => 0.0,
            SnapKind::Mid => 2.0,
            SnapKind::FaceCenter => 4.0,
            SnapKind::OnEdge | SnapKind::Aligned => 6.0,
        }
    }
}

/// A point of interest of the part.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapPoint {
    pub pos: DVec3,
    pub kind: SnapKind,
    /// The faces it lies on (indices in the model), for the plane a shape
    /// starts on.
    pub faces: Vec<usize>,
    /// The features those faces come from (a shape being moved does not
    /// snap to itself).
    pub features: Vec<FeatureId>,
}

/// Where the pointer ended up, for the drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapped {
    /// The point used (on the plane, on the height axis, the moved pivot).
    pub pos: DVec3,
    pub kind: SnapKind,
    /// The point of interest it came from when it is elsewhere (a corner
    /// above the plane, a point whose height was taken): drawn with a
    /// dotted line to `pos`.
    pub source: Option<DVec3>,
    /// Dotted guides (from, to) of an alignment.
    pub guides: Vec<(DVec3, DVec3)>,
}

/// What a move carries along (Romain, 20 nov., 7 h 22: "aligner ce point
/// en rouge sur cette pièce sur le repère en jaune… pour l'instant ça le
/// fait que avec le centre"): its points of interest where they were when
/// the drag began, any of which lands on a point of interest it passes
/// near, and which of the part's points they are (not targets meanwhile).
#[derive(Clone, Debug, Default)]
pub struct Carried {
    pub pts: Vec<(DVec3, SnapKind)>,
    /// Indices in `AppState::snaps` (frozen while the drag is live).
    pub own: Vec<usize>,
}

/// Screen distance (points) under which a point of interest catches.
pub const CATCH: f32 = 12.0;
/// Screen distance under which a guide catches.
pub const ALIGN: f32 = 7.0;

fn d3(p: &[f32; 3]) -> DVec3 {
    DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64)
}

/// Centre of the circle through three points (None when they are in line).
pub fn circumcentre(a: DVec3, b: DVec3, c: DVec3) -> Option<DVec3> {
    let (ab, ac) = (b - a, c - a);
    let n = ab.cross(ac);
    let n2 = n.length_squared();
    if n2 < 1e-12 {
        return None;
    }
    Some(a + (n.cross(ab) * ac.length_squared() + ac.cross(n) * ab.length_squared()) / (2.0 * n2))
}

/// The points of interest of a model.
pub fn snap_points(m: &Model) -> Vec<SnapPoint> {
    let mut out: Vec<SnapPoint> = vec![];
    let feats = |faces: &[usize]| -> Vec<FeatureId> { faces.iter().filter_map(|f| m.faces.get(*f).and_then(|f| f.tag).map(|t| t.feature)).collect() };
    let push = |pos: DVec3, kind: SnapKind, faces: Vec<usize>, out: &mut Vec<SnapPoint>| {
        let features = feats(&faces);
        if let Some(q) = out.iter_mut().find(|q| q.kind == kind && q.pos.distance(pos) < 1e-3) {
            for f in faces {
                if !q.faces.contains(&f) {
                    q.faces.push(f);
                }
            }
            for f in features {
                if !q.features.contains(&f) {
                    q.features.push(f);
                }
            }
            return;
        }
        out.push(SnapPoint { pos, kind, faces, features });
    };
    push(DVec3::ZERO, SnapKind::Origin, vec![], &mut out);
    for e in m.edges.iter().filter(|e| !e.seam && e.pts.len() >= 2) {
        let faces: Vec<usize> = e.faces.iter().filter(|f| **f >= 0).map(|f| *f as usize).collect();
        let (a, b) = (d3(&e.pts[0]), d3(e.pts.last().unwrap()));
        let closed = a.distance(b) < 1e-4;
        if !closed {
            push(a, SnapKind::End, faces.clone(), &mut out);
            push(b, SnapKind::End, faces.clone(), &mut out);
            let mid = if e.kind == EdgeKind::Line { (a + b) * 0.5 } else { polyline_middle(&e.pts).unwrap_or((a + b) * 0.5) };
            push(mid, SnapKind::Mid, faces.clone(), &mut out);
        }
        if e.kind == EdgeKind::Circle {
            let n = e.pts.len() - if closed { 1 } else { 0 };
            if n >= 3 {
                let c = circumcentre(d3(&e.pts[0]), d3(&e.pts[n / 3]), d3(&e.pts[2 * n / 3]));
                if let Some(c) = c {
                    push(c, SnapKind::Center, faces.clone(), &mut out);
                }
            }
        }
    }
    // flat faces: the centre of their area
    let mut acc: Vec<(DVec3, f64)> = vec![(DVec3::ZERO, 0.0); m.faces.len()];
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        let f = *f as usize;
        if m.faces.get(f).map(|f| f.kind != SurfKind::Plane).unwrap_or(true) {
            continue;
        }
        let [a, b, c] = [d3(&m.positions[t[0] as usize]), d3(&m.positions[t[1] as usize]), d3(&m.positions[t[2] as usize])];
        let area = (b - a).cross(c - a).length() * 0.5;
        acc[f].0 += (a + b + c) / 3.0 * area;
        acc[f].1 += area;
    }
    for (f, (s, area)) in acc.into_iter().enumerate() {
        if area > 1e-9 {
            push(s / area, SnapKind::FaceCenter, vec![f], &mut out);
        }
    }
    out
}

/// A value read from the kernel's f32 mesh, cleaned of its float noise
/// (17.0000001 → 17).
pub fn tidy(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}

fn tidy2(a: [f64; 2]) -> [f64; 2] {
    [tidy(a[0]), tidy(a[1])]
}

/// The foot of `p` on the plane.
pub fn on_plane(plane: &BasePlane, p: DVec3) -> DVec3 {
    p - plane.n * (p - plane.origin).dot(plane.n)
}

/// Screen distance from `p` to the segment `a`–`b`.
fn dist_seg(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-6);
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0);
    (a[0] + ab[0] * t - p[0]).hypot(a[1] + ab[1] * t - p[1])
}

/// Screen distance from `p` to the whole line through `a` and `b`.
fn dist_line(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len = ab[0].hypot(ab[1]);
    if len < 1e-4 {
        return (p[0] - a[0]).hypot(p[1] - a[1]);
    }
    ((p[0] - a[0]) * ab[1] - (p[1] - a[1]) * ab[0]).abs() / len
}

impl AppState {
    /// Whether points of interest catch the pointer now (the setting, and
    /// Ctrl not held).
    pub fn snapping(&self) -> bool {
        self.settings.points && !self.snap_off
    }

    /// Recomputes the points of interest when a new model arrived and no
    /// gesture is live (while a shape is moved or created they stay as they
    /// were: it does not snap to itself).
    pub fn refresh_snaps(&mut self) {
        if self.is_live() || self.snaps_gen == self.model_gen {
            return;
        }
        self.snaps_gen = self.model_gen;
        self.snaps = self.model.as_ref().map(|m| snap_points(m)).unwrap_or_default();
    }

    /// The point of interest under the pointer: the nearest on screen
    /// within `CATCH`, corners and centres first, the eye seeing it, not
    /// on a feature of `skip`.
    pub fn pick_snap(&self, p: [f32; 2], skip: &[FeatureId]) -> Option<&SnapPoint> {
        if !self.snapping() {
            return None;
        }
        let v = self.view();
        let mut cands: Vec<(f32, &SnapPoint)> = self
            .snaps
            .iter()
            .filter(|s| !s.features.iter().any(|f| skip.contains(f)))
            .filter_map(|s| {
                let q = v.project(s.pos.as_vec3())?;
                let d = (q[0] - p[0]).hypot(q[1] - p[1]);
                (d < CATCH).then_some((d + s.kind.rank(), s))
            })
            .collect();
        cands.sort_by(|a, b| a.0.total_cmp(&b.0));
        cands.into_iter().map(|c| c.1).find(|s| s.kind == SnapKind::Origin || !self.hidden_but(s.pos.as_vec3(), skip))
    }

    /// Whether the part hides a point from the eye, the faces of the
    /// features of `skip` aside (a shape being moved does not hide what it
    /// moves over).
    pub fn hidden_but(&self, q: Vec3, skip: &[FeatureId]) -> bool {
        let Some(m) = &self.model else { return false };
        let v = self.view();
        let to = q - v.eye;
        let dist = to.length();
        let tol = 0.3 + dist * 0.004;
        match self.picker.pick(m, v.eye, to / dist.max(1e-6)) {
            Some(h) if h.t < dist - tol => self.feature_of_face(h.face).map(|f| skip.contains(&f)).unwrap_or(false) == false,
            _ => false,
        }
    }

    /// The points a move carries (see `Carried`): the points of interest on
    /// the model faces `faces` or on the features `feats`, and the points
    /// `extra` (a picked vertex, an edge's ends and middle). Called before
    /// the drag goes live (the points of interest made current first).
    pub fn carried(&mut self, feats: &[FeatureId], faces: &[usize], extra: &[DVec3]) -> Carried {
        self.refresh_snaps();
        let mut c = Carried::default();
        for (i, s) in self.snaps.iter().enumerate() {
            if s.kind == SnapKind::Origin {
                continue;
            }
            let mine = s.features.iter().any(|f| feats.contains(f)) || s.faces.iter().any(|f| faces.contains(f)) || extra.iter().any(|e| e.distance(s.pos) < 1e-3);
            if mine {
                c.own.push(i);
                c.pts.push((s.pos, s.kind));
            }
        }
        for e in extra {
            if !c.pts.iter().any(|(q, _)| q.distance(*e) < 1e-3) {
                c.pts.push((*e, SnapKind::End));
            }
        }
        c
    }

    /// The alignment of a move along an axis or in a plane (`mask`: the
    /// directions it may take; `raw`: the move the pointer asks for). A
    /// point the move carries that passes near a point of interest on
    /// screen lands on it (lined up only, with a dotted line, when the axis
    /// or the plane cannot take it there); when the pointer itself is on
    /// one, the nearest of `keys` (the pivot; a shape's centre, base or
    /// top) lines up with it. The nearest on screen wins, a carried point
    /// that would stay far from it in depth last. The move and what to draw.
    pub fn align_move(&self, p: [f32; 2], raw: DVec3, mask: DVec3, keys: &[DVec3], carried: &Carried, skip: &[FeatureId]) -> Option<(DVec3, Snapped)> {
        if !self.snapping() {
            return None;
        }
        let v = self.view();
        let targets: Vec<(usize, [f32; 2])> = self
            .snaps
            .iter()
            .enumerate()
            .filter(|(i, s)| !carried.own.contains(i) && !s.features.iter().any(|f| skip.contains(f)))
            .filter_map(|(i, s)| Some((i, v.project(s.pos.as_vec3())?)))
            .collect();
        // (score, the point moved, the move, the point of interest)
        let mut cands: Vec<(f32, DVec3, DVec3, usize)> = vec![];
        for (i, q) in &targets {
            let d = (q[0] - p[0]).hypot(q[1] - p[1]);
            if d >= CATCH {
                continue;
            }
            let s = &self.snaps[*i];
            if let Some((k, delta)) = keys.iter().map(|k| (*k, (s.pos - *k) * mask)).min_by(|x, y| x.1.distance(raw).total_cmp(&y.1.distance(raw))) {
                cands.push((d + s.kind.rank(), k, delta, *i));
            }
        }
        for (from, kind) in &carried.pts {
            let Some(m) = v.project((*from + raw).as_vec3()) else { continue };
            for (i, q) in &targets {
                let d = (q[0] - m[0]).hypot(q[1] - m[1]);
                if d >= CATCH {
                    continue;
                }
                let s = &self.snaps[*i];
                let delta = (s.pos - *from) * mask;
                // near on screen only, far in depth: the piece would jump
                let Some(k) = v.project((*from + delta).as_vec3()) else { continue };
                if (k[0] - m[0]).hypot(k[1] - m[1]) > CATCH * 1.5 {
                    continue;
                }
                // what the axis or plane leaves between them, in points
                let gap = (s.pos.distance(*from + delta) / self.mm_per_point(s.pos)) as f32;
                cands.push((d + s.kind.rank() + 0.5 * kind.rank() + (gap * 0.25).min(12.0), *from, delta, *i));
            }
        }
        cands.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (_, from, delta, i) = cands.into_iter().take(12).find(|c| {
            let s = &self.snaps[c.3];
            s.kind == SnapKind::Origin || !self.hidden_but(s.pos.as_vec3(), skip)
        })?;
        let s = &self.snaps[i];
        let pos = from + delta;
        Some((delta, Snapped { pos, kind: s.kind, source: (pos.distance(s.pos) > 1e-3).then_some(s.pos), guides: vec![] }))
    }

    /// Remembers the point of interest under the pointer (the guides line
    /// up with the last few the pointer passed over).
    pub fn acquire(&mut self, p: [f32; 2], skip: &[FeatureId]) {
        let Some(s) = self.pick_snap(p, skip).cloned() else { return };
        self.acquired.retain(|a| a.pos.distance(s.pos) > 1e-6);
        self.acquired.push(s);
        if self.acquired.len() > 4 {
            self.acquired.remove(0);
        }
    }

    /// The point of a visible edge under the pointer (within a few points).
    pub fn pick_on_edge(&self, p: [f32; 2], skip: &[FeatureId]) -> Option<DVec3> {
        if !self.snapping() {
            return None;
        }
        let m = self.model.as_ref()?;
        let v = self.view();
        let mut best: Option<(f32, DVec3, usize)> = None;
        for (i, e) in m.edges.iter().enumerate().filter(|(_, e)| !e.seam) {
            if e.faces.iter().filter(|f| **f >= 0).any(|f| self.feature_of_face(*f as usize).map(|id| skip.contains(&id)).unwrap_or(false)) {
                continue;
            }
            for w in e.pts.windows(2) {
                let (a, b) = (Vec3::from(w[0]), Vec3::from(w[1]));
                let (Some(sa), Some(sb)) = (v.project(a), v.project(b)) else { continue };
                let d = dist_seg(p, sa, sb);
                if d < ALIGN && best.map(|b| d < b.0).unwrap_or(true) {
                    let ab = [sb[0] - sa[0], sb[1] - sa[1]];
                    let t = (((p[0] - sa[0]) * ab[0] + (p[1] - sa[1]) * ab[1]) / (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-6)).clamp(0.0, 1.0);
                    best = Some((d, a.lerp(b, t).as_dvec3(), i));
                }
            }
        }
        let (_, mut q, i) = best?;
        // a circle: on the circle itself, not on its sampled chords
        let e = &m.edges[i];
        if e.kind == EdgeKind::Circle && e.pts.len() >= 4 {
            let n = e.pts.len() - 1;
            if let Some(c) = circumcentre(d3(&e.pts[0]), d3(&e.pts[n / 3]), d3(&e.pts[2 * n / 3])) {
                let r = d3(&e.pts[0]).distance(c);
                let off = q - c;
                if off.length() > 1e-9 {
                    q = c + off.normalize() * r;
                }
            }
        }
        (!self.hidden_but(q.as_vec3(), skip)).then_some(q)
    }

    /// The pointer on a plane, snapped: to a point of interest (its foot on
    /// the plane), else to a visible edge, else lined up with one or two of
    /// the points the pointer passed over lately (`acquired`), else to the
    /// grid step. Plane coordinates and what caught it.
    pub fn snap_on_plane(&self, plane: &BasePlane, p: [f32; 2], skip: &[FeatureId]) -> Option<([f64; 2], Option<Snapped>)> {
        let (ro, rd) = self.ray(p);
        let raw = plane.hit(ro, rd)?;
        let grid = |a: [f64; 2]| [self.snap(a[0]), self.snap(a[1])];
        let q = plane.to_plane(raw);
        if let Some(s) = self.pick_snap(p, skip) {
            let foot = on_plane(plane, s.pos);
            let source = (foot.distance(s.pos) > 1e-3).then_some(s.pos);
            return Some((tidy2(plane.to_plane(foot)), Some(Snapped { pos: foot, kind: s.kind, source, guides: vec![] })));
        }
        if !self.snapping() {
            return Some((grid(q), None));
        }
        if let Some(e) = self.pick_on_edge(p, skip) {
            let foot = on_plane(plane, e);
            let source = (foot.distance(e) > 1e-3).then_some(e);
            return Some((tidy2(plane.to_plane(foot)), Some(Snapped { pos: foot, kind: SnapKind::OnEdge, source, guides: vec![] })));
        }
        // guides: the lines along u and v through each point's foot
        let v = self.view();
        let screen = |w: DVec3| v.project(w.as_vec3());
        let mut best: [Option<(f32, f64, DVec3)>; 2] = [None, None];
        for s in self.acquired.iter().filter(|s| !s.features.iter().any(|f| skip.contains(f))) {
            let a = plane.to_plane(on_plane(plane, s.pos));
            for k in 0..2 {
                // the guide through the point, along the other axis
                let mut at = q;
                at[k] = a[k];
                let mut far = at;
                far[1 - k] += 1.0;
                let (Some(s0), Some(s1)) = (screen(plane.at(at)), screen(plane.at(far))) else { continue };
                let d = dist_line(p, s0, s1);
                // the nearest point wins between points on the same guide
                let near = screen(s.pos).map(|a| (a[0] - p[0]).hypot(a[1] - p[1])).unwrap_or(1e4);
                let d = d + near * 0.002;
                if d < ALIGN && best[k].map(|b| d < b.0).unwrap_or(true) {
                    if s.kind != SnapKind::Origin && self.hidden_but(s.pos.as_vec3(), skip) {
                        continue;
                    }
                    best[k] = Some((d, a[k], s.pos));
                }
            }
        }
        if best.iter().any(|b| b.is_some()) {
            let mut r = grid(q);
            for k in 0..2 {
                if let Some((_, c, _)) = best[k] {
                    r[k] = tidy(c);
                }
            }
            let pos = plane.at(r);
            // from the point down to its foot on the plane, then along the plane
            let mut guides = vec![];
            for b in best.iter().flatten() {
                let foot = on_plane(plane, b.2);
                if foot.distance(b.2) > 1e-3 {
                    guides.push((b.2, foot));
                }
                guides.push((foot, pos));
            }
            return Some((r, Some(Snapped { pos, kind: SnapKind::Aligned, source: None, guides })));
        }
        Some((grid(q), None))
    }

    /// The plane a shape or a sketch starts on: the flat face under the
    /// pointer (else the ground), or when a point of interest catches, a
    /// flat face it lies on that faces the eye (a corner of a top face
    /// starts on the top face, not on the side the ray hit). Plane
    /// coordinates of the start, and what caught it.
    pub fn start_plane(&self, p: [f32; 2]) -> Option<(BasePlane, [f64; 2], Option<Snapped>)> {
        self.start_plane_in(p, false)
    }

    /// The same while the pointer only hovers (the scan's flat faces are
    /// searched less often).
    pub fn start_plane_hover(&self, p: [f32; 2]) -> Option<(BasePlane, [f64; 2], Option<Snapped>)> {
        self.start_plane_in(p, true)
    }

    fn start_plane_in(&self, p: [f32; 2], hover: bool) -> Option<(BasePlane, [f64; 2], Option<Snapped>)> {
        let (mut plane, _) = self.base_plane_at(p, hover)?;
        if let (Some(s), Some(m)) = (self.pick_snap(p, &[]), self.model.as_ref()) {
            let on = |pl: &BasePlane| (s.pos - pl.origin).dot(pl.n).abs() < 1e-3;
            if !on(&plane) {
                let (_, rd) = self.ray(p);
                let best = s
                    .faces
                    .iter()
                    .filter_map(|f| m.faces.get(*f).map(|face| (*f, face)))
                    .filter(|(_, f)| f.kind == SurfKind::Plane)
                    .map(|(i, f)| (i, DVec3::from_array(f.normal), DVec3::from_array(f.point)))
                    .map(|(i, n, pt)| (i, n, pt, -n.dot(rd)))
                    .filter(|c| c.3 > 0.15)
                    .max_by(|a, b| a.3.total_cmp(&b.3));
                if let Some((i, n, pt, _)) = best {
                    plane = BasePlane::new(pt, n, Some(i));
                } else if s.pos.y.abs() < 1e-3 {
                    plane = BasePlane::ground();
                }
            }
        }
        let (a, snapped) = self.snap_on_plane(&plane, p, &[])?;
        Some((plane, a, snapped))
    }

    /// A height along `axis` from `base`: the one of the point of interest
    /// under the pointer, if one catches.
    pub fn snap_height(&self, base: DVec3, axis: DVec3, p: [f32; 2], skip: &[FeatureId]) -> Option<(f64, Snapped)> {
        let s = self.pick_snap(p, skip)?;
        let t = tidy((s.pos - base).dot(axis));
        let pos = base + axis * t;
        Some((t, Snapped { pos, kind: s.kind, source: (pos.distance(s.pos) > 1e-3).then_some(s.pos), guides: vec![] }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nurbs_kernel::model::{Edge, Face, FaceTag};

    fn face(kind: SurfKind, feature: u32) -> Face {
        Face { tag: Some(FaceTag { feature, sub: 0 }), kind, point: [0.0; 3], normal: [0.0, 1.0, 0.0], axis_origin: [0.0; 3], axis_dir: [0.0, 1.0, 0.0], radius: 0.0 }
    }

    #[test]
    fn circumcentre_of_a_circle() {
        let c = DVec3::new(3.0, 2.0, -1.0);
        let at = |a: f64| c + DVec3::new(a.cos(), 0.0, a.sin()) * 5.0;
        let o = circumcentre(at(0.1), at(1.7), at(4.0)).unwrap();
        assert!(o.distance(c) < 1e-9, "{o}");
        assert!(circumcentre(DVec3::ZERO, DVec3::X, DVec3::X * 2.0).is_none());
    }

    #[test]
    fn points_of_a_square_face_and_a_circle() {
        // a 10 mm square face (2 triangles) at y = 5, its 4 sides, and a
        // full circle of radius 2 round (3, 5, 3)
        let mut m = Model::default();
        m.faces = vec![face(SurfKind::Plane, 1), face(SurfKind::Cylinder, 2)];
        m.positions = vec![[0.0, 5.0, 0.0], [10.0, 5.0, 0.0], [10.0, 5.0, 10.0], [0.0, 5.0, 10.0]];
        m.triangles = vec![[0, 1, 2], [0, 2, 3]];
        m.tri_face = vec![0, 0];
        let c = [[0.0f32, 5.0, 0.0], [10.0, 5.0, 0.0], [10.0, 5.0, 10.0], [0.0, 5.0, 10.0]];
        for i in 0..4 {
            m.edges.push(Edge { index: i as i32 + 1, faces: [0, -1], kind: EdgeKind::Line, seam: false, pts: vec![c[i], c[(i + 1) % 4]] });
        }
        let circle: Vec<[f32; 3]> = (0..=32).map(|k| {
            let a = k as f32 / 32.0 * std::f32::consts::TAU;
            [3.0 + 2.0 * a.cos(), 5.0, 3.0 + 2.0 * a.sin()]
        }).collect();
        m.edges.push(Edge { index: 9, faces: [0, 1], kind: EdgeKind::Circle, seam: false, pts: circle });
        let s = snap_points(&m);
        let count = |k: SnapKind| s.iter().filter(|p| p.kind == k).count();
        assert_eq!(count(SnapKind::Origin), 1);
        assert_eq!(count(SnapKind::End), 4, "corners shared by two sides count once");
        assert_eq!(count(SnapKind::Mid), 4, "a closed circle has no middle");
        assert_eq!(count(SnapKind::Center), 1);
        assert_eq!(count(SnapKind::FaceCenter), 1);
        let has = |k: SnapKind, p: DVec3| s.iter().any(|q| q.kind == k && q.pos.distance(p) < 1e-4);
        assert!(has(SnapKind::Mid, DVec3::new(5.0, 5.0, 0.0)));
        assert!(has(SnapKind::Center, DVec3::new(3.0, 5.0, 3.0)));
        assert!(has(SnapKind::FaceCenter, DVec3::new(5.0, 5.0, 5.0)));
        let centre = s.iter().find(|q| q.kind == SnapKind::Center).unwrap();
        assert_eq!(centre.features, vec![1, 2]);
        let corner = s.iter().find(|q| q.kind == SnapKind::End && q.pos.distance(DVec3::new(10.0, 5.0, 0.0)) < 1e-4).unwrap();
        assert_eq!(corner.faces, vec![0]);
    }

    #[test]
    fn foot_on_a_plane() {
        let g = BasePlane::ground();
        assert_eq!(on_plane(&g, DVec3::new(3.0, 7.0, -2.0)), DVec3::new(3.0, 0.0, -2.0));
    }
}
