//! Faces and vertices of the part (the Face and Vertex component modes) and
//! the Inset and Extrude tools: a flat face's outline moved in, the face
//! moved out or in along its normal. Each is a step in the history
//! (`doc::Push`), made live while the pointer moves.

use crate::state::*;
use glam::{DVec3, Vec3};
use nurbs_kernel::doc::Push;
use nurbs_kernel::model::{EdgeKind, SurfKind};
use nurbs_kernel::{Comp, EdgeRef, FaceRef, Feature, FeatureId, Prim, Tweak};

/// A new inset (mm).
const DEFAULT_INSET: f64 = 2.0;

/// Where the selected inset or extrusion's handles are (world).
#[derive(Clone, Copy, Debug)]
pub struct PushHandles {
    /// The inset's: its line's start on the face's border, the inward
    /// direction, the dot (on the inset outline).
    pub inset: Option<(DVec3, DVec3, DVec3)>,
    /// The arrow, from `base` to `tip`, along the outward normal `n`.
    pub base: DVec3,
    pub tip: DVec3,
    pub n: DVec3,
}

impl AppState {
    // --------------------------------------------------------------- faces

    /// The reference of a model face: its tag and a point on it.
    pub fn face_ref(&self, face: usize, at: DVec3) -> Option<FaceRef> {
        let tag = self.model.as_ref()?.faces.get(face)?.tag?.pack();
        Some(FaceRef { tag, at })
    }

    /// The model face a reference names: among the faces with its tag, the
    /// one whose triangles come nearest its point.
    pub fn model_face_of(&self, r: &FaceRef) -> Option<usize> {
        let m = self.model.as_ref()?;
        let at = r.at.as_vec3();
        let same: Vec<usize> = (0..m.faces.len()).filter(|i| m.faces[*i].tag.map(|t| t.pack()) == Some(r.tag)).collect();
        if same.len() <= 1 {
            return same.first().copied();
        }
        let mut best = vec![f32::MAX; m.faces.len()];
        for (t, tri) in m.triangles.iter().enumerate() {
            let f = m.tri_face[t] as usize;
            if !same.contains(&f) {
                continue;
            }
            let d = point_triangle(at, tri.map(|v| Vec3::from(m.positions[v as usize])));
            best[f] = best[f].min(d);
        }
        same.into_iter().min_by(|a, b| best[*a].total_cmp(&best[*b]))
    }

    /// The faces picked in Face mode, as model faces.
    pub fn picked_faces(&self) -> Vec<usize> {
        self.face_sel.iter().filter_map(|r| self.model_face_of(r)).collect()
    }

    /// A click in Face mode on a face (or on nothing): picks it, or adds
    /// it / takes it away (Shift / Ctrl).
    pub fn click_face(&mut self, at: Option<[f32; 2]>, add: bool) {
        let hit = at.and_then(|p| self.pick_at(p));
        let Some(r) = hit.and_then(|h| self.face_ref(h.face, h.point.as_dvec3())) else {
            if !add {
                self.face_sel.clear();
                self.selection.clear();
            }
            return;
        };
        self.selection.clear();
        let face = hit.map(|h| h.face);
        if add {
            if let Some(i) = self.face_sel.iter().position(|x| self.model_face_of(x) == face) {
                self.face_sel.remove(i);
            } else {
                self.face_sel.push(r);
            }
        } else {
            self.face_sel = vec![r];
        }
    }

    // ------------------------------------------------------------ vertices

    /// The part's B-rep vertices: the ends of its edges (seams left out).
    pub fn vertices(&self) -> Vec<Vec3> {
        let Some(m) = &self.model else { return vec![] };
        let mut out: Vec<Vec3> = vec![];
        for e in m.edges.iter().filter(|e| !e.seam) {
            let (Some(a), Some(b)) = (e.pts.first(), e.pts.last()) else { continue };
            for p in [Vec3::from(*a), Vec3::from(*b)] {
                if !out.iter().any(|q| q.distance(p) < 1e-3) {
                    out.push(p);
                }
            }
        }
        out
    }

    /// Whether the part hides a point, seen from the eye.
    pub fn hidden(&self, q: Vec3) -> bool {
        let Some(m) = &self.model else { return false };
        let v = self.view();
        let to = q - v.eye;
        let dist = to.length();
        let hit = self.picker.pick(m, v.eye, to / dist.max(1e-6));
        let tol = 0.3 + dist * 0.004;
        hit.map(|h| h.t < dist - tol).unwrap_or(false)
    }

    /// The vertices the eye sees, with their place on screen.
    pub fn visible_vertices(&self) -> Vec<(Vec3, [f32; 2])> {
        let v = self.view();
        self.vertices().into_iter().filter(|q| !self.hidden(*q)).filter_map(|q| v.project(q).map(|s| (q, s))).collect()
    }

    /// The vertex under the pointer (within a few points, not hidden).
    pub fn pick_vertex(&self, p: [f32; 2]) -> Option<DVec3> {
        self.visible_vertices()
            .into_iter()
            .map(|(q, s)| (q, (s[0] - p[0]).hypot(s[1] - p[1])))
            .filter(|(_, d)| *d < 9.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(q, _)| q.as_dvec3())
    }

    pub fn click_vertex(&mut self, v: Option<DVec3>, add: bool) {
        let Some(v) = v else {
            if !add {
                self.vert_sel.clear();
            }
            return;
        };
        self.selection.clear();
        if add {
            if let Some(i) = self.vert_sel.iter().position(|q| q.distance(v) < 1e-3) {
                self.vert_sel.remove(i);
            } else {
                self.vert_sel.push(v);
            }
        } else {
            self.vert_sel = vec![v];
        }
    }

    // ------------------------------------------------------ inset, extrude

    /// The selected inset or extrusion, if one is.
    pub fn selected_push(&self) -> Option<&nurbs_kernel::Feature> {
        self.single().filter(|f| f.push.is_some())
    }

    /// The Inset or Extrude tool (I / E). With a face picked in Face mode,
    /// it works on it at once: Inset makes a 2 mm inset, to size with its
    /// handle; Extrude follows the pointer (at `p`), a click ends it. Else
    /// the tool waits for a click on a flat face.
    pub fn arm_push(&mut self, kind: PushKind, p: Option<[f32; 2]>) {
        self.abort_gestures();
        let picked = if self.sel_mode == SelMode::Face && self.tool == Tool::Select { self.face_sel.first().copied() } else { None };
        self.tool = Tool::Push(kind);
        let Some(r) = picked else { return };
        let Some(face) = self.model_face_of(&r) else { return };
        match kind {
            PushKind::Inset => {
                self.make_inset(face, r.at);
            }
            PushKind::Extrude => {
                let at = p.unwrap_or_else(|| self.view().project(r.at.as_vec3()).unwrap_or(self.principal()));
                self.push_start(face, r.at, at, false);
            }
        }
    }

    /// The face's plane through a point: the point on it, the outward normal.
    fn face_plane(&mut self, face: usize, at: DVec3, what: &str) -> Option<(FaceRef, DVec3, DVec3)> {
        let f = self.model.as_ref().and_then(|m| m.faces.get(face)).cloned()?;
        if f.kind != SurfKind::Plane {
            self.toast(&format!("{what}: only flat faces for now"), true);
            return None;
        }
        let r = self.face_ref(face, at)?;
        let n = DVec3::from_array(f.normal).normalize();
        let o = DVec3::from_array(f.point);
        Some((r, at - n * (at - o).dot(n), n))
    }

    /// An inset of a face, 2 mm, the line only (no depth): selected, its
    /// handle and arrow on screen to size it.
    pub fn make_inset(&mut self, face: usize, at: DVec3) -> Option<FeatureId> {
        let (r, origin, _) = self.face_plane(face, at, "Inset")?;
        let mut id = 0;
        self.edit("Inset", |d| {
            let name = d.fresh_name("Inset");
            id = d.add_push(Push { face: FaceRef { tag: r.tag, at: origin }, inset: DEFAULT_INSET, depth: 0.0 });
            if let Some(f) = d.feature_mut(id) {
                f.name = name;
            }
        });
        self.tool = Tool::Select;
        self.sel_mode = SelMode::Face;
        self.face_sel.clear();
        self.edge_sel.clear();
        self.selection = vec![id];
        self.toast("Inset 2 mm · drag the dot to move the outline, the arrow to raise or sink it", false);
        Some(id)
    }

    /// A click with the Inset tool, or a press with the Extrude tool.
    pub fn push_press(&mut self, kind: PushKind, p: [f32; 2], held: bool) {
        let Some(h) = self.pick_at(p) else {
            self.toast(&format!("{}: click a flat face of the part", kind.label()), true);
            return;
        };
        match kind {
            PushKind::Inset => {
                self.make_inset(h.face, h.point.as_dvec3());
            }
            PushKind::Extrude => self.push_start(h.face, h.point.as_dvec3(), p, held),
        }
    }

    fn push_start(&mut self, face: usize, at: DVec3, p: [f32; 2], held: bool) {
        let Some((r, origin, n)) = self.face_plane(face, at, "Extrude") else { return };
        self.begin_live("Extrude");
        let mut id = 0;
        self.live(|d| id = d.add_push(Push { face: FaceRef { tag: r.tag, at: origin }, inset: 0.0, depth: 0.0 }));
        let (ro, rd) = self.ray(p);
        let t0 = closest_on_line(origin, n, ro, rd).unwrap_or(0.0);
        self.push = Some(PushGesture { kind: PushKind::Extrude, id, origin, n, held, from: p, t0, moved: false, inset: 0.0, depth: 0.0 });
        self.tool = Tool::Push(PushKind::Extrude);
        self.face_sel.clear();
        self.selection = vec![id];
    }

    /// The pointer moves: the extrusion's depth follows, along the normal.
    pub fn push_move(&mut self, p: [f32; 2]) {
        let Some(mut g) = self.push.clone() else { return };
        if (p[0] - g.from[0]).hypot(p[1] - g.from[1]) > 3.0 {
            g.moved = true;
        }
        if !g.moved {
            return;
        }
        let (ro, rd) = self.ray(p);
        if let Some(t) = closest_on_line(g.origin, g.n, ro, rd) {
            g.depth = self.snap(t - g.t0);
        }
        self.push = Some(g.clone());
        let (id, depth) = (g.id, g.depth);
        self.live(|d| {
            if let Some(pu) = d.feature_mut(id).and_then(|f| f.push.as_mut()) {
                pu.depth = depth;
            }
        });
    }

    /// The release of the press, or a click: the end of the extrusion (a
    /// press without a move goes on with the pointer, a click ends it).
    pub fn push_next(&mut self, _p: [f32; 2]) {
        let Some(mut g) = self.push.clone() else { return };
        if g.held && !g.moved {
            g.held = false;
            self.push = Some(g);
            return;
        }
        self.finish_push();
    }

    pub fn finish_push(&mut self) {
        let Some(g) = self.push.take() else { return };
        if g.depth.abs() < 1e-9 {
            // nothing moved: nothing to make
            self.cancel_live();
            self.clean_selection_after_push(None);
            return;
        }
        self.end_live();
        self.clean_selection_after_push(Some(g.id));
        self.toast(&format!("Extrude {} mm · drag the arrow to change it", crate::widgets::fmt_mm(g.depth as f32)), false);
    }

    /// After an extrusion: back to picking faces, the new step selected
    /// (its arrow on screen, its sizes in the panel).
    fn clean_selection_after_push(&mut self, id: Option<FeatureId>) {
        self.tool = Tool::Select;
        self.sel_mode = SelMode::Face;
        self.face_sel.clear();
        self.edge_sel.clear();
        self.vert_sel.clear();
        self.selection = id.into_iter().filter(|id| self.doc.feature(*id).is_some()).collect();
    }

    // ------------------------------------------------------------- handles

    /// The selected inset or extrusion's handles (with the Select tool): the
    /// inset's dot, on a line from the face's border (nearest the step's
    /// point) inwards; the arrow, from the moved face along its normal.
    pub fn push_handles(&self) -> Option<PushHandles> {
        if self.tool != Tool::Select || self.push.is_some() || self.sel_mode == SelMode::Vertex {
            return None;
        }
        let f = self.selected_push()?;
        let p = f.push.clone()?;
        let m = self.model.as_ref()?;
        let mine = |i: i32| i >= 0 && m.faces.get(i as usize).and_then(|x| x.tag).map(|t| t.feature == f.id).unwrap_or(false);
        let theirs = |i: i32| i >= 0 && m.faces.get(i as usize).and_then(|x| x.tag).map(|t| t.pack() == p.face.tag).unwrap_or(false);
        // the normal: the step's own face (the inset, or the end), else the face it starts from
        let own = m.faces.iter().find(|x| x.tag.map(|t| t.feature == f.id && t.sub == 1).unwrap_or(false));
        let from = m.faces.iter().find(|x| x.tag.map(|t| t.pack() == p.face.tag).unwrap_or(false));
        let n = DVec3::from_array(own.or(from)?.normal).normalize();
        let at = p.face.at;
        // the inset: the border of the face it starts from (its edges not
        // shared with the step's faces), the point nearest the step's point
        let mut inset = None;
        if p.inset > 0.0 {
            let mut best: Option<(f64, DVec3, DVec3)> = None;
            for e in m.edges.iter().filter(|e| !e.seam) {
                let [a, b] = e.faces;
                let border = (theirs(a) && !mine(b) && b >= 0) || (theirs(b) && !mine(a) && a >= 0);
                if !border {
                    continue;
                }
                for w in e.pts.windows(2) {
                    let (u, v) = (Vec3::from(w[0]).as_dvec3(), Vec3::from(w[1]).as_dvec3());
                    let d = v - u;
                    let s = ((at - u).dot(d) / d.length_squared().max(1e-12)).clamp(0.0, 1.0);
                    let q = u + d * s;
                    let dist = q.distance(at);
                    if best.map(|b| dist < b.0).unwrap_or(true) {
                        best = Some((dist, q, d.normalize_or_zero()));
                    }
                }
            }
            if let Some((_, q, t)) = best {
                // inwards: across the border, on the face's plane, towards the point
                let side = n.cross(t).normalize_or_zero();
                let w = if side.dot(at - q) >= 0.0 { side } else { -side };
                if w.length_squared() > 0.5 {
                    inset = Some((q, w, q + w * p.inset));
                }
            }
        }
        // the arrow: from the middle of the step's own face, else from its point moved
        let base = own.filter(|x| x.kind == SurfKind::Plane).map(|x| DVec3::from_array(x.point)).unwrap_or(at + n * p.depth);
        let v = self.view();
        let mut len = v.pixel_size_at((base.as_vec3() - v.eye).length()) as f64 * 84.0;
        // seen end on, the arrow shrinks: long enough to show 64 points at least
        if let (Some(a), Some(b)) = (v.project(base.as_vec3()), v.project((base + n * len).as_vec3())) {
            let px = ((b[0] - a[0]).hypot(b[1] - a[1]) as f64).max(1.0);
            if px < 64.0 {
                len *= (64.0 / px).min(4.0);
            }
        }
        Some(PushHandles { inset, base, tip: base + n * len, n })
    }

    pub(crate) fn begin_push_drag(&mut self, h: Handle, p: [f32; 2]) {
        let Some(hs) = self.push_handles() else { return };
        let Some(f) = self.selected_push().cloned() else { return };
        let line = match h {
            Handle::PushInset => match hs.inset {
                Some((q, w, _)) => (q, w),
                None => return,
            },
            _ => (hs.base, hs.n),
        };
        let (ro, rd) = self.ray(p);
        let Some(t) = closest_on_line(line.0, line.1, ro, rd) else { return };
        self.begin_live(f.kind_label());
        self.drag = Some(Drag { handle: h, grab: line.0 + line.1 * t, start: vec![f], line: Some(line) });
    }

    pub(crate) fn update_push_drag(&mut self, d: &Drag, p: [f32; 2]) {
        let (Some((o, dir)), Some(f)) = (d.line, d.start.first()) else { return };
        let Some(p0) = f.push.clone() else { return };
        let (ro, rd) = self.ray(p);
        let Some(t) = closest_on_line(o, dir, ro, rd) else { return };
        let amount = t - (d.grab - o).dot(dir);
        let (inset, depth) = match d.handle {
            Handle::PushInset => (self.snap(p0.inset + amount).max(0.1), p0.depth),
            _ => (p0.inset, self.snap(p0.depth + amount)),
        };
        let id = f.id;
        self.live(|doc| {
            if let Some(pu) = doc.feature_mut(id).and_then(|g| g.push.as_mut()) {
                pu.inset = inset;
                pu.depth = depth;
            }
        });
    }

    /// Sets the selected inset or extrusion's sizes from the panel.
    pub fn set_push(&mut self, inset: f64, depth: f64, start: bool) {
        let Some(id) = self.selected_push().map(|f| f.id) else { return };
        if start {
            if self.is_live() {
                self.end_live();
            }
            self.begin_live("Inset");
        } else if !self.is_live() {
            self.begin_live("Inset");
        }
        self.live(|d| {
            // "Inset 2" becomes "Extrude 2" when its inset goes to 0 (a name
            // the user gave stays)
            let rename = d.feature(id).and_then(|f| {
                let old = f.push.as_ref()?.label();
                let new = if inset > 0.0 { "Inset" } else { "Extrude" };
                (old != new && f.name.starts_with(old)).then(|| d.fresh_name(new))
            });
            if let Some(f) = d.feature_mut(id) {
                if let Some(name) = rename {
                    f.name = name;
                }
                if let Some(pu) = f.push.as_mut() {
                    pu.inset = inset.max(0.0);
                    pu.depth = depth;
                }
            }
        });
    }
}

/// Distance from a point to a triangle.
fn point_triangle(p: Vec3, t: [Vec3; 3]) -> f32 {
    let [a, b, c] = t;
    let n = (b - a).cross(c - a);
    let nl = n.length();
    if nl > 1e-9 {
        let n = n / nl;
        let q = p - n * (p - a).dot(n);
        // inside: the distance to the plane
        let inside = [(a, b), (b, c), (c, a)].iter().all(|(u, v)| (*v - *u).cross(q - *u).dot(n) >= -1e-6);
        if inside {
            return (p - q).length();
        }
    }
    [(a, b), (b, c), (c, a)]
        .iter()
        .map(|(u, v)| {
            let d = *v - *u;
            let s = ((p - *u).dot(d) / d.length_squared().max(1e-12)).clamp(0.0, 1.0);
            (p - (*u + d * s)).length()
        })
        .fold(f32::MAX, f32::min)
}

// ------------------------------------------------- component moves

impl AppState {
    /// The sketch corner at a vertex (top or bottom of a sketched shape, at
    /// one of its outline's corners, or at either end of its cut): the
    /// sketch and the point's index.
    pub fn profile_corner(&self, v: DVec3) -> Option<(FeatureId, usize)> {
        for f in self.doc.features.iter().filter(|f| f.visible) {
            let (Prim::Profile { x, z, .. }, Some(pr)) = (&f.prim, &f.profile) else { continue };
            let l = f.matrix().inverse().transform_point3(v);
            let h = f.prim.height();
            if !(l.y.abs() < 1e-3 || (l.y.abs() - h).abs() < 1e-3) {
                continue;
            }
            let q = [l.x, l.z];
            let near = |p: [f64; 2]| (p[0] - q[0]).hypot(p[1] - q[1]) < 0.05;
            let pts = pr.scaled(*x, *z);
            let cuts = pr.cuts(*x, *z);
            for (i, p) in pts.iter().enumerate() {
                if pr.smooth.get(i).copied().unwrap_or(false) {
                    continue;
                }
                if near(*p) || cuts[i].map(|c| near(c.a) || near(c.b)).unwrap_or(false) {
                    return Some((f.id, i));
                }
            }
        }
        None
    }

    /// The picked vertices' sketch corners.
    pub fn picked_profile_corners(&self) -> Vec<(FeatureId, usize)> {
        let mut out: Vec<(FeatureId, usize)> = vec![];
        for v in &self.vert_sel {
            if let Some(c) = self.profile_corner(*v) {
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
        out
    }

    /// Cuts the picked sketch corners: a fillet of radius `size` (or a
    /// chamfer of `size` along each side), 0 makes them sharp again. A
    /// slider drag is one undo step (`start` begins it).
    pub fn set_profile_corners(&mut self, size: f64, chamfer: bool, start: bool) {
        let corners = self.picked_profile_corners();
        if corners.is_empty() {
            return;
        }
        if start || !self.is_live() {
            self.end_live();
            self.begin_live(if chamfer { "Chamfer corner" } else { "Fillet corner" });
        }
        let v = if chamfer { -size.max(0.0) } else { size.max(0.0) };
        self.live(|d| {
            for (id, i) in &corners {
                if let Some(pr) = d.feature_mut(*id).and_then(|f| f.profile.as_mut()) {
                    if pr.corner.len() < pr.points.len() {
                        pr.corner.resize(pr.points.len(), 0.0);
                    }
                    pr.corner[*i] = v;
                }
            }
        });
    }

    /// The first picked sketch corner's cut as asked (size, chamfer) and
    /// the largest its sides allow.
    pub fn profile_corner_state(&self) -> Option<(f64, bool, f64)> {
        let (id, i) = *self.picked_profile_corners().first()?;
        let f = self.doc.feature(id)?;
        let (Prim::Profile { x, z, .. }, Some(pr)) = (&f.prim, &f.profile) else { return None };
        let c = pr.corner.get(i).copied().unwrap_or(0.0);
        // the largest: a cut asked very big stops there
        let mut big = pr.clone();
        big.corner.resize(big.points.len(), 0.0);
        big.corner[i] = if c < 0.0 { -1e9 } else { 1e9 };
        let max = nurbs_kernel::doc::corner_cut(&big.scaled(*x, *z), &big.smooth, &big.corner, i).map(|k| k.max).unwrap_or(0.0);
        Some((c.abs(), c < 0.0, max))
    }

    /// The edge a vertex fillet rounds (Romain 18 nov.: "Fillet sur les
    /// vertex quand on a une surface fine"): at a corner of a thin plate,
    /// the short straight edge across its thickness, much shorter than the
    /// other edges at that corner. None elsewhere.
    pub fn corner_edge(&self, v: DVec3) -> Option<EdgeRef> {
        let m = self.model.as_ref()?;
        let at = v.as_vec3();
        let mut here: Vec<(usize, f32, bool)> = m
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.seam && e.pts.len() >= 2)
            .filter(|(_, e)| e.pts.first().map(|p| Vec3::from_array(*p).distance(at) < 1e-3).unwrap_or(false) || e.pts.last().map(|p| Vec3::from_array(*p).distance(at) < 1e-3).unwrap_or(false))
            .map(|(i, e)| (i, e.pts.windows(2).map(|w| Vec3::from_array(w[0]).distance(Vec3::from_array(w[1]))).sum::<f32>(), e.kind == EdgeKind::Line))
            .collect();
        here.sort_by(|a, b| a.1.total_cmp(&b.1));
        match here.as_slice() {
            [(i, short, true), (_, next, _), ..] if *short < 0.6 * *next => self.edge_ref(*i),
            _ => None,
        }
    }

    /// Rounds the corners at the picked vertices (their short edges) at
    /// `size`: one fillet step, selected after.
    pub fn round_corners(&mut self, size: f64) {
        let mut edges: Vec<EdgeRef> = vec![];
        for v in self.vert_sel.clone() {
            if let Some(r) = self.corner_edge(v) {
                if !edges.iter().any(|e| e.key() == r.key() && e.at.distance(r.at) < 1e-3) {
                    edges.push(r);
                }
            }
        }
        if edges.is_empty() || size <= 1e-6 {
            return;
        }
        self.edge_sel = edges;
        self.set_round(size, false, true);
        self.edge_sel.clear();
        // the corners are gone: the fillet stays selected
        self.vert_sel.clear();
        self.hover_vertex = None;
    }

    /// The selected move step (Move face / edge / vertex), if one.
    pub fn selected_tweak(&self) -> Option<&Feature> {
        self.single().filter(|f| f.tweak.is_some())
    }

    /// The component the gizmo moves in Vertex, Edge and Face modes: the
    /// selected move step's, else the first one picked.
    pub fn picked_comp(&self) -> Option<Comp> {
        match self.sel_mode {
            SelMode::Face => self.face_sel.first().cloned().map(Comp::Face),
            SelMode::Edge => self.edge_sel.first().cloned().map(Comp::Edge),
            SelMode::Vertex => self.vert_sel.first().copied().map(Comp::Vertex),
            SelMode::Object => None,
        }
    }

    /// Where the gizmo sits in Vertex, Edge and Face modes.
    pub fn comp_pivot(&self) -> Option<DVec3> {
        if self.tool != Tool::Select || self.push.is_some() {
            return None;
        }
        if let Some(t) = self.selected_tweak().and_then(|f| f.tweak.as_ref()) {
            return Some(t.at() + t.delta);
        }
        match self.picked_comp()? {
            Comp::Face(f) => Some(f.at),
            Comp::Edge(e) => Some(e.at),
            // a vertex that is gone (a sketch corner just cut): no gizmo
            Comp::Vertex(v) => self.vertices().iter().any(|q| q.as_dvec3().distance(v) < 1e-3).then_some(v),
        }
    }

    /// A drag on the gizmo in Vertex, Edge or Face mode: the selected move
    /// step changes, or a new one starts on the picked component.
    pub(crate) fn begin_tweak_drag(&mut self, h: Handle, p: [f32; 2]) {
        if !matches!(h, Handle::Axis(_) | Handle::Plane(_)) {
            return;
        }
        let Some(pivot) = self.comp_pivot() else { return };
        let Some(grab) = self.handle_param(h, pivot, p) else { return };
        let id = match self.selected_tweak() {
            Some(f) => {
                let id = f.id;
                self.begin_live("Move");
                id
            }
            None => {
                let Some(comp) = self.picked_comp() else { return };
                let t = Tweak { comp, delta: DVec3::ZERO };
                self.begin_live(t.label());
                let mut id = 0;
                self.live(|d| id = d.add_tweak(t));
                self.select(Some(id), false);
                id
            }
        };
        let Some(f) = self.doc.feature(id).cloned() else { return };
        self.drag = Some(Drag { handle: h, grab, start: vec![f], line: None });
    }

    pub(crate) fn update_tweak_drag(&mut self, d: &Drag, p: [f32; 2]) {
        let Some(t0) = d.start[0].tweak.clone() else { return };
        let id = d.start[0].id;
        let pivot0 = t0.at() + t0.delta;
        let Some(now) = self.handle_param(d.handle, pivot0, p) else { return };
        let raw = now - d.grab;
        let mut delta = DVec3::new(self.snap(raw.x), self.snap(raw.y), self.snap(raw.z));
        self.snap_shown = None;
        let a = match d.handle {
            Handle::Axis(a) | Handle::Plane(a) => a,
            _ => return,
        };
        // a point of interest under the pointer: the component lines up
        // with it, along the axis or in the plane
        if let Some(s) = self.pick_snap(p, &[]).cloned() {
            let mask = if matches!(d.handle, Handle::Axis(_)) { DVec3::AXES[a] } else { DVec3::ONE - DVec3::AXES[a] };
            delta = (s.pos - pivot0) * mask;
            let pos = pivot0 + delta;
            self.snap_shown = Some(crate::snap::Snapped { pos, kind: s.kind, source: (pos.distance(s.pos) > 1e-3).then_some(s.pos), guides: vec![] });
        }
        self.live(|doc| {
            if let Some(tw) = doc.feature_mut(id).and_then(|f| f.tweak.as_mut()) {
                tw.delta = t0.delta + delta;
            }
        });
    }
}
