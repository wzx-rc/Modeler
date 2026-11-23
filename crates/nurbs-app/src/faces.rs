//! Faces and vertices of the part (the Face and Vertex component modes) and
//! the Inset and Extrude tools: a flat face's outline moved in, the face
//! moved out or in along its normal. Each is a step in the history
//! (`doc::Push`), made live while the pointer moves.

use crate::state::*;
use glam::{DVec3, Vec3};
use nurbs_kernel::doc::Push;
use nurbs_kernel::model::{EdgeKind, SurfKind};
use nurbs_kernel::{Comp, EdgeRef, FaceRef, Feature, FeatureId, Knife, Prim, Tweak};

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
        // (only the triangles of the faces with that tag: after a Mirror
        // every face has a twin, and going through the whole mesh for each
        // picked face froze a rectangle's release for 13 s on a heavy scene)
        self.picker.face_near(self.model.as_ref()?, r.tag, r.at.as_vec3())
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

    // ------------------------------------------- convert (Maya, Ctrl + mode)

    /// What is picked converted to another mode's components and that
    /// mode set (Maya's Convert Selection; Romain 23 nov., 16 h 11: "Si je
    /// click CTRL + un component icon ça fait une conversion"): faces give
    /// their edges, their vertices or their elements; edges the faces on
    /// either side, their ends, their elements; vertices the edges and the
    /// faces round them; shapes (Object mode) all their faces, edges or
    /// vertices. One undo step (a selection step).
    pub fn convert_selection(&mut self, to: SelMode) {
        let from = self.sel_mode;
        let Some(m) = self.model.clone() else {
            self.set_sel_mode(to);
            return;
        };
        let near = |a: Vec3, b: Vec3| a.distance(b) < 1e-3;
        let ends = |i: usize| -> Vec<Vec3> {
            let e = &m.edges[i];
            [e.pts.first(), e.pts.last()].into_iter().flatten().map(|p| Vec3::from(*p)).collect()
        };
        let live = |i: usize| !m.edges[i].seam && m.edges[i].pts.len() >= 2;
        // what is picked, as model faces, edges and vertices
        let (mut faces, mut edges, mut verts): (Vec<usize>, Vec<usize>, Vec<Vec3>) = (vec![], vec![], vec![]);
        match from {
            SelMode::Face => faces = self.picked_faces(),
            SelMode::Edge => edges = self.edge_sel.iter().filter_map(|e| self.model_edge_of(e)).collect(),
            SelMode::Vertex => verts = self.vert_sel.iter().map(|v| v.as_vec3()).collect(),
            SelMode::Object => {
                let els: Vec<FeatureId> = self.selection.iter().map(|id| self.element_of(*id)).collect();
                faces = (0..m.faces.len()).filter(|&f| m.faces[f].tag.map(|t| els.contains(&self.element_of(t.feature))).unwrap_or(false)).collect();
            }
        }
        if from == to && from != SelMode::Object {
            return;
        }
        // filled in from what was picked: faces → edges → vertices, or back
        if !faces.is_empty() {
            edges = (0..m.edges.len()).filter(|&i| live(i) && m.edges[i].faces.iter().any(|f| *f >= 0 && faces.contains(&(*f as usize)))).collect();
        } else if !verts.is_empty() {
            edges = (0..m.edges.len()).filter(|&i| live(i) && ends(i).iter().any(|q| verts.iter().any(|v| near(*v, *q)))).collect();
        }
        if faces.is_empty() {
            for &i in &edges {
                for f in m.edges[i].faces {
                    if f >= 0 && !faces.contains(&(f as usize)) {
                        faces.push(f as usize);
                    }
                }
            }
        }
        if verts.is_empty() {
            for &i in &edges {
                for q in ends(i) {
                    if !verts.iter().any(|v| near(*v, q)) {
                        verts.push(q);
                    }
                }
            }
        }
        let (nf, ne, nv) = (faces.len(), edges.len(), verts.len());
        // a point inside each face: its first triangle's middle
        let face_point = |f: usize| -> Option<DVec3> {
            let t = self.picker.range(f).find(|&t| m.tri_face[t] as usize == f)?;
            Some((m.triangles[t].iter().map(|k| Vec3::from(m.positions[*k as usize])).sum::<Vec3>() / 3.0).as_dvec3())
        };
        let face_refs: Vec<FaceRef> = faces.iter().filter_map(|&f| self.face_ref(f, face_point(f)?)).collect();
        let edge_refs: Vec<EdgeRef> = edges.iter().filter_map(|&i| self.edge_ref(i)).collect();
        let mut elements: Vec<FeatureId> = vec![];
        for &f in &faces {
            if let Some(t) = m.faces[f].tag {
                let el = self.element_of(t.feature);
                if self.doc.feature(el).is_some() && !elements.contains(&el) {
                    elements.push(el);
                }
            }
        }
        self.set_sel_mode(to);
        self.selection.clear();
        let n = match to {
            SelMode::Face => {
                self.face_sel = face_refs;
                format!("{nf} face{}", if nf == 1 { "" } else { "s" })
            }
            SelMode::Edge => {
                self.edge_sel = edge_refs;
                format!("{ne} edge{}", if ne == 1 { "" } else { "s" })
            }
            SelMode::Vertex => {
                self.vert_sel = verts.iter().map(|v| v.as_dvec3()).collect();
                format!("{nv} verti{}", if nv == 1 { "x" } else { "ces" })
            }
            SelMode::Object => {
                let k = elements.len();
                self.selection = elements;
                format!("{k} shape{}", if k == 1 { "" } else { "s" })
            }
        };
        self.toast(&format!("Converted to {n}"), false);
    }

    // ----------------------------------------------------- linked (Maya)

    /// The faces linked to `face` (a double-click in Face mode, as Maya's
    /// shell): those reached across the part's edges without leaving its
    /// element. A piece of faces copied apart from the part is all of it.
    pub fn linked_faces(&self, face: usize) -> Vec<usize> {
        let Some(m) = &self.model else { return vec![] };
        let el = |f: usize| m.faces.get(f).and_then(|x| x.tag).map(|t| self.element_of(t.feature));
        let want = el(face);
        if want.is_none() {
            return vec![face];
        }
        let mut nb: Vec<Vec<usize>> = vec![vec![]; m.faces.len()];
        for e in &m.edges {
            let (a, b) = (e.faces[0], e.faces[1]);
            if a >= 0 && b >= 0 && a != b {
                nb[a as usize].push(b as usize);
                nb[b as usize].push(a as usize);
            }
        }
        let mut seen = vec![false; m.faces.len()];
        let mut out = vec![face];
        seen[face] = true;
        let mut i = 0;
        while i < out.len() {
            for &g in &nb[out[i]] {
                if !seen[g] && el(g) == want {
                    seen[g] = true;
                    out.push(g);
                }
            }
            i += 1;
        }
        out
    }

    /// A point on a model face (its largest triangle's centre), to name it.
    pub fn face_point(&self, face: usize) -> Option<DVec3> {
        let m = self.model.as_ref()?;
        let mut best: Option<(f32, Vec3)> = None;
        for (t, tri) in m.triangles.iter().enumerate() {
            if m.tri_face[t] as usize != face {
                continue;
            }
            let [a, b, c] = tri.map(|v| Vec3::from(m.positions[v as usize]));
            let area = (b - a).cross(c - a).length();
            if best.map(|x| area > x.0).unwrap_or(true) {
                best = Some((area, (a + b + c) / 3.0));
            }
        }
        best.map(|b| b.1.as_dvec3())
    }

    /// A double-click in Face mode: the linked faces picked (Shift / Ctrl:
    /// added to those picked).
    pub fn pick_linked_faces(&mut self, at: [f32; 2], add: bool) {
        let Some(h) = self.pick_at(at) else { return };
        let refs: Vec<FaceRef> = self.linked_faces(h.face).into_iter().filter_map(|f| self.face_ref(f, self.face_point(f)?)).collect();
        if refs.is_empty() {
            return;
        }
        self.selection.clear();
        if !add {
            self.face_sel.clear();
        }
        for r in refs {
            if !self.face_sel.iter().any(|x| x.tag == r.tag && x.at.distance(r.at) < 1e-3) {
                self.face_sel.push(r);
            }
        }
        let n = self.face_sel.len();
        self.record("pick", &format!("double-click: {n} linked faces"));
    }

    /// The edges linked to edge `i` (a double-click in Edge mode, as
    /// Maya's edge loop): on from each end through the edge that goes on
    /// tangent to it (within 20°), or the only other edge there.
    pub fn linked_edges(&self, i: usize) -> Vec<usize> {
        let Some(m) = &self.model else { return vec![] };
        let ok = |j: usize| {
            let e = &m.edges[j];
            !e.seam && e.faces[0] >= 0 && e.faces[1] >= 0 && e.pts.len() >= 2
        };
        if i >= m.edges.len() || !ok(i) {
            return vec![i];
        }
        // an end of edge j: its point and the way out of the edge there
        let end = |j: usize, last: bool| -> (Vec3, Vec3) {
            let p = &m.edges[j].pts;
            let (a, b) = if last { (p[p.len() - 1], p[p.len() - 2]) } else { (p[0], p[1]) };
            let (a, b) = (Vec3::from(a), Vec3::from(b));
            (a, (a - b).normalize_or_zero())
        };
        const TOL: f32 = 2e-3;
        let cos = 20f32.to_radians().cos();
        let mut out = vec![i];
        for last in [false, true] {
            let (mut p, mut d) = end(i, last);
            let mut cur = i;
            loop {
                // the other edges ending at p: how many, and those that may
                // go on (not taken yet), with the way into them
                let mut others = 0;
                let mut here: Vec<(usize, bool, Vec3)> = vec![];
                for (j, e) in m.edges.iter().enumerate() {
                    if j == cur || e.seam || e.pts.len() < 2 {
                        continue;
                    }
                    for l in [false, true] {
                        let (q, w) = end(j, l);
                        if q.distance(p) < TOL {
                            others += 1;
                            if ok(j) && !out.contains(&j) {
                                here.push((j, l, -w));
                            }
                        }
                    }
                }
                let next = here
                    .iter()
                    .map(|&(j, l, w)| (j, l, w.dot(d)))
                    .filter(|&(_, _, c)| c > cos || others == 1)
                    .max_by(|a, b| a.2.total_cmp(&b.2));
                let Some((j, l, _)) = next else { break };
                out.push(j);
                cur = j;
                (p, d) = end(j, !l);
            }
        }
        out
    }

    /// A double-click in Edge mode: the linked edges picked (Shift / Ctrl:
    /// added to those picked).
    pub fn pick_linked_edges(&mut self, edge: usize, add: bool) {
        let refs: Vec<EdgeRef> = self.linked_edges(edge).into_iter().filter_map(|j| self.edge_ref(j)).collect();
        if refs.is_empty() {
            return;
        }
        self.selection.clear();
        if !add {
            self.edge_sel.clear();
        }
        for r in refs {
            if !self.edge_sel.iter().any(|x| x.key() == r.key() && x.at.distance(r.at) < 1e-3) {
                self.edge_sel.push(r);
            }
        }
        let n = self.edge_sel.len();
        self.record("pick", &format!("double-click: {n} linked edges"));
    }

    /// A double-click in Vertex mode: the vertices of the faces linked to
    /// one face round the vertex (Shift / Ctrl: added).
    pub fn pick_linked_vertices(&mut self, v: DVec3, add: bool) {
        let Some(m) = &self.model else { return };
        let v32 = v.as_vec3();
        let near = |p: &[f32; 3]| Vec3::from(*p).distance(v32) < 2e-3;
        let Some(face) = m.edges.iter().filter(|e| !e.seam && e.pts.len() >= 2 && (near(&e.pts[0]) || near(e.pts.last().unwrap()))).flat_map(|e| e.faces).find(|f| *f >= 0) else { return };
        let faces = self.linked_faces(face as usize);
        let Some(m) = &self.model else { return };
        let mut pts: Vec<DVec3> = vec![];
        for e in m.edges.iter().filter(|e| !e.seam && e.pts.len() >= 2 && e.faces.iter().any(|f| *f >= 0 && faces.contains(&(*f as usize)))) {
            for p in [e.pts[0], *e.pts.last().unwrap()] {
                let p = Vec3::from(p).as_dvec3();
                if !pts.iter().any(|q| q.distance(p) < 1e-3) {
                    pts.push(p);
                }
            }
        }
        self.selection.clear();
        if !add {
            self.vert_sel.clear();
        }
        for p in pts {
            if !self.vert_sel.iter().any(|q| q.distance(p) < 1e-3) {
                self.vert_sel.push(p);
            }
        }
        let n = self.vert_sel.len();
        self.record("pick", &format!("double-click: {n} linked vertices"));
    }

    // ------------------------------------------------------------ vertices

    /// The vertices Vertex mode picks: the part's, and the paths' points
    /// (a cut corner by the middle of its cut).
    pub fn vertices(&self) -> Vec<Vec3> {
        let mut out = self.part_vertices();
        // a curve's points, all of them (20 nov.: moved with the gizmo; a
        // corner cut at its cut's middle)
        for f in self.doc.features.iter().filter(|f| f.visible && f.is_path() && f.shape_bool.is_none()) {
            let Some(pr) = &f.profile else { continue };
            for i in 0..pr.points.len() {
                if let Some(q) = crate::surfaces::path_point_pos(f, i) {
                    out.push(q.as_vec3());
                }
            }
        }
        // a 2D boolean's own corners (where its curves cross)
        out.extend(self.bool_corners().into_iter().map(|c| c.1.as_vec3()));
        out
    }

    /// The part's B-rep vertices: the ends of its edges (seams left out).
    pub fn part_vertices(&self) -> Vec<Vec3> {
        let Some(m) = &self.model else { return vec![] };
        self.picker.vertices(m).to_vec()
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

    /// Whether the part hides a point lying on its faces `own` (an edge's
    /// two faces, the faces round a vertex), seen from the eye. Stricter
    /// than `hidden`: a face that is not one of its own hides it as soon as
    /// it is hit clearly before the point (a thin wall's far side, 2 mm
    /// behind, was taken as seen); its own faces keep the looser margin,
    /// the mesh lying a little off the edge's points on a curved face.
    pub fn hidden_on(&self, q: Vec3, own: &[i32]) -> bool {
        let Some(m) = &self.model else { return false };
        let v = self.view();
        let to = q - v.eye;
        let dist = to.length();
        let Some(h) = self.picker.pick(m, v.eye, to / dist.max(1e-6)) else { return false };
        let tol = if own.contains(&(h.face as i32)) { 0.3 + dist * 0.004 } else { 0.02 + dist * 2e-4 };
        h.t < dist - tol
    }

    /// The vertices the eye sees, with their place on screen.
    pub fn visible_vertices(&self) -> Vec<(Vec3, [f32; 2])> {
        let v = self.view();
        let all = self.vertices();
        if self.xray {
            return all.into_iter().filter_map(|q| v.project(q).map(|s| (q, s))).collect();
        }
        // which ones the part hides: kept while the model, the eye and the
        // vertices stay the same (asked every frame in Vertex mode)
        let mut key = (self.model_gen, [v.eye.x.to_bits(), v.eye.y.to_bits(), v.eye.z.to_bits()], all.len() as u64);
        for q in &all {
            key.2 = key.2.wrapping_mul(0x100000001b3) ^ (q.x.to_bits() as u64) ^ ((q.y.to_bits() as u64) << 20) ^ ((q.z.to_bits() as u64) << 40);
        }
        let mut memo = self.vert_hidden.borrow_mut();
        if memo.0 != key || memo.1.len() != all.len() {
            *memo = (key, all.iter().map(|q| self.hidden(*q)).collect());
        }
        all.into_iter().zip(memo.1.iter()).filter(|(_, h)| !**h).filter_map(|(q, _)| v.project(q).map(|s| (q, s))).collect()
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

    /// A rectangle drawn in Vertex, Edge or Face mode (Maya): the
    /// components the eye sees inside it. Shift adds them, Ctrl takes them
    /// away, else they replace the picks. A vertex counts by its point, an
    /// edge by its middle, a face by any of its triangles seen there.
    /// **Only what the camera sees** (Romain 21 nov., 0 h 11: "un rectangle
    /// de sélection sélectionne les faces arrières"): nothing behind the
    /// part is taken, in X-ray too (a click there takes the front one as
    /// well), and a thin wall's far side is hidden (`hidden_on`).
    /// The rectangle in Object mode: every element it touches, whole
    /// (Romain 23 nov., 7 h 08: "Le rectangle de sélection doit prendre
    /// complètement un élément dès lors qu'il est touché", Maya's and 3ds
    /// Max's crossing): the part's faces seen anywhere in it (rays through
    /// a grid of it, every 4 points at most) give their elements, a curve
    /// whose line crosses it its own; a closed group comes whole. Shift
    /// adds, Ctrl takes away, else they replace the selection.
    pub fn marquee_elements(&mut self, a: [f32; 2], b: [f32; 2], shift: bool, ctrl: bool) {
        let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
        let mut ids: Vec<FeatureId> = vec![];
        if self.model.is_some() {
            let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
            let (nx, ny) = (((w / 4.0).ceil() as usize).clamp(1, 160), ((h / 4.0).ceil() as usize).clamp(1, 160));
            let mut faces: Vec<usize> = vec![];
            for j in 0..=ny {
                for i in 0..=nx {
                    let s = [lo[0] + w * i as f32 / nx as f32, lo[1] + h * j as f32 / ny as f32];
                    if let Some(hit) = self.pick_at(s) {
                        if !faces.contains(&hit.face) {
                            faces.push(hit.face);
                        }
                    }
                }
            }
            for f in faces {
                if let Some(id) = self.feature_of_face(f) {
                    ids.push(self.element_of(id));
                }
            }
        }
        // the curves whose line runs through the rectangle
        let v = self.view();
        let inside = |s: [f32; 2]| s[0] >= lo[0] && s[0] <= hi[0] && s[1] >= lo[1] && s[1] <= hi[1];
        let crosses = |p: [f32; 2], q: [f32; 2]| {
            if inside(p) || inside(q) {
                return true;
            }
            // the segment against the rectangle's four sides
            let seg = |a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]| {
                let cr = |o: [f32; 2], u: [f32; 2], w: [f32; 2]| (u[0] - o[0]) * (w[1] - o[1]) - (u[1] - o[1]) * (w[0] - o[0]);
                (cr(a, b, c) * cr(a, b, d) < 0.0) && (cr(c, d, a) * cr(c, d, b) < 0.0)
            };
            let k = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]];
            (0..4).any(|i| seg(p, q, k[i], k[(i + 1) % 4]))
        };
        for f in self.doc.features.iter().filter(|f| f.visible && f.is_path()) {
            if self.curve_inside(f.id).is_some_and(|b| !b.visible || !self.boolean_open(b)) {
                continue;
            }
            let hit = self.curve_lines(f).iter().any(|line| {
                let s: Vec<Option<[f32; 2]>> = line.iter().map(|q| v.project(q.as_vec3())).collect();
                s.windows(2).any(|w| matches!((w[0], w[1]), (Some(p), Some(q)) if crosses(p, q)))
            });
            if hit {
                ids.push(f.id);
            }
        }
        // a shape of a closed group brings the whole group
        let mut all: Vec<FeatureId> = vec![];
        for id in ids {
            for x in self.pick_ids(id) {
                if !all.contains(&x) {
                    all.push(x);
                }
            }
        }
        if !(shift || ctrl) {
            self.selection.clear();
        }
        self.group_sel = None;
        for id in all {
            let at = self.selection.iter().position(|x| *x == id);
            match (ctrl, at) {
                (true, Some(i)) => {
                    self.selection.remove(i);
                }
                (false, None) => self.selection.push(id),
                _ => {}
            }
        }
    }

    pub fn marquee_components(&mut self, a: [f32; 2], b: [f32; 2], shift: bool, ctrl: bool) {
        let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
        let inside = |s: [f32; 2]| s[0] >= lo[0] && s[0] <= hi[0] && s[1] >= lo[1] && s[1] <= hi[1];
        let v = self.view();
        // backface culling off: what is behind is taken too
        let through = self.settings.select_through;
        self.selection.clear();
        match self.sel_mode {
            SelMode::Vertex => {
                // the part's vertices with the faces round each (its edges'),
                // the curves' points as they are
                let mut found: Vec<DVec3> = vec![];
                if let Some(m) = self.model.as_ref() {
                    let mut seen: Vec<(Vec3, Vec<i32>)> = vec![];
                    for e in m.edges.iter().filter(|e| !e.seam) {
                        let (Some(a), Some(b)) = (e.pts.first(), e.pts.last()) else { continue };
                        for p in [Vec3::from(*a), Vec3::from(*b)] {
                            if !v.project(p).map(inside).unwrap_or(false) {
                                continue;
                            }
                            match seen.iter_mut().find(|q| q.0.distance(p) < 1e-3) {
                                Some(q) => q.1.extend(e.faces),
                                None => seen.push((p, e.faces.to_vec())),
                            }
                        }
                    }
                    found.extend(seen.iter().filter(|(p, own)| through || !self.hidden_on(*p, own)).map(|(p, _)| p.as_dvec3()));
                }
                let part = self.part_vertices();
                for q in self.vertices() {
                    if part.iter().any(|p| p.distance(q) < 1e-3) {
                        continue;
                    }
                    if v.project(q).map(inside).unwrap_or(false) && (through || !self.hidden(q)) {
                        found.push(q.as_dvec3());
                    }
                }
                if !(shift || ctrl) {
                    self.vert_sel.clear();
                }
                for q in found {
                    let at = self.vert_sel.iter().position(|x| x.distance(q) < 1e-3);
                    match (ctrl, at) {
                        (true, Some(i)) => {
                            self.vert_sel.remove(i);
                        }
                        (false, None) => self.vert_sel.push(q),
                        _ => {}
                    }
                }
            }
            SelMode::Edge => {
                let Some(m) = self.model.as_ref() else { return };
                let mut found: Vec<EdgeRef> = vec![];
                for i in 0..m.edges.len() {
                    let Some(r) = self.edge_ref(i) else { continue };
                    let mid = r.at.as_vec3();
                    if v.project(mid).map(inside).unwrap_or(false) && (through || !self.hidden_on(mid, &m.edges[i].faces)) {
                        found.push(r);
                    }
                }
                if !(shift || ctrl) {
                    self.edge_sel.clear();
                }
                for r in found {
                    let at = self.edge_sel.iter().position(|x| x.key() == r.key() && x.at.distance(r.at) < 1e-3);
                    match (ctrl, at) {
                        (true, Some(i)) => {
                            self.edge_sel.remove(i);
                        }
                        (false, None) => self.edge_sel.push(r),
                        _ => {}
                    }
                }
            }
            SelMode::Face => {
                let Some(m) = self.model.as_ref() else { return };
                // per face, a few of its triangles inside the rectangle,
                // seen there (the ray through it hits that face first)
                let mut found: Vec<(usize, DVec3)> = vec![];
                let mut inside_tris: Vec<(Vec3, [f32; 2])> = vec![];
                for f in 0..m.faces.len() {
                    // its triangles inside the rectangle
                    inside_tris.clear();
                    for t in self.picker.range(f) {
                        if m.tri_face[t] as usize != f {
                            continue;
                        }
                        let c = m.triangles[t].iter().map(|k| Vec3::from(m.positions[*k as usize])).sum::<Vec3>() / 3.0;
                        if let Some(s) = v.project(c).filter(|s| inside(*s)) {
                            inside_tris.push((c, s));
                        }
                    }
                    // through: any face with a triangle inside, seen or not
                    if through {
                        if let Some((c, _)) = inside_tris.first() {
                            found.push((f, c.as_dvec3()));
                        }
                        continue;
                    }
                    // else up to 16 of them, spread over the face: only
                    // where the eye sees it
                    let n = inside_tris.len();
                    for k in 0..n.min(16) {
                        let (c, s) = inside_tris[k * n / n.min(16)];
                        if self.pick_at(s).map(|h| h.face) == Some(f) {
                            found.push((f, c.as_dvec3()));
                            break;
                        }
                    }
                }
                if !(shift || ctrl) {
                    self.face_sel.clear();
                }
                // the faces picked so far, as model faces (in step with
                // `face_sel`)
                let mut picked: Vec<Option<usize>> = self.face_sel.iter().map(|x| self.model_face_of(x)).collect();
                for (f, c) in found {
                    let at = picked.iter().position(|x| *x == Some(f));
                    match (ctrl, at) {
                        (true, Some(i)) => {
                            self.face_sel.remove(i);
                            picked.remove(i);
                        }
                        (false, None) => {
                            if let Some(r) = self.face_ref(f, c) {
                                self.face_sel.push(r);
                                picked.push(Some(f));
                            }
                        }
                        _ => {}
                    }
                }
            }
            SelMode::Object => {}
        }
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
        self.selected_pushes().first().and_then(|id| self.doc.feature(*id))
    }

    /// The selected insets and extrusions, when the selection is only
    /// those (several made at once on the picked faces: their window, dot
    /// and arrow set them all; the first one carries the handles).
    pub fn selected_pushes(&self) -> Vec<FeatureId> {
        let all = !self.selection.is_empty() && self.selection.iter().all(|id| self.doc.feature(*id).map(|f| f.push.is_some()).unwrap_or(false));
        if all { self.selection.clone() } else { vec![] }
    }

    /// The Inset or Extrude tool (I / E). With a face picked in Face mode,
    /// it works on it at once: Inset makes a 2 mm inset, to size with its
    /// handle; Extrude follows the pointer (at `p`), a click ends it. Else
    /// the tool waits for a click on a flat face.
    pub fn arm_push(&mut self, kind: PushKind, p: Option<[f32; 2]>) {
        // a closed curve selected: the Extrude tool pulls it into a solid
        // (Romain 20 nov., 9 h 25: no Extrude of its own on a curve, "si on
        // veut extrude on va chercher le Extrude tool")
        if kind == PushKind::Extrude {
            if let Some(id) = self.pullable() {
                let at = p.or_else(|| self.doc.feature(id).and_then(|f| self.view().project(f.origin.as_vec3()))).unwrap_or(self.principal());
                self.begin_curve_pull(id, at);
                return;
            }
        }
        self.abort_gestures();
        let picked = if self.sel_mode == SelMode::Face && self.tool == Tool::Select { self.face_sel.first().copied() } else { None };
        self.tool = Tool::Push(kind);
        let Some(r) = picked else { return };
        let Some(face) = self.model_face_of(&r) else { return };
        match kind {
            PushKind::Inset => {
                // every picked face (Romain 23 nov., 15 h 19: "Les tools
                // doivent fonctionner sur multi selection")
                let faces = self.picked_face_points();
                self.make_insets(&faces);
            }
            PushKind::Extrude => {
                let at = p.unwrap_or_else(|| self.view().project(r.at.as_vec3()).unwrap_or(self.principal()));
                self.push_start(face, r.at, at, false);
            }
        }
    }

    /// The face's plane through a point: the point on it, the outward
    /// normal. A curved face (for an extrusion): the point, the normal there.
    fn face_plane(&mut self, face: usize, at: DVec3, what: &str) -> Option<(FaceRef, DVec3, DVec3)> {
        let f = self.model.as_ref().and_then(|m| m.faces.get(face)).cloned()?;
        if f.kind != SurfKind::Plane {
            if what == "Inset" {
                self.toast("Inset: only flat faces (a curved face can be extruded: Ctrl + E)", true);
                return None;
            }
            let r = self.face_ref(face, at)?;
            let n = self.normal_near(face, at)?;
            return Some((r, at, n));
        }
        let r = self.face_ref(face, at)?;
        let n = DVec3::from_array(f.normal).normalize();
        let o = DVec3::from_array(f.point);
        Some((r, at - n * (at - o).dot(n), n))
    }

    /// The outward normal of a model face near a point: a plane's, else
    /// the mesh's there (its triangle nearest the point, its vertices'
    /// normals).
    pub(crate) fn normal_near(&self, face: usize, at: DVec3) -> Option<DVec3> {
        let m = self.model.as_ref()?;
        let f = m.faces.get(face)?;
        if f.kind == SurfKind::Plane {
            return Some(DVec3::from_array(f.normal).normalize());
        }
        let p = |i: u32| Vec3::from(m.positions[i as usize]).as_dvec3();
        let t = m
            .triangles
            .iter()
            .zip(&m.tri_face)
            .filter(|(_, fi)| **fi as usize == face)
            .map(|(t, _)| t)
            .min_by(|a, b| {
                let d = |t: &&[u32; 3]| ((p(t[0]) + p(t[1]) + p(t[2])) / 3.0).distance_squared(at);
                d(a).total_cmp(&d(b))
            })?;
        let n: Vec3 = t.iter().map(|i| Vec3::from(m.normals[*i as usize])).sum();
        Some(n.as_dvec3().try_normalize().unwrap_or(DVec3::from_array(f.normal)))
    }

    /// An inset of a face, 2 mm, the line only (no depth): selected, its
    /// handle and arrow on screen to size it.
    pub fn make_inset(&mut self, face: usize, at: DVec3) -> Option<FeatureId> {
        self.make_insets(&[(face, at)]).into_iter().next()
    }

    /// The picked faces as model faces with their points (Face mode).
    pub fn picked_face_points(&self) -> Vec<(usize, DVec3)> {
        let mut out: Vec<(usize, DVec3)> = vec![];
        for q in &self.face_sel {
            if let Some(fi) = self.model_face_of(q) {
                if !out.iter().any(|x| x.0 == fi) {
                    out.push((fi, q.at));
                }
            }
        }
        out
    }

    /// An inset of 2 mm on each face (the line only), one step per face,
    /// all in one undo step and selected together: their window, dot and
    /// arrow size them all at once (Maya's Inset on several faces). A
    /// curved face is left out (said once).
    pub fn make_insets(&mut self, faces: &[(usize, DVec3)]) -> Vec<FeatureId> {
        let mut refs: Vec<FaceRef> = vec![];
        let mut curved = 0;
        for &(face, at) in faces {
            let flat = self.model.as_ref().and_then(|m| m.faces.get(face)).map(|f| f.kind == SurfKind::Plane).unwrap_or(false);
            if !flat && faces.len() > 1 {
                curved += 1;
                continue;
            }
            if let Some((r, origin, _)) = self.face_plane(face, at, "Inset") {
                refs.push(FaceRef { tag: r.tag, at: origin });
            }
        }
        if refs.is_empty() {
            if curved > 0 {
                self.toast("Inset: only flat faces (a curved face can be extruded: Ctrl + E)", true);
            }
            return vec![];
        }
        let mut ids = vec![];
        self.edit("Inset", |d| {
            for r in &refs {
                let name = d.fresh_name("Inset");
                let id = d.add_push(Push { face: r.clone(), inset: DEFAULT_INSET, depth: 0.0 });
                if let Some(f) = d.feature_mut(id) {
                    f.name = name;
                }
                ids.push(id);
            }
        });
        self.tool = Tool::Select;
        self.sel_mode = SelMode::Face;
        self.face_sel.clear();
        self.edge_sel.clear();
        self.selection = ids.clone();
        let n = ids.len();
        let what = if n > 1 { format!("Inset 2 mm on {n} faces") } else { "Inset 2 mm".to_string() };
        let skipped = if curved > 0 { format!(" ({curved} curved face{} left out)", if curved > 1 { "s" } else { "" }) } else { String::new() };
        self.toast(&format!("{what}{skipped} · drag the dot to move the outline, the arrow to raise or sink it"), false);
        ids
    }

    /// A click with the Inset tool, or a press with the Extrude tool.
    pub fn push_press(&mut self, kind: PushKind, p: [f32; 2], held: bool) {
        // Extrude on a closed curve's line: the curve pulled into a solid
        if kind == PushKind::Extrude {
            if let Some(id) = self.pick_path(p).map(|id| self.top_boolean(id)).filter(|id| self.doc.is_closed_curve(*id)) {
                self.begin_curve_pull(id, p);
                return;
            }
        }
        let Some(h) = self.pick_at(p) else {
            self.toast(&format!("{}: click a {}face of the part", kind.label(), if kind == PushKind::Inset { "flat " } else { "" }), true);
            return;
        };
        match kind {
            PushKind::Inset => {
                // on a picked face: all the picked faces
                let picked = self.picked_face_points();
                if picked.len() > 1 && picked.iter().any(|x| x.0 == h.face) {
                    self.make_insets(&picked);
                } else {
                    self.make_inset(h.face, h.point.as_dvec3());
                }
            }
            PushKind::Extrude => self.push_start(h.face, h.point.as_dvec3(), p, held),
        }
    }

    fn push_start(&mut self, face: usize, at: DVec3, p: [f32; 2], held: bool) {
        let Some((r, origin, n)) = self.face_plane(face, at, "Extrude") else { return };
        // the other picked faces go with it
        let mut more: Vec<FaceRef> = vec![];
        for q in self.face_sel.clone() {
            let Some(fi) = self.model_face_of(&q) else { continue };
            if fi == face {
                continue;
            }
            if let Some((r2, o2, _)) = self.face_plane(fi, q.at, "Extrude") {
                more.push(FaceRef { tag: r2.tag, at: o2 });
            }
        }
        self.begin_live("Extrude");
        let mut id = 0;
        let mut others = vec![];
        self.live(|d| {
            id = d.add_push(Push { face: FaceRef { tag: r.tag, at: origin }, inset: 0.0, depth: 0.0 });
            for m in &more {
                others.push(d.add_push(Push { face: m.clone(), inset: 0.0, depth: 0.0 }));
            }
        });
        let (ro, rd) = self.ray(p);
        let t0 = closest_on_line(origin, n, ro, rd).unwrap_or(0.0);
        self.push = Some(PushGesture { kind: PushKind::Extrude, id, origin, n, held, from: p, t0, moved: false, inset: 0.0, depth: 0.0, others: others.clone() });
        self.tool = Tool::Push(PushKind::Extrude);
        self.face_sel.clear();
        self.selection = std::iter::once(id).chain(others).collect();
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
        let others = g.others.clone();
        self.live(|d| {
            for i in std::iter::once(id).chain(others.iter().copied()) {
                if let Some(pu) = d.feature_mut(i).and_then(|f| f.push.as_mut()) {
                    pu.depth = depth;
                }
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
        self.selection.extend(g.others.iter().copied().filter(|i| self.doc.feature(*i).is_some()));
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
        let own_i = m.faces.iter().position(|x| x.tag.map(|t| t.feature == f.id && t.sub == 1).unwrap_or(false));
        let from_i = m.faces.iter().position(|x| x.tag.map(|t| t.pack() == p.face.tag).unwrap_or(false));
        let own = own_i.map(|i| &m.faces[i]);
        let at = p.face.at;
        // a curved face's: the normal at the step's point (its own face is
        // the face moved along its normals: nearest the point there too)
        let n = self.normal_near(own_i.or(from_i)?, at)?;
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
        // the others selected with it follow (start: the first one, then them)
        let start: Vec<nurbs_kernel::Feature> = std::iter::once(f.clone()).chain(self.selected_pushes().iter().skip(1).filter_map(|id| self.doc.feature(*id).cloned())).collect();
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
        self.drag = Some(Drag { handle: h, grab: line.0 + line.1 * t, start, line: Some(line), ops: vec![], carried: Default::default(), pivot: line.0, custom: false, keep: vec![], cage: None, pts: vec![], groups: vec![] });
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
        // the dragged size given to every step selected with it
        let ids: Vec<FeatureId> = d.start.iter().map(|g| g.id).collect();
        let inset_drag = d.handle == Handle::PushInset;
        self.live(|doc| {
            for id in &ids {
                if let Some(pu) = doc.feature_mut(*id).and_then(|g| g.push.as_mut()) {
                    if inset_drag {
                        pu.inset = inset;
                    } else {
                        pu.depth = depth;
                    }
                }
            }
        });
    }

    /// Sets the selected inset or extrusion's sizes from the panel.
    pub fn set_push(&mut self, inset: f64, depth: f64, start: bool) {
        let ids = self.selected_pushes();
        let Some(first) = ids.first().and_then(|id| self.doc.feature(*id)).and_then(|f| f.push.clone()) else { return };
        // a slider moves one size: that one given to all of them
        let (set_inset, set_depth) = ((inset - first.inset).abs() > 1e-12, (depth - first.depth).abs() > 1e-12);
        if start {
            if self.is_live() {
                self.end_live();
            }
            self.begin_live("Inset");
        } else if !self.is_live() {
            self.begin_live("Inset");
        }
        self.live(|d| {
            for &id in &ids {
                let Some(cur) = d.feature(id).and_then(|f| f.push.clone()) else { continue };
                let inset = if set_inset { inset.max(0.0) } else { cur.inset };
                let depth = if set_depth { depth } else { cur.depth };
                // "Inset 2" becomes "Extrude 2" when its inset goes to 0 (a
                // name the user gave stays)
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
                        pu.inset = inset;
                        pu.depth = depth;
                    }
                }
            }
        });
    }
}

/// Distance from a point to a triangle.
pub(crate) fn point_triangle(p: Vec3, t: [Vec3; 3]) -> f32 {
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
            let (Prim::Profile { x, z, .. } | Prim::Path { x, z }, Some(pr)) = (&f.prim, &f.profile) else { continue };
            // a spline's points are not cut
            if pr.spline {
                continue;
            }
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
                // a path's open ends are plain points
                if pr.smooth.get(i).copied().unwrap_or(false) || (pr.open && (i == 0 || i + 1 == pts.len())) {
                    continue;
                }
                if near(*p) || cuts[i].map(|c| near(c.a) || near(c.b) || near(c.m)).unwrap_or(false) {
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
        // a path's corner is picked by the middle of its cut, which moves
        // with the size: the pick follows it
        let picks: Vec<(usize, (FeatureId, usize))> = self.vert_sel.iter().enumerate().filter_map(|(k, q)| self.profile_corner(*q).map(|c| (k, c))).collect();
        self.live(|d| {
            for (id, i) in &corners {
                if let Some(pr) = d.feature_mut(*id).and_then(editable) {
                    if pr.corner.len() < pr.points.len() {
                        pr.corner.resize(pr.points.len(), 0.0);
                    }
                    pr.corner[*i] = v;
                }
            }
        });
        for (k, (id, i)) in picks {
            let Some(f) = self.doc.feature(id) else { continue };
            let (Prim::Path { x, z }, Some(pr)) = (&f.prim, &f.profile) else { continue };
            let q = pr.cuts(*x, *z)[i].map(|c| c.m).unwrap_or(pr.scaled(*x, *z)[i]);
            self.vert_sel[k] = f.matrix().transform_point3(DVec3::new(q[0], 0.0, q[1]));
        }
    }

    /// The first picked sketch corner's cut as asked (size, chamfer) and
    /// the largest its sides allow.
    pub fn profile_corner_state(&self) -> Option<(f64, bool, f64)> {
        let (id, i) = *self.picked_profile_corners().first()?;
        let f = self.doc.feature(id)?;
        let (Prim::Profile { x, z, .. } | Prim::Path { x, z }, Some(pr)) = (&f.prim, &f.profile) else { return None };
        let c = pr.corner.get(i).copied().unwrap_or(0.0);
        // the largest: a cut asked very big stops there
        let mut big = pr.clone();
        big.corner.resize(big.points.len(), 0.0);
        big.corner[i] = if c < 0.0 { -1e9 } else { 1e9 };
        let max = nurbs_kernel::doc::corner_cut_open(&big.scaled(*x, *z), &big.smooth, &big.corner, i, big.open).map(|k| k.max).unwrap_or(0.0);
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

    /// The edges a vertex fillet rounds: a thin plate's corner, its short
    /// edge; any other corner (a cube's, Romain 19 nov. 23 h 18, "Fillet de
    /// vertex"), every edge leaving it, made in one fillet so they meet in a
    /// rounded corner.
    pub fn vertex_edges(&self, v: DVec3) -> Vec<EdgeRef> {
        if let Some(r) = self.corner_edge(v) {
            return vec![r];
        }
        let Some(m) = self.model.as_ref() else { return vec![] };
        let at = v.as_vec3();
        let near = |p: Option<&[f32; 3]>| p.map(|p| Vec3::from_array(*p).distance(at) < 1e-3).unwrap_or(false);
        (0..m.edges.len())
            .filter(|&i| !m.edges[i].seam && m.edges[i].pts.len() >= 2 && (near(m.edges[i].pts.first()) || near(m.edges[i].pts.last())))
            .filter_map(|i| self.edge_ref(i))
            .collect()
    }

    /// Rounds the corners at the picked vertices (their edges, see
    /// `vertex_edges`) at `size`: one fillet step, selected after.
    pub fn round_corners(&mut self, size: f64) {
        let mut edges: Vec<EdgeRef> = vec![];
        for v in self.vert_sel.clone() {
            for r in self.vertex_edges(v) {
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

    /// The components the gizmo moves in Vertex, Edge and Face modes: all
    /// those picked (19 nov.: two vertices moved with W).
    pub fn picked_comps(&self) -> Vec<Comp> {
        match self.sel_mode {
            SelMode::Face => self.face_sel.iter().cloned().map(Comp::Face).collect(),
            SelMode::Edge => self.edge_sel.iter().cloned().map(Comp::Edge).collect(),
            SelMode::Vertex => self.vert_sel.iter().copied().map(Comp::Vertex).collect(),
            SelMode::Object => vec![],
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
        // curve points: their middle
        if self.picked_path_points().is_some() {
            return Some(self.vert_sel.iter().copied().sum::<DVec3>() / self.vert_sel.len() as f64);
        }
        let comps = self.picked_comps();
        // a vertex that is gone (a sketch corner just cut): no gizmo
        if let [Comp::Vertex(v)] = comps.as_slice() {
            if !self.part_vertices().iter().any(|q| q.as_dvec3().distance(*v) < 1e-3) {
                return None;
            }
        }
        Tweak::new(comps).map(|t| t.at())
    }

    /// The points a component move carries, before it goes live: the
    /// selected move step's (a copy: all its faces'; else its components
    /// where the step put them), else the picked components' (a face's
    /// corners, edges' middles and centre; an edge's ends and middle; a
    /// vertex).
    fn tweak_carried(&mut self) -> crate::snap::Carried {
        let mut feats: Vec<FeatureId> = vec![];
        let mut faces: Vec<usize> = vec![];
        let mut extra: Vec<DVec3> = vec![];
        let (comps, shift) = match self.selected_tweak() {
            Some(f) => {
                let t = f.tweak.clone().unwrap();
                feats.push(f.id);
                (if t.copy { vec![] } else { t.comps().cloned().collect() }, t.delta)
            }
            None => (self.picked_comps(), DVec3::ZERO),
        };
        for c in comps {
            match c {
                Comp::Face(r) => faces.extend(self.model_face_of(&FaceRef { at: r.at + shift, ..r })),
                Comp::Edge(r) => {
                    let Some(i) = self.model_edge_of(&EdgeRef { at: r.at + shift, ..r }) else { continue };
                    let Some(e) = self.model.as_ref().and_then(|m| m.edges.get(i)) else { continue };
                    let d3 = |q: &[f32; 3]| Vec3::from(*q).as_dvec3();
                    extra.extend(e.pts.first().map(d3));
                    extra.extend(e.pts.last().map(d3));
                    extra.extend(polyline_middle(&e.pts));
                }
                Comp::Vertex(v) => extra.push(v + shift),
            }
        }
        self.carried(&feats, &faces, &extra)
    }

    /// A drag on the gizmo in Vertex, Edge or Face mode: the selected move
    /// step changes, or a new one starts on the picked component.
    pub(crate) fn begin_tweak_drag(&mut self, h: Handle, p: [f32; 2]) {
        if !matches!(h, Handle::Axis(_) | Handle::Plane(_) | Handle::Free | Handle::Scale(_) | Handle::ScaleAll | Handle::Cage(_)) {
            return;
        }
        // curve points: moved in their plane, not a step on the part
        if self.selected_tweak().is_none() {
            if let Some(pts) = self.picked_path_points() {
                self.begin_path_points_drag(h, p, pts);
                return;
            }
        }
        let cage = if matches!(h, Handle::Cage(_)) { self.cage() } else { None };
        let Some(pivot) = self.pivot() else { return };
        let Some(grab) = self.handle_param(h, pivot, p) else { return };
        let custom = self.custom_pivot_on();
        let mut carried = if crate::state::move_mask(h).is_some() { self.tweak_carried() } else { Default::default() };
        // a pivot put by hand (D): it alone lands on points
        if custom {
            carried.pts = vec![(pivot, crate::snap::SnapKind::End)];
        }
        let id = match self.selected_tweak() {
            Some(f) => {
                let id = f.id;
                self.begin_live("Move");
                id
            }
            None => {
                let Some(t) = Tweak::new(self.picked_comps()) else { return };
                // R (the cage too): named a scale from the start
                let scaling = matches!(h, Handle::Scale(_) | Handle::ScaleAll | Handle::Cage(_));
                let label = if scaling {
                    match t.comp {
                        Comp::Face(_) => "Scale faces",
                        Comp::Edge(_) => "Scale edges",
                        Comp::Vertex(_) => "Scale vertices",
                    }
                } else {
                    t.label()
                };
                self.begin_live(label);
                let mut id = 0;
                self.live(|d| {
                    id = d.add_tweak(t);
                    if scaling {
                        let name = d.fresh_name(label);
                        if let Some(f) = d.feature_mut(id) {
                            f.name = name;
                        }
                    }
                });
                self.select(Some(id), false);
                id
            }
        };
        let Some(f) = self.doc.feature(id).cloned() else { return };
        self.drag = Some(Drag { handle: h, grab, start: vec![f], line: None, ops: vec![], carried, pivot, custom, keep: vec![], cage, pts: vec![], groups: vec![] });
        // the move step now selected: the pivot put by hand goes on with it
        if custom {
            self.custom_pivot = Some((self.pivot_key(), pivot));
        }
    }

    pub(crate) fn update_tweak_drag(&mut self, d: &Drag, p: [f32; 2]) {
        let Some(t0) = d.start[0].tweak.clone() else { return };
        let id = d.start[0].id;
        let pivot0 = d.pivot;
        // the scale cage: scaled about the side facing the grip (Alt: its
        // middle), as the step's own scale and centre
        if let (Handle::Cage(s), Some(c)) = (d.handle, d.cage) {
            let Some((k, anchor)) = self.cage_scale(&c, s, p, &[]) else { return };
            let (s0, d0) = (t0.scale, t0.delta);
            let a0 = t0.about.unwrap_or_else(|| if (s0 - DVec3::ONE).abs().max_element() < 1e-9 { anchor - d0 } else { c.point([0, 0, 0]) - d0 });
            self.live(|doc| {
                if let Some(tw) = doc.feature_mut(id).and_then(|f| f.tweak.as_mut()) {
                    tw.about = Some(a0);
                    tw.scale = s0 * k;
                    tw.delta = anchor + k * (a0 - anchor + d0) - a0;
                }
            });
            return;
        }
        let Some(now) = self.handle_param(d.handle, pivot0, p) else { return };
        let raw = now - d.grab;
        let mut delta = DVec3::new(self.snap(raw.x), self.snap(raw.y), self.snap(raw.z));
        self.snap_shown = None;
        // R in a component mode: scaled round their centre, along X, Y, Z
        if matches!(d.handle, Handle::Scale(_) | Handle::ScaleAll) {
            let k = match d.handle {
                Handle::Scale(a) => {
                    let (s0, s1) = ((d.grab - pivot0)[a], (now - pivot0)[a]);
                    if s0.abs() < 1e-9 {
                        return;
                    }
                    let mut v = DVec3::ONE;
                    v[a] = s1 / s0;
                    v
                }
                _ => DVec3::splat(2f64.powf((now.x - d.grab.x) / 120.0)),
            };
            let snap = self.settings.snap;
            let k = k.map(|x| if snap { (x * 100.0).round() / 100.0 } else { x }.clamp(0.0, 100.0));
            self.gizmo_readout = Some(match d.handle {
                Handle::Scale(a) => format!("{} × {:.2}", ["X", "Y", "Z"][a], k[a]),
                _ => format!("× {:.2}", k.x),
            });
            self.live(|doc| {
                if let Some(tw) = doc.feature_mut(id).and_then(|f| f.tweak.as_mut()) {
                    tw.scale = t0.scale * k;
                }
            });
            return;
        }
        // a point of what moves passing near a point of interest lands on
        // it; the pointer on one: the pivot lines up with it, along the
        // axis, in the plane, or on it (the centre: all three axes)
        let Some(mask) = crate::state::move_mask(d.handle) else { return };
        if let Some((best, shown)) = self.align_move(p, raw, mask, &[pivot0], &d.carried, &[]) {
            delta = best;
            self.snap_shown = Some(shown);
        }
        if d.custom {
            self.custom_pivot = Some((self.pivot_key(), pivot0 + delta));
        }
        self.live(|doc| {
            if let Some(tw) = doc.feature_mut(id).and_then(|f| f.tweak.as_mut()) {
                tw.delta = t0.delta + delta;
            }
        });
    }
}

// ------------------------------------------ direct drags on picked parts

impl AppState {
    /// The two picked faces joined by a solid (Bridge, Romain 23 nov.,
    /// 7 h 04: "Il faut un Bridge tool"); the step selected.
    pub fn add_bridge(&mut self) -> Option<FeatureId> {
        if self.sel_mode != SelMode::Face || self.face_sel.len() != 2 {
            self.toast("Bridge: pick two faces (Face mode, Shift + click)", true);
            return None;
        }
        self.abort_gestures();
        let faces = [self.face_sel[0], self.face_sel[1]];
        let mut id = None;
        self.edit("Bridge", |d| id = Some(d.add_bridge(faces)));
        let id = id?;
        self.face_sel.clear();
        self.selection = vec![id];
        self.toast("Bridge: the two faces joined", false);
        Some(id)
    }

    /// The picked edges that are open borders (one face: Delete faces left
    /// them).
    pub fn border_edges_picked(&self) -> Vec<EdgeRef> {
        if self.sel_mode != SelMode::Edge {
            return vec![];
        }
        self.edge_sel.iter().filter(|e| e.faces[1] < 0).cloned().collect()
    }

    /// Fill Hole (Maya's; Romain 23 nov., 14 h 52): the open border by each
    /// of these closed by a face, one step; a part that closes is a solid
    /// again.
    pub fn fill_holes(&mut self, at: Vec<EdgeRef>) -> Option<FeatureId> {
        if at.is_empty() {
            self.toast("Fill hole: pick an open border's edge (Edge mode)", true);
            return None;
        }
        self.abort_gestures();
        let mut id = None;
        self.edit("Fill hole", |d| id = Some(d.add_knife(Knife { pts: vec![], segs: vec![], remove_faces: vec![], remove_edges: vec![], fill: at }, "Fill hole")));
        let id = id?;
        self.edge_sel.clear();
        self.selection = vec![id];
        self.toast("Hole filled", false);
        Some(id)
    }

    /// Shift + drag on the move gizmo with faces picked (Maya): the picked
    /// faces extruded, each along its own normal, instead of moved. False
    /// when no face is picked.
    pub fn shift_extrude_picked(&mut self, p: [f32; 2]) -> bool {
        if self.sel_mode != SelMode::Face {
            return false;
        }
        let Some(r) = self.face_sel.first().copied() else { return false };
        let Some(face) = self.model_face_of(&r) else { return false };
        self.push_start(face, r.at, p, true);
        self.push.is_some()
    }

    /// Shift or Ctrl held at the press on a picked face or edge (Maya /
    /// Plasticity): a face, Shift extrudes it, Ctrl insets it; an edge,
    /// Shift pulls it out. False when the press is on nothing picked (then
    /// the drag sizes a fillet or draws a rectangle).
    pub fn begin_face_drag(&mut self, p: [f32; 2], shift: bool, ctrl: bool) -> bool {
        match self.sel_mode {
            SelMode::Face => {
                let Some(h) = self.pick_at(p) else { return false };
                let at = h.point.as_dvec3();
                if shift {
                    // (Romain 23 nov., 7 h 05: "Et que Shift + Drag face une
                    // extrude?") on a face not picked too: it alone
                    if !self.picked_faces().contains(&h.face) {
                        let Some(r) = self.face_ref(h.face, at) else { return false };
                        self.face_sel = vec![r];
                    }
                    self.push_start(h.face, at, p, true);
                    return self.push.is_some();
                }
                if !self.picked_faces().contains(&h.face) {
                    return false;
                }
                if ctrl {
                    let Some((r, origin, _)) = self.face_plane(h.face, at, "Inset") else { return true };
                    // the other picked flat faces are inset with it
                    let mut more: Vec<FaceRef> = vec![];
                    for (fi, q) in self.picked_face_points() {
                        let flat = self.model.as_ref().and_then(|m| m.faces.get(fi)).map(|f| f.kind == SurfKind::Plane).unwrap_or(false);
                        if fi == h.face || !flat {
                            continue;
                        }
                        if let Some((r2, o2, _)) = self.face_plane(fi, q, "Inset") {
                            more.push(FaceRef { tag: r2.tag, at: o2 });
                        }
                    }
                    self.begin_live("Inset");
                    let mut id = 0;
                    let mut others = vec![];
                    self.live(|d| {
                        id = d.add_push(Push { face: FaceRef { tag: r.tag, at: origin }, inset: 0.1, depth: 0.0 });
                        for m in &more {
                            let name = d.fresh_name("Inset");
                            let o = d.add_push(Push { face: m.clone(), inset: 0.1, depth: 0.0 });
                            if let Some(f) = d.feature_mut(o) {
                                f.name = name;
                            }
                            others.push(o);
                        }
                    });
                    self.face_sel.clear();
                    self.selection = std::iter::once(id).chain(others.iter().copied()).collect();
                    let px = self.mm_per_point(at);
                    self.face_drag = Some(FaceDrag { id, what: FaceDragWhat::Inset, press: p, px, size: 0.1, others, dirs: vec![] });
                    return true;
                }
                false
            }
            SelMode::Edge if shift => {
                let Some(i) = self.pick_edge(p) else { return false };
                let Some(r) = self.edge_ref(i) else { return false };
                if !self.edge_sel.iter().any(|x| x.key() == r.key() && x.at.distance(r.at) < 1e-3) {
                    return false;
                }
                let Some(n) = self.edge_normal(i) else {
                    self.toast("Pull edge: only between flat faces for now", true);
                    return true;
                };
                let (ro, rd) = self.ray(p);
                let t0 = closest_on_line(r.at, n, ro, rd).unwrap_or(0.0);
                // the other picked edges are pulled with it, each along its
                // own faces' normal, by the same amount (a step each)
                let mut more: Vec<(EdgeRef, DVec3)> = vec![];
                for x in self.edge_sel.clone() {
                    if (x.key() == r.key() && x.at.distance(r.at) < 1e-3) || x.faces[1] < 0 {
                        continue;
                    }
                    if let Some(n2) = self.model_edge_of(&x).and_then(|k| self.edge_normal(k)) {
                        more.push((x, n2));
                    }
                }
                let Some(t) = Tweak::new(vec![Comp::Edge(r.clone())]) else { return true };
                self.begin_live(t.label());
                let mut id = 0;
                let mut others = vec![];
                self.live(|d| {
                    id = d.add_tweak(t);
                    for (x, _) in &more {
                        if let Some(t2) = Tweak::new(vec![Comp::Edge(x.clone())]) {
                            others.push(d.add_tweak(t2));
                        }
                    }
                });
                self.edge_sel.clear();
                self.selection = std::iter::once(id).chain(others.iter().copied()).collect();
                let px = self.mm_per_point(r.at);
                let dirs = more.iter().map(|m| m.1).collect();
                self.face_drag = Some(FaceDrag { id, what: FaceDragWhat::Edge { at: r.at, n, t0 }, press: p, px, size: 0.0, others, dirs });
                true
            }
            _ => false,
        }
    }

    /// The mean of an edge's two faces' outward normals (both flat).
    fn edge_normal(&self, i: usize) -> Option<DVec3> {
        let m = self.model.as_ref()?;
        let e = m.edges.get(i)?;
        let f = |k: i32| m.faces.get(k as usize).filter(|f| f.kind == SurfKind::Plane).map(|f| DVec3::from_array(f.normal).normalize());
        let n = f(e.faces[0])? + f(e.faces[1])?;
        (n.length() > 1e-6).then(|| n.normalize())
    }

    pub fn update_face_drag(&mut self, p: [f32; 2]) {
        let Some(mut g) = self.face_drag.clone() else { return };
        let id = g.id;
        match g.what {
            FaceDragWhat::Inset => {
                let dist = (p[0] - g.press[0]).hypot(p[1] - g.press[1]) as f64;
                let size = nice_round(dist * g.px, g.px).max(0.1);
                g.size = size;
                let others = g.others.clone();
                self.live(|d| {
                    for i in std::iter::once(id).chain(others.iter().copied()) {
                        if let Some(pu) = d.feature_mut(i).and_then(|f| f.push.as_mut()) {
                            pu.inset = size;
                        }
                    }
                });
            }
            FaceDragWhat::Edge { at, n, t0 } => {
                let (ro, rd) = self.ray(p);
                let Some(t) = closest_on_line(at, n, ro, rd) else { return };
                let d = self.snap(t - t0);
                g.size = d;
                let more: Vec<(FeatureId, DVec3)> = g.others.iter().copied().zip(g.dirs.iter().copied()).collect();
                self.live(|doc| {
                    for (i, dir) in std::iter::once((id, n)).chain(more.iter().copied()) {
                        if let Some(tw) = doc.feature_mut(i).and_then(|f| f.tweak.as_mut()) {
                            tw.delta = dir * d;
                        }
                    }
                });
            }
        }
        self.face_drag = Some(g);
    }

    pub fn end_face_drag(&mut self) {
        let Some(g) = self.face_drag.take() else { return };
        if g.what == FaceDragWhat::Inset || g.size.abs() > 1e-9 {
            self.end_live();
            self.selection = std::iter::once(g.id).chain(g.others.iter().copied()).filter(|i| self.doc.feature(*i).is_some()).collect();
        } else {
            // not moved: nothing made
            self.cancel_live();
            self.selection.clear();
        }
    }
}

// ------------------------------------------- fillets and chamfers by drag

/// A size rounded to a step fit for `px` mm per point (1, 2, 5 × 10ⁿ, at
/// least two points' worth), as in Rectify.
pub fn nice_round(v: f64, px: f64) -> f64 {
    let want = (px * 2.0).max(1e-4);
    let e = 10f64.powf(want.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * e).find(|s| *s >= want).unwrap_or(want);
    (v / step).round() * step
}

impl AppState {
    /// mm per screen point at a world point.
    pub fn mm_per_point(&self, at: DVec3) -> f64 {
        let v = self.view();
        (v.pixel_size_at((at.as_vec3() - v.eye).length()) as f64).max(1e-5)
    }

    /// A press and drag in Edge or Vertex mode (Rectify): on an edge, a new
    /// fillet (or chamfer, the option) of it and the edges picked with it;
    /// on a fillet's faces, its size; on a sketch or path corner, its cut;
    /// at a thin plate's corner, its short edge rounded. False: nothing
    /// there to size.
    pub fn begin_round_drag(&mut self, p: [f32; 2]) -> bool {
        let chamfer = self.round_chamfer;
        match self.sel_mode {
            SelMode::Edge => {
                if let Some(r) = self.pick_edge(p).and_then(|i| self.edge_ref(i)).filter(|r| r.faces[1] >= 0) {
                    let same = |x: &EdgeRef| x.key() == r.key() && x.at.distance(r.at) < 1e-3;
                    if !self.edge_sel.iter().any(same) {
                        self.edge_sel = vec![r.clone()];
                    }
                    self.selection.clear();
                    let px = self.mm_per_point(r.at);
                    self.round_drag = Some(RoundDrag { what: RoundWhat::Edges, press: p, r0: 0.0, px, chamfer, size: 0.0, chamfer0: chamfer, tension_from: None, tension: 0.0 });
                    return true;
                }
                let Some(h) = self.pick_at(p) else { return false };
                let Some(id) = self.feature_of_face(h.face) else { return false };
                let Some(rd) = self.doc.feature(id).and_then(|f| f.round.clone()) else { return false };
                self.edge_sel.clear();
                self.selection = vec![id];
                let (ro, dir) = self.ray(p);
                let px = self.mm_per_point(ro + dir * h.t as f64);
                self.begin_live(rd.label());
                self.round_drag = Some(RoundDrag { what: RoundWhat::Step(id), press: p, r0: rd.size, px, chamfer: rd.chamfer, size: rd.size, chamfer0: rd.chamfer, tension_from: None, tension: rd.tension });
                true
            }
            SelMode::Vertex => {
                let Some(v) = self.pick_vertex(p) else { return false };
                if let Some((id, i)) = self.profile_corner(v) {
                    if !self.vert_sel.iter().any(|q| self.profile_corner(*q) == Some((id, i))) {
                        self.vert_sel = vec![v];
                    }
                    let Some((size, was_chamfer, _)) = self.profile_corner_state_of(id, i) else { return false };
                    // a sharp corner takes the option, a cut one keeps its kind
                    let chamfer = if size > 1e-9 { was_chamfer } else { chamfer };
                    let Some((_, bis, per, y)) = self.corner_dot(id, i, v, chamfer) else { return false };
                    let px = self.mm_per_point(v);
                    let corners = self.picked_profile_corners();
                    self.begin_live(if chamfer { "Chamfer corner" } else { "Fillet corner" });
                    self.round_drag = Some(RoundDrag { what: RoundWhat::Corners { corners, id, bis, per, y }, press: p, r0: size, px, chamfer, size, chamfer0: chamfer, tension_from: None, tension: self.corner_tension(id, i) });
                    return true;
                }
                let es = self.vertex_edges(v);
                if !es.is_empty() {
                    self.edge_sel = es;
                    self.vert_sel.clear();
                    self.hover_vertex = None;
                    self.selection.clear();
                    let px = self.mm_per_point(v);
                    self.round_drag = Some(RoundDrag { what: RoundWhat::Edges, press: p, r0: 0.0, px, chamfer, size: 0.0, chamfer0: chamfer, tension_from: None, tension: 0.0 });
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// The pointer moved during the drag (Romain 19 nov., "direct
    /// fillet"): Ctrl held, a chamfer; Shift held, the tension follows the
    /// pointer instead of the size (up: fuller, down: flatter; on a sketch
    /// corner its dot along the line to the corner).
    pub fn update_round_drag(&mut self, at: [f32; 2], ctrl: bool, shift: bool) {
        let Some(mut g) = self.round_drag.clone() else { return };
        let chamfer = g.chamfer0 || ctrl;
        // Shift down or up: the tension starts from where it is, the size
        // goes on from where it was
        match (shift && !chamfer, g.tension_from.is_some()) {
            (true, false) => {
                let t0 = if g.tension > 0.0 { g.tension } else { self.circle_tension(&g) };
                g.tension_from = Some((at, t0));
            }
            (false, true) => {
                g.tension_from = None;
                g.press = at;
                g.r0 = g.size;
            }
            _ => {}
        }
        g.chamfer = chamfer;
        if let Some((from, t0)) = g.tension_from {
            let mut t = (t0 + (from[1] - at[1]) as f64 * 0.004).clamp(0.05, 0.95);
            if let RoundWhat::Corners { id, .. } = &g.what {
                if let Some(rho) = self.corner_rho_at(*id, &g, at) {
                    t = rho.clamp(0.05, 0.95);
                }
            }
            let t = (t * 100.0).round() / 100.0;
            g.tension = t;
            self.round_drag = Some(g.clone());
            self.set_drag_tension(&g, t);
            return;
        }
        self.round_drag = Some(g.clone());
        let dist = (at[0] - g.press[0]).hypot(at[1] - g.press[1]) as f64;
        let mut size = g.r0 + dist * g.px;
        // a corner's dot follows the pointer along its bisector (from where
        // the press was, so the size does not jump)
        if let RoundWhat::Corners { id, bis, per, y, .. } = &g.what {
            if let (Some(a), Some(b)) = (self.along_corner(*id, *bis, *y, g.press), self.along_corner(*id, *bis, *y, at)) {
                size = (g.r0 + (b - a) / per).max(0.0);
            }
        }
        let size = nice_round(size, g.px).max(0.0);
        // Ctrl is held for this drag only: the top bar's option stays
        let option = self.round_chamfer;
        match g.what {
            RoundWhat::Edges => {
                if size > 1e-6 {
                    self.set_round(size, g.chamfer, true);
                    if let Some(id) = self.selected_round().map(|f| f.id) {
                        if let Some(r) = self.round_drag.as_mut() {
                            r.what = RoundWhat::Step(id);
                        }
                    }
                }
            }
            RoundWhat::Step(_) => self.set_round(size, g.chamfer, false),
            RoundWhat::Corners { ref corners, id, y, .. } => {
                let v = if g.chamfer { -size } else { size };
                self.live(|d| {
                    for (fid, i) in corners {
                        if let Some(pr) = d.feature_mut(*fid).and_then(editable) {
                            if pr.corner.len() < pr.points.len() {
                                pr.corner.resize(pr.points.len(), 0.0);
                            }
                            pr.corner[*i] = v;
                        }
                    }
                });
                // the picks follow the cut corners (their vertices moved)
                self.vert_sel = corners.iter().filter_map(|(fid, i)| self.corner_vertex(*fid, *i, if *fid == id { y } else { 0.0 })).collect();
            }
        }
        self.round_chamfer = option;
        let shown = match &g.what {
            RoundWhat::Corners { id, corners, .. } => corners.first().and_then(|c| self.profile_corner_state_of(*id, c.1)).map(|(s, _, max)| s.min(max)).unwrap_or(size),
            _ => self.selected_round().and_then(|f| f.round.as_ref().map(|r| r.size)).unwrap_or(size),
        };
        if let Some(r) = self.round_drag.as_mut() {
            r.size = shown;
        }
    }

    /// How far along a sketch corner's bisector the pointer is, on the
    /// corner's plane (the sketch's frame).
    fn along_corner(&self, id: FeatureId, bis: [f64; 2], y: f64, sp: [f32; 2]) -> Option<f64> {
        let q = self.on_sketch_plane(id, y, sp)?;
        Some(q[0] * bis[0] + q[1] * bis[1])
    }

    /// The pointer on the plane of a sketch at level `y` of its frame (its
    /// local x, z).
    fn on_sketch_plane(&self, id: FeatureId, y: f64, sp: [f32; 2]) -> Option<[f64; 2]> {
        let f = self.doc.feature(id)?;
        let inv = f.matrix().inverse();
        let (ro, rd) = self.ray(sp);
        let (lo, ld) = (inv.transform_point3(ro), inv.transform_vector3(rd));
        if ld.y.abs() < 1e-6 {
            return None;
        }
        let t = (y - lo.y) / ld.y;
        let q = lo + ld * t;
        (t > 0.0).then_some([q.x, q.z])
    }

    /// Shift on a sketch corner: the conic's rho that puts its middle where
    /// the pointer is, along the line from its ends' middle to the corner.
    fn corner_rho_at(&self, id: FeatureId, g: &RoundDrag, at: [f32; 2]) -> Option<f64> {
        let RoundWhat::Corners { corners, y, .. } = &g.what else { return None };
        let (_, i) = *corners.iter().find(|c| c.0 == id)?;
        let f = self.doc.feature(id)?;
        let (Prim::Profile { x, z, .. } | Prim::Path { x, z }, Some(pr)) = (&f.prim, &f.profile) else { return None };
        let c = pr.cuts(*x, *z).get(i).copied().flatten()?;
        let q = self.on_sketch_plane(id, *y, at)?;
        let mid = [(c.a[0] + c.b[0]) * 0.5, (c.a[1] + c.b[1]) * 0.5];
        let d = [c.corner[0] - mid[0], c.corner[1] - mid[1]];
        let l2 = d[0] * d[0] + d[1] * d[1];
        (l2 > 1e-12).then(|| ((q[0] - mid[0]) * d[0] + (q[1] - mid[1]) * d[1]) / l2)
    }

    /// The circular arc's tension where the drag is (the start of a Shift
    /// drag on a round fillet): a corner's own, else a right angle's.
    fn circle_tension(&self, g: &RoundDrag) -> f64 {
        if let RoundWhat::Corners { id, corners, .. } = &g.what {
            if let Some(f) = self.doc.feature(*id) {
                if let (Prim::Profile { x, z, .. } | Prim::Path { x, z }, Some(pr), Some((_, i))) = (&f.prim, &f.profile, corners.iter().find(|c| c.0 == *id)) {
                    if let Some(c) = pr.cuts(*x, *z).get(*i).copied().flatten() {
                        return c.rho_or_circle();
                    }
                }
            }
        }
        nurbs_kernel::doc::circle_rho(std::f64::consts::FRAC_PI_2)
    }

    /// A sketch corner's conic rho as built (its own tension, else the
    /// circle's for its angle).
    pub fn corner_rho(&self, id: FeatureId, i: usize) -> Option<f64> {
        let f = self.doc.feature(id)?;
        let (Prim::Profile { x, z, .. } | Prim::Path { x, z }, Some(pr)) = (&f.prim, &f.profile) else { return None };
        pr.cuts(*x, *z).get(i).copied().flatten().map(|c| c.rho_or_circle())
    }

    /// A sketch corner's tension (0: circular).
    pub fn corner_tension(&self, id: FeatureId, i: usize) -> f64 {
        self.doc.feature(id).and_then(|f| f.profile.as_ref()).and_then(|p| p.tension.get(i).copied()).unwrap_or(0.0)
    }

    /// The drag's tension written to what it sizes.
    fn set_drag_tension(&mut self, g: &RoundDrag, t: f64) {
        match &g.what {
            RoundWhat::Step(id) => self.set_round_tension(*id, t, false),
            RoundWhat::Corners { corners, .. } => {
                let corners = corners.clone();
                self.set_corners_tension(&corners, t, false);
            }
            RoundWhat::Edges => {}
        }
    }

    /// A fillet step's tension (0: circular). `start`: one undo step.
    pub fn set_round_tension(&mut self, id: FeatureId, t: f64, start: bool) {
        if start || !self.is_live() {
            self.end_live();
            self.begin_live("Tension");
        }
        self.live(|d| {
            if let Some(r) = d.feature_mut(id).and_then(|f| f.round.as_mut()) {
                r.tension = t;
            }
        });
    }

    /// Sketch corners' tension (0: circular).
    pub fn set_corners_tension(&mut self, corners: &[(FeatureId, usize)], t: f64, start: bool) {
        if start || !self.is_live() {
            self.end_live();
            self.begin_live("Tension");
        }
        self.live(|d| {
            for (fid, i) in corners {
                if let Some(pr) = d.feature_mut(*fid).and_then(editable) {
                    if pr.tension.len() < pr.points.len() {
                        pr.tension.resize(pr.points.len(), 0.0);
                    }
                    pr.tension[*i] = t;
                }
            }
        });
    }

    pub fn end_round_drag(&mut self) {
        if let Some(g) = self.round_drag.take() {
            self.end_live();
            if g.what == RoundWhat::Edges {
                // released before any size: the edge stays picked
                return;
            }
            if !matches!(g.what, RoundWhat::Corners { .. }) {
                self.edge_sel.clear();
            }
        }
    }

    /// The cut of sketch corner `i` of feature `id` (size, chamfer, the
    /// largest it can be).
    pub fn profile_corner_state_of(&self, id: FeatureId, i: usize) -> Option<(f64, bool, f64)> {
        let f = self.doc.feature(id)?;
        let (Prim::Profile { x, z, .. } | Prim::Path { x, z }, Some(pr)) = (&f.prim, &f.profile) else { return None };
        let c = pr.corner.get(i).copied().unwrap_or(0.0);
        let mut big = pr.clone();
        big.corner.resize(big.points.len(), 0.0);
        big.corner[i] = if c < 0.0 { -1e9 } else { 1e9 };
        let max = nurbs_kernel::doc::corner_cut_open(&big.scaled(*x, *z), &big.smooth, &big.corner, i, big.open).map(|k| k.max).unwrap_or(0.0);
        Some((c.abs(), c < 0.0, max))
    }

    /// Where sketch corner `i` of `id` shows as a vertex at the level `y`
    /// of its frame: a path's cut by its middle, a solid's by the cut's
    /// start (a vertex of the part), a sharp one at its point.
    pub fn corner_vertex(&self, id: FeatureId, i: usize, y: f64) -> Option<DVec3> {
        let f = self.doc.feature(id)?;
        let (Prim::Profile { x, z, .. } | Prim::Path { x, z }, Some(pr)) = (&f.prim, &f.profile) else { return None };
        let path = matches!(f.prim, Prim::Path { .. });
        let q = match pr.cuts(*x, *z).get(i).copied().flatten() {
            Some(c) => if path { c.m } else { c.a },
            None => *pr.scaled(*x, *z).get(i)?,
        };
        Some(f.matrix().transform_point3(DVec3::new(q[0], y, q[1])))
    }

    /// The dot of sketch corner `i` (the middle of its cut): the corner's
    /// point, the way the dot moves as the cut grows, how far per mm, and
    /// the level of the vertex `v` in the sketch's frame.
    fn corner_dot(&self, id: FeatureId, i: usize, v: DVec3, chamfer: bool) -> Option<([f64; 2], [f64; 2], f64, f64)> {
        let f = self.doc.feature(id)?;
        let (Prim::Profile { x, z, .. } | Prim::Path { x, z }, Some(pr)) = (&f.prim, &f.profile) else { return None };
        let pts = pr.scaled(*x, *z);
        let p = *pts.get(i)?;
        let mut c = pr.corner.clone();
        c.resize(pr.points.len(), 0.0);
        // the cut's middle at a small size, well within the sides' room
        let s = 0.05 * {
            let n = pts.len();
            let a = pts[(i + n - 1) % n];
            let b = pts[(i + 1) % n];
            (a[0] - p[0]).hypot(a[1] - p[1]).min((b[0] - p[0]).hypot(b[1] - p[1]))
        }
        .max(1e-4);
        c[i] = if chamfer { -s } else { s };
        let m = nurbs_kernel::doc::corner_cut_open(&pts, &pr.smooth, &c, i, pr.open)?.m;
        let d = [m[0] - p[0], m[1] - p[1]];
        let len = d[0].hypot(d[1]);
        if len < 1e-12 {
            return None;
        }
        let y = f.matrix().inverse().transform_point3(v).y;
        Some((p, [d[0] / len, d[1] / len], len / s, y))
    }

    /// Ctrl + B (Rectify's bevel): a chamfer on what is picked: the picked
    /// edges (1 mm), the selected fillet becomes a chamfer, the picked
    /// sketch corners (their size, else 2 mm).
    pub fn make_bevel(&mut self) {
        self.round_chamfer = true;
        if let Some(r) = self.selected_round().and_then(|f| f.round.clone()) {
            self.set_round(r.size, true, true);
            self.end_live();
        } else if !self.edge_sel.is_empty() {
            self.set_round(1.0, true, true);
            self.end_live();
            self.edge_sel.clear();
        } else if let Some((s, _, _)) = self.profile_corner_state() {
            self.set_profile_corners(if s > 1e-9 { s } else { 2.0 }, true, true);
            self.end_live();
        } else {
            self.toast("Ctrl + B: pick edges (2) or sketch corners (1) first", true);
        }
    }
}

/// A feature's outline to edit by hand: a curve of the Curves set becomes
/// a plain path first (3ds Max's Convert to Editable Spline), else its
/// settings would make it again.
fn editable(f: &mut nurbs_kernel::Feature) -> Option<&mut nurbs_kernel::Profile> {
    f.curve = None;
    f.profile.as_mut()
}
