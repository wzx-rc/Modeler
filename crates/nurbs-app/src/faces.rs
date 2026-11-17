//! Faces and vertices of the part (the Face and Vertex component modes) and
//! the Inset and Extrude tools: a flat face's outline moved in, the face
//! moved out or in along its normal. Each is a step in the history
//! (`doc::Push`), made live while the pointer moves.

use crate::state::*;
use glam::{DVec3, Vec3};
use nurbs_kernel::doc::Push;
use nurbs_kernel::model::SurfKind;
use nurbs_kernel::{FaceRef, FeatureId};

/// The inset a click alone gives (mm).
const DEFAULT_INSET: f64 = 2.0;

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
    /// it starts on it at once: the pointer (at `p`) sets the value, a
    /// click ends it. Else it waits for a press on a flat face.
    pub fn arm_push(&mut self, kind: PushKind, p: Option<[f32; 2]>) {
        self.abort_gestures();
        let picked = if self.sel_mode == SelMode::Face && self.tool == Tool::Select { self.face_sel.first().copied() } else { None };
        self.tool = Tool::Push(kind);
        if let Some(r) = picked {
            if let Some(face) = self.model_face_of(&r) {
                let at = p.unwrap_or_else(|| self.view().project(r.at.as_vec3()).unwrap_or(self.principal()));
                self.push_start(kind, face, r.at, at, false);
            }
        }
    }

    /// A press (or a click) with the Inset / Extrude tool on the part.
    pub fn push_press(&mut self, kind: PushKind, p: [f32; 2], held: bool) {
        let Some(h) = self.pick_at(p) else {
            self.toast(&format!("{}: press on a flat face of the part", kind.label()), true);
            return;
        };
        self.push_start(kind, h.face, h.point.as_dvec3(), p, held);
    }

    fn push_start(&mut self, kind: PushKind, face: usize, at: DVec3, p: [f32; 2], held: bool) {
        let Some(f) = self.model.as_ref().and_then(|m| m.faces.get(face)).cloned() else { return };
        if f.kind != SurfKind::Plane {
            self.toast(&format!("{}: only flat faces for now", kind.label()), true);
            return;
        }
        let Some(r) = self.face_ref(face, at) else { return };
        let n = DVec3::from_array(f.normal).normalize();
        // the grab point on the face's plane
        let o = DVec3::from_array(f.point);
        let origin = at - n * (at - o).dot(n);
        self.begin_live(kind.label());
        let mut id = 0;
        self.live(|d| {
            // named after the tool ("Inset 2"), though it starts with no inset
            let name = d.fresh_name(kind.label());
            id = d.add_push(Push { face: FaceRef { tag: r.tag, at: origin }, inset: 0.0, depth: 0.0 });
            if let Some(f) = d.feature_mut(id) {
                f.name = name;
            }
        });
        let (ro, rd) = self.ray(p);
        let t0 = closest_on_line(origin, n, ro, rd).unwrap_or(0.0);
        let phase = if kind == PushKind::Inset { PushPhase::Inset } else { PushPhase::Depth };
        self.push = Some(PushGesture { kind, id, origin, n, phase, held, from: p, t0, moved: false, inset: 0.0, depth: 0.0 });
        self.tool = Tool::Push(kind);
        self.face_sel.clear();
        self.selection = vec![id];
    }

    /// The pointer moves: the inset (distance on the face's plane from the
    /// grab point) or the depth (along the normal) follows.
    pub fn push_move(&mut self, p: [f32; 2]) {
        let Some(mut g) = self.push.clone() else { return };
        if (p[0] - g.from[0]).hypot(p[1] - g.from[1]) > 3.0 {
            g.moved = true;
        }
        if !g.moved {
            return;
        }
        let (ro, rd) = self.ray(p);
        match g.phase {
            PushPhase::Inset => {
                let den = rd.dot(g.n);
                if den.abs() > 1e-9 {
                    let t = (g.origin - ro).dot(g.n) / den;
                    if t > 0.0 {
                        let q = ro + rd * t;
                        g.inset = self.snap(q.distance(g.origin)).max(0.0);
                    }
                }
            }
            PushPhase::Depth => {
                if let Some(t) = closest_on_line(g.origin, g.n, ro, rd) {
                    g.depth = self.snap(t - g.t0);
                }
            }
        }
        self.push = Some(g.clone());
        self.apply_push(&g);
    }

    fn apply_push(&mut self, g: &PushGesture) {
        let (id, inset, depth) = (g.id, g.inset, g.depth);
        self.live(|d| {
            if let Some(pu) = d.feature_mut(id).and_then(|f| f.push.as_mut()) {
                pu.inset = inset;
                pu.depth = depth;
            }
        });
    }

    /// The end of a phase: the release of the press, or a click. The inset
    /// goes on to the depth (a click alone gives a 2 mm inset); the depth
    /// ends the step.
    pub fn push_next(&mut self, p: [f32; 2]) {
        let Some(mut g) = self.push.clone() else { return };
        match g.phase {
            PushPhase::Inset => {
                if !g.moved || g.inset <= 0.0 {
                    g.inset = DEFAULT_INSET;
                }
                g.phase = PushPhase::Depth;
                g.held = false;
                g.from = p;
                g.moved = false;
                let (ro, rd) = self.ray(p);
                g.t0 = closest_on_line(g.origin, g.n, ro, rd).unwrap_or(0.0);
                self.push = Some(g.clone());
                self.apply_push(&g);
            }
            PushPhase::Depth if g.held && !g.moved => {
                // pressed without moving: the pointer moves it now, a click ends it
                g.held = false;
                self.push = Some(g);
            }
            PushPhase::Depth => self.finish_push(),
        }
    }

    pub fn finish_push(&mut self) {
        let Some(g) = self.push.take() else { return };
        if g.inset <= 0.0 && g.depth.abs() < 1e-9 {
            // nothing moved: nothing to make
            self.cancel_live();
            self.clean_selection_after_push(None);
            return;
        }
        self.end_live();
        self.clean_selection_after_push(Some(g.id));
        let what = match (g.inset > 0.0, g.depth) {
            (true, d) if d.abs() < 1e-9 => format!("Inset {} mm", crate::widgets::fmt_mm(g.inset as f32)),
            (true, d) => format!("Inset {} mm, {} {} mm", crate::widgets::fmt_mm(g.inset as f32), if d > 0.0 { "raised" } else { "sunk" }, crate::widgets::fmt_mm(d.abs() as f32)),
            (false, d) => format!("Extrude {} mm", crate::widgets::fmt_mm(d as f32)),
        };
        self.toast(&what, false);
    }

    /// After an inset or extrusion: back to picking faces, the new step
    /// selected (its sizes in the panel).
    fn clean_selection_after_push(&mut self, id: Option<FeatureId>) {
        self.tool = Tool::Select;
        self.sel_mode = SelMode::Face;
        self.face_sel.clear();
        self.edge_sel.clear();
        self.vert_sel.clear();
        self.selection = id.into_iter().filter(|id| self.doc.feature(*id).is_some()).collect();
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
