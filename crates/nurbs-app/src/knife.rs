//! The Knife (K, Romain 19 nov.: "like the Maya Multicut tool") and Point
//! on edge (J), from Forme Rectify: clicks on the part's edges put points
//! in them; with the Knife, each point after the first is joined to the
//! one before by a line cut into the face holding both (straight on a flat
//! face; across a curved one, straight in the face's own grid). One
//! history step per cut (`Feature::cut`), ended by Enter, Esc, a right
//! click or a click off the edges.

use crate::state::*;
use glam::{DVec3, Vec3};
use nurbs_kernel::model::SurfKind;
use nurbs_kernel::{FeatureId, Knife, KnifePoint};

impl AppState {
    /// The edge under the pointer for the Knife: its index, the point on it
    /// and how far along it (0 to 1, from its first point); its middle
    /// catches the pointer within 8 points.
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
        if let Some(s) = self.view().project(mid.as_vec3()) {
            if (s[0] - p[0]).hypot(s[1] - p[1]) <= 8.0 {
                return Some((e, mid, 0.5));
            }
        }
        Some((e, best.2, best.1))
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

    /// The cut under way, if its step is still the last one.
    fn knife_going(&self) -> Option<(FeatureId, usize)> {
        let (id, last) = self.knife?;
        let f = self.doc.features.last()?;
        (f.id == id && f.cut.is_some()).then_some((id, last))
    }

    /// A click with the Knife or Point on edge: a point cut into the edge
    /// under the pointer; with the Knife, a line from the point before when
    /// both lie on one face (across a curved one it follows the face). Off
    /// the edges: the cut ends.
    pub fn knife_click(&mut self, p: [f32; 2]) {
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
                self.edit(name, |d| id = d.add_knife(Knife { pts: vec![kp], segs: vec![] }, name));
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
        let near = |pts: &[[f32; 3]]| {
            pts.windows(2).any(|w| {
                let (p, q) = (DVec3::from(Vec3::from(w[0])), DVec3::from(Vec3::from(w[1])));
                let d = q - p;
                let t = if d.length_squared() > 0.0 { ((a - p).dot(d) / d.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
                (p + d * t).distance(a) < tol
            })
        };
        let round_a: Vec<i32> = m.edges.iter().filter(|g| near(&g.pts)).flat_map(|g| g.faces).filter(|&f| f >= 0).collect();
        m.edges[e].faces.iter().filter(|&&fi| fi >= 0).any(|&fi| {
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
