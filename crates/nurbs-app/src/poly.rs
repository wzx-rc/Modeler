//! The Poly tab (Romain, 20 nov. 2026: "j'ai besoin de faire un truc
//! polygonal, je vais dans Polygone, je crée mon mesh"): polygonal meshes
//! made in the scene (Cube, Cylinder, Sphere, Plane) and edited Maya
//! style: the same Vertex / Edge / Face / Object modes and Q W E R gizmo
//! as the Forme tab, here on the mesh's own points; Extrude (Ctrl + E) and
//! Inset (I) follow the pointer until a click, Insert Edge Loop (Ctrl + R)
//! on an edge, Delete, Subdivide, the smooth preview, and Send to Sculpt.

use crate::meshes::{refit, smoothed, to_unit, world_points};
use crate::state::{AppState, Handle, SelMode};
use glam::{DVec3, Vec3};
use nurbs_kernel::mesh::{delete_faces, edge_key, edge_ring, extrude_faces, inset_faces, loop_cut, poly_centre, poly_cube, poly_cylinder, poly_normal, poly_plane, quad_sphere, triangulated, MeshData, MeshKind};
use nurbs_kernel::{Feature, FeatureId};

/// A linked surface's control points kept in place when its Poly layer's
/// unit box is fitted again (`MeshData::store`).
fn remap_patch(md: &mut MeshData, old: Vec3, base: Vec3, new: Vec3) {
    if let Some(p) = md.patch.as_mut() {
        p.remap(old, base, new);
    }
}

/// The components picked on one Poly mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PolySel {
    pub id: FeatureId,
    pub verts: Vec<u32>,
    pub edges: Vec<(u32, u32)>,
    pub faces: Vec<u32>,
}

impl PolySel {
    pub fn is_empty(&self) -> bool {
        self.verts.is_empty() && self.edges.is_empty() && self.faces.is_empty()
    }
}

/// The component under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PolyHover {
    Vert(FeatureId, u32),
    Edge(FeatureId, (u32, u32)),
    Face(FeatureId, u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullKind {
    Extrude,
    Inset,
}

/// Extrude or Inset following the pointer until a click (Maya's
/// manipulator, without one): the mesh as it was, the faces, how far.
#[derive(Clone, Debug)]
pub struct PolyPull {
    pub kind: PullKind,
    pub id: FeatureId,
    pub faces: Vec<u32>,
    pub before: Feature,
    /// Extrude: the line along the faces' mean normal (world); Inset: the
    /// faces' centre on screen.
    pub at: Vec3,
    pub normal: Vec3,
    pub press: [f32; 2],
    pub amount: f32,
}

/// The shapes of the Poly tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolyShape {
    Cube,
    Cylinder,
    Sphere,
    Plane,
}

impl PolyShape {
    pub const ALL: [PolyShape; 4] = [PolyShape::Cube, PolyShape::Cylinder, PolyShape::Sphere, PolyShape::Plane];
    pub fn label(self) -> &'static str {
        match self {
            PolyShape::Cube => "Cube",
            PolyShape::Cylinder => "Cylinder",
            PolyShape::Sphere => "Sphere",
            PolyShape::Plane => "Plane",
        }
    }
}

impl AppState {
    /// A new polygonal shape, 20 mm across, its centre on the view's target.
    pub fn add_poly(&mut self, shape: PolyShape) {
        let s = 20.0f32;
        let (p, f) = match shape {
            PolyShape::Cube => poly_cube(s),
            PolyShape::Cylinder => poly_cylinder(s, 16),
            PolyShape::Sphere => quad_sphere(s, 4, 1.0),
            PolyShape::Plane => poly_plane(s, 4),
        };
        let lift = if shape == PolyShape::Plane { 0.0 } else { s as f64 * 0.5 };
        let at = self.camera.target.as_dvec3() - DVec3::new(0.0, lift, 0.0);
        let mut made = 0;
        self.edit(&format!("New poly {}", shape.label().to_lowercase()), |d| {
            made = d.add_mesh(MeshKind::Poly, &p, f, at, glam::DQuat::IDENTITY, shape.label());
        });
        self.selection = vec![made];
        self.poly_sel = None;
        self.mesh_gen += 1;
    }

    /// The Poly mesh the component modes work on: the one picked on, else
    /// the single selected Poly mesh.
    pub fn poly_target(&self) -> Option<FeatureId> {
        if let Some(s) = &self.poly_sel {
            if self.doc.feature(s.id).is_some() {
                return Some(s.id);
            }
        }
        let polys: Vec<FeatureId> = self
            .selection
            .iter()
            .copied()
            .filter(|id| self.is_poly(*id) && !self.doc.feature(*id).map(crate::linked::shows_nurbs).unwrap_or(false))
            .collect();
        (polys.len() == 1).then(|| polys[0])
    }

    pub fn is_poly(&self, id: FeatureId) -> bool {
        self.doc.feature(id).and_then(|f| f.mesh.as_ref()).map(|m| m.kind == MeshKind::Poly).unwrap_or(false)
    }

    /// The picked points (world) of the components.
    pub fn poly_points(&self) -> Vec<Vec3> {
        let Some(s) = &self.poly_sel else { return vec![] };
        let Some(f) = self.doc.feature(s.id) else { return vec![] };
        let pts = world_points(f);
        self.poly_verts_of(s, f).iter().filter_map(|v| pts.get(*v as usize).copied()).collect()
    }

    /// The points the picked components hold.
    pub fn poly_verts_of(&self, s: &PolySel, f: &Feature) -> Vec<u32> {
        let mut v: Vec<u32> = s.verts.clone();
        for e in &s.edges {
            v.push(e.0);
            v.push(e.1);
        }
        if let Some(m) = &f.mesh {
            for fi in &s.faces {
                if let Some(q) = m.faces.get(*fi as usize) {
                    v.extend_from_slice(q);
                }
            }
        }
        v.sort();
        v.dedup();
        v
    }

    /// The gizmo's place in a component mode of the Poly tab.
    pub fn poly_pivot(&self) -> Option<DVec3> {
        let pts = self.poly_points();
        if pts.is_empty() {
            return None;
        }
        Some((pts.iter().sum::<Vec3>() / pts.len() as f32).as_dvec3())
    }

    /// The component under the pointer in the current mode.
    pub fn poly_hover_at(&self, p: [f32; 2]) -> Option<PolyHover> {
        let hit = self.pick_mesh(p, Some(MeshKind::Poly));
        // a component of the mesh under the pointer, else of the one worked on
        let id = hit.map(|h| h.id).or_else(|| self.poly_target())?;
        let f = self.doc.feature(id)?;
        let m = f.mesh.as_ref()?;
        let v = self.view();
        let pts = world_points(f);
        let (ro, _) = v.ray(p[0], p[1]);
        // seen: nothing of the mesh in front of it
        let seen = |q: Vec3| {
            let d = q - ro;
            let dist = d.length();
            match crate::meshes::ray_mesh(f, ro, d / dist) {
                Some(h) => h.t > dist - dist * 0.002 - 0.05,
                None => true,
            }
        };
        match self.sel_mode {
            SelMode::Face => hit.filter(|h| h.id == id).map(|h| PolyHover::Face(id, h.face)),
            SelMode::Vertex => {
                let mut best: Option<(f32, u32)> = None;
                for (i, q) in pts.iter().enumerate() {
                    if let Some(s) = v.project(*q) {
                        let d = (s[0] - p[0]).hypot(s[1] - p[1]);
                        if d < 14.0 && best.map(|b| d < b.0).unwrap_or(true) && seen(*q) {
                            best = Some((d, i as u32));
                        }
                    }
                }
                best.map(|b| PolyHover::Vert(id, b.1))
            }
            SelMode::Edge => {
                let mut best: Option<(f32, (u32, u32))> = None;
                for e in m.edges() {
                    let (a, b) = (pts[e.0 as usize], pts[e.1 as usize]);
                    let (Some(sa), Some(sb)) = (v.project(a), v.project(b)) else { continue };
                    let d = seg_dist(p, sa, sb);
                    if d < 10.0 && best.map(|bb| d < bb.0).unwrap_or(true) && seen((a + b) * 0.5) {
                        best = Some((d, e));
                    }
                }
                best.map(|b| PolyHover::Edge(id, b.1))
            }
            SelMode::Object => None,
        }
    }

    /// A click in a component mode: the component picked (Shift: added or
    /// taken out, Ctrl: taken out); nothing there: none picked.
    pub fn poly_click(&mut self, p: [f32; 2], shift: bool, ctrl: bool) {
        let h = self.poly_hover_at(p);
        let Some(h) = h else {
            if !shift && !ctrl {
                if let Some(s) = self.poly_sel.as_mut() {
                    s.verts.clear();
                    s.edges.clear();
                    s.faces.clear();
                }
            }
            return;
        };
        let id = match h {
            PolyHover::Vert(id, _) | PolyHover::Edge(id, _) | PolyHover::Face(id, _) => id,
        };
        let mut s = match self.poly_sel.clone() {
            Some(s) if s.id == id => s,
            _ => PolySel { id, ..Default::default() },
        };
        if !shift && !ctrl {
            s = PolySel { id, ..Default::default() };
        }
        fn toggle<T: PartialEq + Copy>(v: &mut Vec<T>, x: T, shift: bool, ctrl: bool) {
            let has = v.contains(&x);
            if ctrl || (shift && has) {
                v.retain(|y| *y != x);
            } else if !has {
                v.push(x);
            }
        }
        match h {
            PolyHover::Vert(_, i) => toggle(&mut s.verts, i, shift, ctrl),
            PolyHover::Edge(_, e) => toggle(&mut s.edges, e, shift, ctrl),
            PolyHover::Face(_, f) => toggle(&mut s.faces, f, shift, ctrl),
        }
        if !self.selection.contains(&id) {
            self.selection = vec![id];
        }
        self.poly_sel = Some(s);
    }

    /// A rectangle in a component mode: what lies inside it on screen.
    pub fn poly_marquee(&mut self, a: [f32; 2], b: [f32; 2], shift: bool, ctrl: bool) {
        let Some(id) = self.poly_target().or_else(|| self.shown_meshes().iter().find(|f| self.is_poly(f.id)).map(|f| f.id)) else { return };
        let Some(f) = self.doc.feature(id).cloned() else { return };
        let Some(m) = f.mesh.clone() else { return };
        let v = self.view();
        let pts = world_points(&f);
        let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
        let inside = |q: Vec3| v.project(q).map(|s| s[0] >= lo[0] && s[0] <= hi[0] && s[1] >= lo[1] && s[1] <= hi[1]).unwrap_or(false);
        let mut s = match self.poly_sel.clone() {
            Some(s) if s.id == id && (shift || ctrl) => s,
            _ => PolySel { id, ..Default::default() },
        };
        let apply = |list: &mut Vec<u32>, x: u32| {
            if ctrl {
                list.retain(|y| *y != x);
            } else if !list.contains(&x) {
                list.push(x);
            }
        };
        match self.sel_mode {
            SelMode::Vertex => {
                for (i, q) in pts.iter().enumerate() {
                    if inside(*q) {
                        apply(&mut s.verts, i as u32);
                    }
                }
            }
            SelMode::Face => {
                for (i, q) in m.faces.iter().enumerate() {
                    if inside(poly_centre(&pts, q)) {
                        apply(&mut s.faces, i as u32);
                    }
                }
            }
            SelMode::Edge => {
                for e in m.edges() {
                    if inside((pts[e.0 as usize] + pts[e.1 as usize]) * 0.5) {
                        if ctrl {
                            s.edges.retain(|y| *y != e);
                        } else if !s.edges.contains(&e) {
                            s.edges.push(e);
                        }
                    }
                }
            }
            SelMode::Object => {}
        }
        self.selection = vec![id];
        self.poly_sel = Some(s);
    }

    // ------------------------------------------------------- gizmo drags

    /// A gizmo drag on the picked components begins (move, turn, scale).
    pub fn begin_poly_drag(&mut self, h: Handle, p: [f32; 2]) {
        let Some(s) = self.poly_sel.clone() else { return };
        let Some(f) = self.doc.feature(s.id).cloned() else { return };
        let Some(pivot) = (match h {
            Handle::Cage(g) => self.cage().map(|c| c.point(g)),
            _ => self.pivot(),
        }) else {
            return;
        };
        let Some(grab) = self.handle_param(h, pivot, p) else { return };
        self.begin_live(match h {
            Handle::Rot(_) => "Rotate components",
            Handle::Cage(_) => "Scale components",
            _ => "Move components",
        });
        let cage = if matches!(h, Handle::Cage(_)) { self.cage() } else { None };
        self.poly_drag = Some(PolyDrag { handle: h, grab, pivot, before: f, cage, verts: self.poly_verts_of(&s, self.doc.feature(s.id).unwrap()) });
    }

    pub fn update_poly_drag(&mut self, p: [f32; 2]) {
        let Some(d) = self.poly_drag.clone() else { return };
        let Some(now) = self.handle_param(d.handle, d.pivot, p) else { return };
        let mut pts = world_points(&d.before);
        let pivot = d.pivot.as_vec3();
        match d.handle {
            Handle::Rot(a) => {
                // the angle round the axis, from the grab to the pointer
                let ax = DVec3::AXES[a];
                let (u, w) = (d.grab - d.pivot, now - d.pivot);
                let (u, w) = (u - ax * u.dot(ax), w - ax * w.dot(ax));
                let mut ang = u.cross(w).dot(ax).atan2(u.dot(w));
                if self.settings.snap {
                    let st = 15f64.to_radians();
                    ang = (ang / st).round() * st;
                }
                self.gizmo_readout = Some(format!("{:.0}°", ang.to_degrees()));
                let q = glam::Quat::from_axis_angle(ax.as_vec3(), ang as f32);
                for v in &d.verts {
                    let i = *v as usize;
                    pts[i] = pivot + q * (pts[i] - pivot);
                }
            }
            Handle::Cage(g) => {
                let Some(c) = d.cage else { return };
                let Some((k, about)) = self.cage_scale(&c, g, p, &[]) else { return };
                let (k, about) = (k.as_vec3(), about.as_vec3());
                for v in &d.verts {
                    let i = *v as usize;
                    pts[i] = about + (pts[i] - about) * k;
                }
            }
            h => {
                let Some(mask) = crate::state::move_mask(h) else { return };
                let mut delta = (now - d.grab) * mask;
                if self.settings.snap && !self.snap_off {
                    delta = delta.map(|x| self.snap(x));
                }
                let dv = delta.as_vec3();
                for v in &d.verts {
                    pts[*v as usize] += dv;
                }
            }
        }
        let unit = to_unit(&d.before, &pts);
        let id = d.before.id;
        self.live(|doc| {
            if let Some(f) = doc.feature_mut(id) {
                if let Some(m) = f.mesh.as_mut() {
                    m.make_mut().verts = unit;
                }
            }
        });
        self.mesh_gen += 1;
    }

    pub fn end_poly_drag(&mut self) {
        let Some(d) = self.poly_drag.take() else { return };
        if let Some(f) = self.doc.feature_mut(d.before.id) {
            refit(f);
        }
        self.gizmo_readout = None;
        self.mesh_gen += 1;
        self.end_live();
    }

    // ------------------------------------------------------- operations

    /// Changes the Poly mesh `id` in millimetres (its frame), one undo step;
    /// its box fitted again after.
    pub fn poly_edit(&mut self, label: &str, id: FeatureId, f: impl FnOnce(&mut Vec<Vec3>, &mut Vec<Vec<u32>>, &mut MeshData)) {
        self.edit(label, |doc| {
            let Some(ft) = doc.feature_mut(id) else { return };
            let size = crate::meshes::size(ft);
            let Some(m) = ft.mesh.as_mut() else { return };
            let md = m.make_mut();
            let mut pts = md.local(size);
            let mut faces = md.faces.clone();
            f(&mut pts, &mut faces, md);
            let (s2, base) = md.store(&pts, size);
            md.faces = faces;
            ft.origin += ft.rot * base.as_dvec3();
            ft.prim = nurbs_kernel::Prim::Box { x: s2.x as f64, y: s2.y as f64, z: s2.z as f64 };
        });
        self.mesh_gen += 1;
    }

    /// The picked faces (Face mode), or those all of whose points are
    /// picked.
    fn picked_faces_poly(&self) -> Option<(FeatureId, Vec<u32>)> {
        let s = self.poly_sel.as_ref()?;
        if !s.faces.is_empty() {
            return Some((s.id, s.faces.clone()));
        }
        let f = self.doc.feature(s.id)?;
        let m = f.mesh.as_ref()?;
        let vs = self.poly_verts_of(s, f);
        let fs: Vec<u32> = m.faces.iter().enumerate().filter(|(_, q)| q.iter().all(|v| vs.contains(v))).map(|(i, _)| i as u32).collect();
        (!fs.is_empty()).then_some((s.id, fs))
    }

    /// Ctrl + E or I on picked faces: the pull begins (the pointer sets how
    /// far, a click keeps it).
    pub fn begin_poly_pull(&mut self, kind: PullKind, p: [f32; 2]) -> bool {
        let Some((id, faces)) = self.picked_faces_poly() else {
            self.toast(&format!("{}: pick faces first (Face mode, 3)", if kind == PullKind::Extrude { "Extrude" } else { "Inset" }), true);
            return false;
        };
        let Some(f) = self.doc.feature(id).cloned() else { return false };
        let pts = world_points(&f);
        let m = f.mesh.as_ref().unwrap();
        let mut n = Vec3::ZERO;
        let mut c = Vec3::ZERO;
        for fi in &faces {
            let q = &m.faces[*fi as usize];
            n += poly_normal(&pts, q);
            c += poly_centre(&pts, q);
        }
        let c = c / faces.len() as f32;
        self.begin_live(if kind == PullKind::Extrude { "Extrude faces" } else { "Inset faces" });
        self.poly_pull = Some(PolyPull { kind, id, faces, before: f, at: c, normal: n.normalize_or(Vec3::Y), press: p, amount: 0.0 });
        self.update_poly_pull(p);
        true
    }

    pub fn update_poly_pull(&mut self, p: [f32; 2]) {
        let Some(mut g) = self.poly_pull.clone() else { return };
        let amount = match g.kind {
            PullKind::Extrude => {
                // along the normal line, where the pointer's ray passes nearest
                let (ro, rd) = self.ray(p);
                let t = crate::state::closest_on_line(g.at.as_dvec3(), g.normal.as_dvec3(), ro, rd).unwrap_or(0.0);
                let (pr, prd) = self.ray(g.press);
                let t0 = crate::state::closest_on_line(g.at.as_dvec3(), g.normal.as_dvec3(), pr, prd).unwrap_or(0.0);
                let d = t - t0;
                if self.settings.snap && !self.snap_off {
                    self.snap(d) as f32
                } else {
                    d as f32
                }
            }
            PullKind::Inset => {
                let v = self.view();
                let c = v.project(g.at).unwrap_or(g.press);
                let d0 = (g.press[0] - c[0]).hypot(g.press[1] - c[1]).max(20.0);
                let d1 = (p[0] - c[0]).hypot(p[1] - c[1]);
                ((d0 - d1) / d0 * 0.9 + 0.1).clamp(0.02, 0.95)
            }
        };
        g.amount = amount;
        // made again from the mesh as it was
        let before = g.before.clone();
        let size = crate::meshes::size(&before);
        let Some(bm) = before.mesh.clone() else { return };
        let mut pts = bm.local(size);
        let mut faces = bm.faces.clone();
        match g.kind {
            PullKind::Extrude => {
                // in the mesh's own millimetres (its frame turns, never scales)
                extrude_faces(&mut pts, &mut faces, &g.faces, amount);
            }
            PullKind::Inset => {
                inset_faces(&mut pts, &mut faces, &g.faces, amount);
            }
        }
        let (unit, s2, base) = nurbs_kernel::mesh::normalize(&pts);
        let id = g.id;
        self.live(|doc| {
            if let Some(f) = doc.feature_mut(id) {
                *f = before.clone();
                let old = crate::meshes::size(f);
                f.origin += f.rot * base.as_dvec3();
                f.prim = nurbs_kernel::Prim::Box { x: s2.x as f64, y: s2.y as f64, z: s2.z as f64 };
                if let Some(m) = f.mesh.as_mut() {
                    let md = m.make_mut();
                    md.verts = unit;
                    remap_patch(md, old, base, s2);
                    md.faces = faces;
                }
            }
        });
        let label = match g.kind {
            PullKind::Extrude => format!("Extrude {} mm", crate::widgets::fmt_mm(amount)),
            PullKind::Inset => format!("Inset {:.0} %", amount * 100.0),
        };
        self.gizmo_readout = Some(label);
        self.poly_pull = Some(g);
        self.mesh_gen += 1;
    }

    /// A click: the pull kept; the faces made stay picked.
    pub fn finish_poly_pull(&mut self) {
        let Some(g) = self.poly_pull.take() else { return };
        self.gizmo_readout = None;
        self.poly_sel = Some(PolySel { id: g.id, faces: g.faces.clone(), ..Default::default() });
        self.end_live();
    }

    pub fn cancel_poly_pull(&mut self) {
        if self.poly_pull.take().is_some() {
            self.gizmo_readout = None;
            self.cancel_live();
            self.mesh_gen += 1;
        }
    }

    /// Delete in a component mode: the picked faces (the faces round the
    /// picked points or edges) taken out.
    pub fn poly_delete(&mut self) -> bool {
        let Some(s) = self.poly_sel.clone() else { return false };
        if s.is_empty() {
            return false;
        }
        let Some(f) = self.doc.feature(s.id) else { return false };
        let Some(m) = f.mesh.as_ref() else { return false };
        let gone: Vec<u32> = if !s.faces.is_empty() {
            s.faces.clone()
        } else {
            let vs = &s.verts;
            m.faces
                .iter()
                .enumerate()
                .filter(|(_, q)| q.iter().any(|v| vs.contains(v)) || (0..q.len()).any(|i| s.edges.contains(&edge_key(q[i], q[(i + 1) % q.len()]))))
                .map(|(i, _)| i as u32)
                .collect()
        };
        if gone.len() >= m.faces.len() {
            // all of it: the mesh itself goes
            self.poly_sel = None;
            self.selection = vec![s.id];
            self.delete_selected();
            return true;
        }
        self.poly_edit("Delete faces", s.id, |pts, faces, md| {
            let mut tmp = MeshData { kind: MeshKind::Poly, verts: pts.iter().map(|p| p.to_array()).collect(), faces: faces.clone(), smooth: md.smooth, angle: md.angle, patch: None, sdf: None };
            delete_faces(&mut tmp, &gone);
            *pts = tmp.verts.iter().map(|v| Vec3::from(*v)).collect();
            *faces = tmp.faces;
        });
        self.poly_sel = Some(PolySel { id: s.id, ..Default::default() });
        true
    }

    /// Ctrl + R: the Insert Edge Loop tool (a click on an edge).
    pub fn arm_poly_loop(&mut self) {
        self.commit_gestures(self.principal());
        if self.sel_mode == SelMode::Object {
            self.sel_mode = SelMode::Edge;
        }
        self.tool = crate::state::Tool::PolyLoop;
    }

    /// Insert Edge Loop: across the ring of the edge under the pointer.
    pub fn poly_loop_at(&mut self, p: [f32; 2]) -> bool {
        let Some((id, e)) = (PolyProbe { st: self }).edge_at(p) else { return false };
        let mut made = vec![];
        self.poly_edit("Insert edge loop", id, |pts, faces, _| {
            made = loop_cut(pts, faces, e);
        });
        self.poly_sel = Some(PolySel { id, edges: made, ..Default::default() });
        self.sel_mode = SelMode::Edge;
        true
    }

    /// The loop a click would insert (shown under the pointer).
    pub fn poly_loop_preview(&self, p: [f32; 2]) -> Vec<(Vec3, Vec3)> {
        // the edge nearest the pointer on the face under it, whatever the mode
        let Some((id, e)) = (PolyProbe { st: self }).edge_at(p) else { return vec![] };
        let Some(f) = self.doc.feature(id) else { return vec![] };
        let Some(m) = f.mesh.as_ref() else { return vec![] };
        let pts = world_points(f);
        let (ring, closed) = edge_ring(&m.faces, e);
        let mids: Vec<Vec3> = ring.iter().map(|e| (pts[e.0 as usize] + pts[e.1 as usize]) * 0.5).collect();
        let mut out: Vec<(Vec3, Vec3)> = mids.windows(2).map(|w| (w[0], w[1])).collect();
        if closed && mids.len() > 2 {
            out.push((mids[mids.len() - 1], mids[0]));
        }
        out
    }

    /// Subdivide (Catmull-Clark once) the selected Poly meshes.
    pub fn poly_subdivide(&mut self) {
        let ids: Vec<FeatureId> = self.poly_ids();
        if ids.is_empty() {
            self.toast("Subdivide: select a Poly mesh", true);
            return;
        }
        for id in ids {
            self.poly_edit("Subdivide", id, |pts, faces, _| {
                let (p2, f2) = nurbs_kernel::mesh::catmull_clark(pts, faces);
                *pts = p2;
                *faces = f2;
            });
        }
        self.poly_sel = None;
    }

    /// The selected Poly meshes (or the one worked on).
    pub fn poly_ids(&self) -> Vec<FeatureId> {
        let mut ids: Vec<FeatureId> = self.selection.iter().copied().filter(|id| self.is_poly(*id)).collect();
        if ids.is_empty() {
            ids.extend(self.poly_target());
        }
        ids
    }

    /// The smooth preview of the selected Poly meshes on or off.
    pub fn poly_toggle_smooth(&mut self) {
        let ids = self.poly_ids();
        if ids.is_empty() {
            self.toast("Smooth preview: select a Poly mesh", true);
            return;
        }
        let on = !ids.iter().any(|id| self.doc.feature(*id).and_then(|f| f.mesh.as_ref()).map(|m| m.smooth).unwrap_or(false));
        self.edit(if on { "Smooth preview on" } else { "Smooth preview off" }, |d| {
            for id in &ids {
                if let Some(m) = d.feature_mut(*id).and_then(|f| f.mesh.as_mut()) {
                    m.make_mut().smooth = on;
                }
            }
        });
        self.mesh_gen += 1;
    }

    /// Send to Sculpt: the selected Poly meshes smoothed into clay (their
    /// quads subdivided until dense enough, in triangles), in place.
    pub fn poly_to_sculpt(&mut self) {
        let ids = self.poly_ids();
        if ids.is_empty() {
            self.toast("Send to Sculpt: select a Poly mesh", true);
            return;
        }
        // a linked object goes into its own Sculpt state (the clay stays
        // linked to its NURBS and Poly layer)
        if ids.len() == 1 && self.doc.feature(ids[0]).and_then(crate::linked::patch_of).is_some() {
            self.set_patch_state(ids[0], nurbs_kernel::patch::PatchState::Sculpt);
            return;
        }
        let mut made = vec![];
        for id in ids {
            let Some(f) = self.doc.feature(id).cloned() else { continue };
            let Some(m) = f.mesh.clone() else { continue };
            let size = crate::meshes::size(&f);
            let pts = m.local(size);
            let mut levels = 0;
            let mut n = m.faces.iter().map(|q| q.len()).sum::<usize>();
            while n < 30000 && levels < 6 {
                n *= 4;
                levels += 1;
            }
            let (p, fs, _) = smoothed(&pts, &m.faces, levels);
            let tris = triangulated(&fs);
            let name = format!("{} clay", f.name);
            self.edit("Send to Sculpt", |d| {
                let new = d.add_mesh(MeshKind::Sculpt, &p, tris, f.origin, f.rot, &name);
                if let Some(nf) = d.feature_mut(new) {
                    nf.material = f.material;
                }
                if let Some(of) = d.feature_mut(id) {
                    of.visible = false;
                }
                made.push(new);
            });
        }
        self.selection = made;
        self.poly_sel = None;
        self.mesh_gen += 1;
        self.toast("Sent to Sculpt: the Poly mesh is kept, hidden", false);
    }
}

/// Edge picking whatever the mode (the loop tool's preview).
struct PolyProbe<'a> {
    st: &'a AppState,
}

impl PolyProbe<'_> {
    fn edge_at(&self, p: [f32; 2]) -> Option<(FeatureId, (u32, u32))> {
        let st = self.st;
        let hit = st.pick_mesh(p, Some(MeshKind::Poly))?;
        let f = st.doc.feature(hit.id)?;
        let m = f.mesh.as_ref()?;
        let v = st.view();
        let pts = world_points(f);
        let q = &m.faces[hit.face as usize];
        let mut best: Option<(f32, (u32, u32))> = None;
        for i in 0..q.len() {
            let e = edge_key(q[i], q[(i + 1) % q.len()]);
            let (Some(sa), Some(sb)) = (v.project(pts[e.0 as usize]), v.project(pts[e.1 as usize])) else { continue };
            let d = seg_dist(p, sa, sb);
            if best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, e));
            }
        }
        best.map(|b| (hit.id, b.1))
    }
}

/// A Poly gizmo drag under way.
#[derive(Clone, Debug)]
pub struct PolyDrag {
    pub handle: Handle,
    pub grab: DVec3,
    pub pivot: DVec3,
    pub before: Feature,
    pub cage: Option<crate::cage::CageBox>,
    pub verts: Vec<u32>,
}

pub fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (abx, aby) = (b[0] - a[0], b[1] - a[1]);
    let l2 = (abx * abx + aby * aby).max(1e-6);
    let t = (((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / l2).clamp(0.0, 1.0);
    (a[0] + abx * t - p[0]).hypot(a[1] + aby * t - p[1])
}

// ------------------------------------------------------------------ Bevel

/// Maya's Bevel on the picked edges (Ctrl + B; Romain 20 nov., 15 h 40:
/// "travaille le bevel comme Maya, avec chamfer et bevel, et surtout en
/// travaillant bien les coins"): made at once at a first width, the
/// pointer's travel left or right then sets it until a click, `[` `]` the
/// segments, C Chamfer or rounded. Kept, its window keeps its settings
/// until something else changes (made again from the mesh before it).
#[derive(Clone, Debug)]
pub struct PolyBevel {
    pub id: FeatureId,
    pub edges: Vec<(u32, u32)>,
    pub before: Feature,
    /// The pointer sets the width (until a click).
    pub live: bool,
    pub press: [f32; 2],
    pub w0: f32,
    pub mm_per_pt: f32,
    pub width: f32,
    pub segments: u32,
    pub chamfer: bool,
    /// What it made: the width (less when clamped), the new faces, the
    /// feature (the window stays while it is still the document's).
    pub made: f32,
    pub clamped: bool,
    pub faces: Vec<u32>,
    pub result: Option<Feature>,
}

/// The Cut tool (Maya's Multi-Cut, K; Romain 20 nov., 15 h 40: "un cut
/// tool"): each click puts a point on an edge (or takes a vertex) and joins
/// it to the one before across the face they share; Ctrl + click inserts an
/// edge loop there. One undo step per cut.
#[derive(Clone, Debug)]
pub struct PolyCut {
    pub id: FeatureId,
    pub last: Option<u32>,
    pub made: Vec<(u32, u32)>,
}

/// What a click of the Cut tool would take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CutAt {
    Vert(FeatureId, u32),
    /// An edge (its two points, from the first) at a fraction.
    Edge(FeatureId, u32, u32, f32),
}

impl AppState {
    /// The edges a Bevel works on: the picked edges; in Face mode the
    /// picked faces' edges; in Vertex mode the edges between picked points;
    /// a whole Poly mesh selected in Object mode: all its edges (Maya).
    pub fn poly_bevel_edges(&self) -> Option<(FeatureId, Vec<(u32, u32)>)> {
        if let Some(s) = self.poly_sel.as_ref().filter(|s| !s.is_empty() && self.sel_mode != SelMode::Object) {
            let f = self.doc.feature(s.id)?;
            let m = f.mesh.as_ref()?;
            let mut e: Vec<(u32, u32)> = match self.sel_mode {
                SelMode::Edge => s.edges.clone(),
                SelMode::Face => s.faces.iter().filter_map(|fi| m.faces.get(*fi as usize)).flat_map(|q| (0..q.len()).map(move |i| edge_key(q[i], q[(i + 1) % q.len()]))).collect(),
                SelMode::Vertex => m.edges().into_iter().filter(|e| s.verts.contains(&e.0) && s.verts.contains(&e.1)).collect(),
                SelMode::Object => vec![],
            };
            e.sort();
            e.dedup();
            return (!e.is_empty()).then_some((s.id, e));
        }
        let id = self.poly_ids().into_iter().next()?;
        let m = self.doc.feature(id)?.mesh.as_ref()?;
        Some((id, m.edges()))
    }

    /// Ctrl + B in the Poly tab: the bevel made at a first width; the
    /// pointer sets it until a click.
    pub fn begin_poly_bevel(&mut self, p: [f32; 2]) -> bool {
        let Some((id, edges)) = self.poly_bevel_edges() else {
            self.toast("Bevel: pick edges (Edge mode, 2), faces, or select a Poly mesh", true);
            return false;
        };
        let Some(f) = self.doc.feature(id).cloned() else { return false };
        let pts = world_points(&f);
        let lens: Vec<f32> = edges.iter().map(|e| (pts[e.0 as usize] - pts[e.1 as usize]).length()).collect();
        let mean = lens.iter().sum::<f32>() / lens.len().max(1) as f32;
        let c = edges.iter().map(|e| (pts[e.0 as usize] + pts[e.1 as usize]) * 0.5).sum::<Vec3>() / edges.len().max(1) as f32;
        let v = self.view();
        let mm_per_pt = v.pixel_size_at((c - v.eye).length()).max(1e-4);
        // a tenth of the edges' mean length, on a round value
        let w0 = (crate::faces::nice_round((mean * 0.1) as f64, 0.025) as f32).max(0.05);
        let (segments, chamfer) = self.poly_bevel.as_ref().map(|b| (b.segments, b.chamfer)).unwrap_or((1, true));
        self.abort_gestures();
        self.begin_live("Bevel");
        self.poly_bevel = Some(PolyBevel { id, edges, before: f, live: true, press: p, w0, mm_per_pt, width: w0, segments, chamfer, made: 0.0, clamped: false, faces: vec![], result: None });
        self.apply_poly_bevel();
        true
    }

    /// The pointer during a live bevel: its travel right widens it.
    pub fn update_poly_bevel(&mut self, p: [f32; 2]) {
        let Some(b) = self.poly_bevel.as_mut().filter(|b| b.live) else { return };
        let mut w = (b.w0 + (p[0] - b.press[0]) * b.mm_per_pt).max(0.0);
        if !self.snap_off {
            w = crate::faces::nice_round(w as f64, b.mm_per_pt as f64) as f32;
        }
        if (w - b.width).abs() > 1e-6 {
            b.width = w;
            self.apply_poly_bevel();
        }
    }

    /// Made again from the mesh before the bevel, with its settings.
    pub fn apply_poly_bevel(&mut self) {
        let Some(mut b) = self.poly_bevel.clone() else { return };
        let before = b.before.clone();
        let size = crate::meshes::size(&before);
        let Some(bm) = before.mesh.clone() else { return };
        let mut pts = bm.local(size);
        let mut faces = bm.faces.clone();
        let opts = nurbs_kernel::mesh_bevel::BevelOptions { width: b.width, segments: b.segments, chamfer: b.chamfer };
        let res = if b.width > 1e-5 { nurbs_kernel::mesh_bevel::bevel(&mut pts, &mut faces, &b.edges, opts) } else { None };
        let id = b.id;
        match res {
            Some(r) => {
                b.made = r.width;
                b.clamped = r.clamped;
                b.faces = r.faces;
                let (unit, s2, base) = nurbs_kernel::mesh::normalize(&pts);
                self.live(|doc| {
                    if let Some(f) = doc.feature_mut(id) {
                        *f = before.clone();
                        let old = crate::meshes::size(f);
                        f.origin += f.rot * base.as_dvec3();
                        f.prim = nurbs_kernel::Prim::Box { x: s2.x as f64, y: s2.y as f64, z: s2.z as f64 };
                        if let Some(m) = f.mesh.as_mut() {
                            let md = m.make_mut();
                            md.verts = unit;
                            remap_patch(md, old, base, s2);
                            md.faces = faces;
                        }
                    }
                });
            }
            None => {
                // no width: the mesh as it was
                b.made = 0.0;
                b.clamped = false;
                b.faces.clear();
                self.live(|doc| {
                    if let Some(f) = doc.feature_mut(id) {
                        *f = before.clone();
                    }
                });
            }
        }
        b.result = self.doc.feature(id).cloned();
        let kind = if b.chamfer { "Chamfer" } else { "Bevel" };
        let segs = if b.segments == 1 { "1 segment".to_string() } else { format!("{} segments", b.segments) };
        let clamp = if b.clamped { " · clamped" } else { "" };
        self.gizmo_readout = b.live.then(|| format!("{kind} {} mm · {segs}{clamp}", crate::widgets::fmt_mm(b.made)));
        if !b.faces.is_empty() {
            self.poly_sel = Some(PolySel { id, faces: b.faces.clone(), ..Default::default() });
        }
        self.poly_bevel = Some(b);
        self.mesh_gen += 1;
    }

    /// A click: the bevel kept (its faces picked, its window stays).
    pub fn finish_poly_bevel(&mut self) {
        let Some(b) = self.poly_bevel.as_mut() else { return };
        if !b.live {
            return;
        }
        b.live = false;
        let faces = b.faces.clone();
        let id = b.id;
        self.gizmo_readout = None;
        self.end_live();
        if !faces.is_empty() {
            self.sel_mode = SelMode::Face;
            self.poly_sel = Some(PolySel { id, faces, ..Default::default() });
        }
        if let Some(b) = self.poly_bevel.as_mut() {
            b.result = self.doc.feature(id).cloned();
        }
    }

    pub fn cancel_poly_bevel(&mut self) {
        if let Some(b) = self.poly_bevel.take() {
            self.gizmo_readout = None;
            if b.live {
                self.cancel_live();
            }
            self.mesh_gen += 1;
        }
    }

    /// Still the bevel just made (or being made): its window shows.
    pub fn poly_bevel_valid(&self) -> bool {
        match &self.poly_bevel {
            Some(b) if b.live => true,
            // kept while its mesh is still the one worked on and unchanged
            Some(b) => {
                b.result.is_some()
                    && self.doc.feature(b.id) == b.result.as_ref()
                    && (self.selection.contains(&b.id)
                        || (self.sel_mode != SelMode::Object && self.poly_sel.as_ref().map(|s| s.id == b.id).unwrap_or(false)))
            }
            None => false,
        }
    }

    /// The window's settings changed (one undo step per slider drag).
    pub fn set_poly_bevel(&mut self, width: f32, segments: u32, chamfer: bool, start: bool) {
        let Some(b) = self.poly_bevel.as_mut() else { return };
        b.width = width.max(0.0);
        b.segments = segments.clamp(1, 32);
        b.chamfer = chamfer;
        let live_pointer = b.live;
        if !live_pointer {
            if start {
                if self.is_live() {
                    self.end_live();
                }
                self.begin_live("Bevel");
            } else if !self.is_live() {
                self.begin_live("Bevel");
            }
        }
        self.apply_poly_bevel();
        if !live_pointer {
            if let Some(b) = self.poly_bevel.as_ref() {
                let (id, faces) = (b.id, b.faces.clone());
                if !faces.is_empty() {
                    self.poly_sel = Some(PolySel { id, faces, ..Default::default() });
                }
            }
        }
    }

    /// `[` / `]` during a bevel: a segment less or more.
    pub fn poly_bevel_segments(&mut self, by: i32) {
        let Some(b) = self.poly_bevel.clone() else { return };
        let s = (b.segments as i32 + by).clamp(1, 32) as u32;
        self.set_poly_bevel(b.width, s, b.chamfer, true);
    }

    pub fn poly_bevel_toggle_chamfer(&mut self) {
        let Some(b) = self.poly_bevel.clone() else { return };
        self.set_poly_bevel(b.width, b.segments, !b.chamfer, true);
    }

    // -------------------------------------------------------------- Cut

    /// K in the Poly tab: the Cut tool (Maya's Multi-Cut).
    pub fn arm_poly_cut(&mut self) {
        self.commit_gestures(self.principal());
        self.tool = crate::state::Tool::PolyCut;
    }

    /// What a click would take: a point (within 12 points) before an edge
    /// (its middle within 6 points, else where the pointer is along it).
    pub fn poly_cut_at(&self, p: [f32; 2]) -> Option<CutAt> {
        let hit = self.pick_mesh(p, Some(MeshKind::Poly));
        let id = match (&self.poly_cut, hit) {
            (Some(c), _) if self.doc.feature(c.id).is_some() => c.id,
            (_, Some(h)) => h.id,
            _ => return None,
        };
        let f = self.doc.feature(id)?;
        let m = f.mesh.as_ref()?;
        let v = self.view();
        let pts = world_points(f);
        let (ro, _) = v.ray(p[0], p[1]);
        let seen = |q: Vec3| {
            let d = q - ro;
            let dist = d.length();
            match crate::meshes::ray_mesh(f, ro, d / dist) {
                Some(h) => h.t > dist - dist * 0.002 - 0.05,
                None => true,
            }
        };
        let mut best: Option<(f32, u32)> = None;
        for (i, q) in pts.iter().enumerate() {
            if let Some(s) = v.project(*q) {
                let d = (s[0] - p[0]).hypot(s[1] - p[1]);
                if d < 12.0 && best.map(|b| d < b.0).unwrap_or(true) && seen(*q) {
                    best = Some((d, i as u32));
                }
            }
        }
        if let Some((_, i)) = best {
            return Some(CutAt::Vert(id, i));
        }
        let mut be: Option<(f32, (u32, u32))> = None;
        for e in m.edges() {
            let (a, b) = (pts[e.0 as usize], pts[e.1 as usize]);
            let (Some(sa), Some(sb)) = (v.project(a), v.project(b)) else { continue };
            let d = seg_dist(p, sa, sb);
            if d < 10.0 && be.map(|x| d < x.0).unwrap_or(true) && seen((a + b) * 0.5) {
                be = Some((d, e));
            }
        }
        let (_, e) = be?;
        let (a, b) = (pts[e.0 as usize], pts[e.1 as usize]);
        let (sa, sb) = (v.project(a)?, v.project(b)?);
        let mid = v.project((a + b) * 0.5)?;
        let t = if (mid[0] - p[0]).hypot(mid[1] - p[1]) < 6.0 {
            0.5
        } else {
            // where the pointer's ray passes nearest the edge
            let (ro, rd) = self.ray(p);
            let len = (b - a).length().max(1e-6);
            let s = crate::state::closest_on_line(a.as_dvec3(), ((b - a) / len).as_dvec3(), ro, rd).map(|s| s as f32 / len);
            s.unwrap_or_else(|| {
                let (abx, aby) = (sb[0] - sa[0], sb[1] - sa[1]);
                ((p[0] - sa[0]) * abx + (p[1] - sa[1]) * aby) / (abx * abx + aby * aby).max(1e-6)
            })
            .clamp(0.03, 0.97)
        };
        Some(CutAt::Edge(id, e.0, e.1, t))
    }

    /// Where a cut point is (world).
    pub fn cut_point(&self, c: CutAt) -> Option<Vec3> {
        match c {
            CutAt::Vert(id, i) => world_points(self.doc.feature(id)?).get(i as usize).copied(),
            CutAt::Edge(id, a, b, t) => {
                let pts = world_points(self.doc.feature(id)?);
                Some(pts.get(a as usize)?.lerp(*pts.get(b as usize)?, t))
            }
        }
    }

    /// The cut's last point (world).
    pub fn poly_cut_last(&self) -> Option<Vec3> {
        let c = self.poly_cut.as_ref()?;
        world_points(self.doc.feature(c.id)?).get(c.last? as usize).copied()
    }

    /// Would a click there join the last point across a face?
    pub fn poly_cut_joins(&self, at: CutAt) -> bool {
        let Some(c) = &self.poly_cut else { return false };
        let Some(last) = c.last else { return false };
        let Some(m) = self.doc.feature(c.id).and_then(|f| f.mesh.as_ref()) else { return false };
        match at {
            CutAt::Vert(id, v) => id == c.id && nurbs_kernel::mesh::common_face(&m.faces, last, v).is_some(),
            CutAt::Edge(id, a, b, _) => {
                // a face holding the last point and that edge as a side
                id == c.id
                    && m.faces.iter().any(|q| {
                        let n = q.len();
                        q.contains(&last) && (0..n).any(|i| edge_key(q[i], q[(i + 1) % n]) == edge_key(a, b)) && a != last && b != last
                    })
            }
        }
    }

    /// A click with the Cut tool: the point put in (and joined to the one
    /// before); Ctrl: an edge loop there. Nothing under it: the cut ends.
    pub fn poly_cut_click(&mut self, p: [f32; 2], ctrl: bool) {
        let Some(at) = self.poly_cut_at(p) else {
            self.end_poly_cut();
            return;
        };
        let id = match at {
            CutAt::Vert(id, _) | CutAt::Edge(id, ..) => id,
        };
        if self.poly_cut.as_ref().map(|c| c.id != id).unwrap_or(false) {
            self.end_poly_cut();
        }
        if self.poly_cut.is_none() {
            self.begin_live("Cut");
            self.poly_cut = Some(PolyCut { id, last: None, made: vec![] });
        }
        let Some(f) = self.doc.feature(id).cloned() else { return };
        let size = crate::meshes::size(&f);
        let Some(m) = f.mesh.clone() else { return };
        let mut pts = m.local(size);
        let mut faces = m.faces.clone();
        let mut cut = self.poly_cut.clone().unwrap();
        if ctrl {
            if let CutAt::Edge(_, a, b, t) = at {
                let made = nurbs_kernel::mesh::loop_cut_at(&mut pts, &mut faces, (a, b), t);
                cut.made.extend(made);
                cut.last = None;
            } else {
                return;
            }
        } else {
            let v = match at {
                CutAt::Vert(_, v) => v,
                CutAt::Edge(_, a, b, t) => nurbs_kernel::mesh::split_edge(&mut pts, &mut faces, a, b, t),
            };
            if let Some(l) = cut.last {
                if nurbs_kernel::mesh::split_face(&mut faces, l, v).is_some() {
                    cut.made.push(edge_key(l, v));
                }
            }
            cut.last = Some(v);
        }
        let (unit, s2, base) = nurbs_kernel::mesh::normalize(&pts);
        self.live(|doc| {
            if let Some(ft) = doc.feature_mut(id) {
                let old = crate::meshes::size(ft);
                ft.origin += ft.rot * base.as_dvec3();
                ft.prim = nurbs_kernel::Prim::Box { x: s2.x as f64, y: s2.y as f64, z: s2.z as f64 };
                if let Some(mr) = ft.mesh.as_mut() {
                    let md = mr.make_mut();
                    md.verts = unit;
                    remap_patch(md, old, base, s2);
                    md.faces = faces;
                }
            }
        });
        if !self.selection.contains(&id) {
            self.selection = vec![id];
        }
        self.poly_sel = Some(PolySel { id, edges: cut.made.clone(), ..Default::default() });
        self.poly_cut = Some(cut);
        self.mesh_gen += 1;
    }

    /// The cut ends (Enter, a right click, a click off the mesh): one undo
    /// step; the tool stays for the next one.
    pub fn end_poly_cut(&mut self) {
        if let Some(c) = self.poly_cut.take() {
            self.end_live();
            if !c.made.is_empty() {
                self.sel_mode = SelMode::Edge;
                self.poly_sel = Some(PolySel { id: c.id, edges: c.made, ..Default::default() });
            }
        }
    }

    /// Ctrl held over an edge: the loop a click would insert there.
    pub fn poly_cut_loop_preview(&self, at: CutAt) -> Vec<(Vec3, Vec3)> {
        let CutAt::Edge(id, a, b, t) = at else { return vec![] };
        let Some(f) = self.doc.feature(id) else { return vec![] };
        let Some(m) = f.mesh.as_ref() else { return vec![] };
        let mut pts = world_points(f);
        let mut faces = m.faces.clone();
        let made = nurbs_kernel::mesh::loop_cut_at(&mut pts, &mut faces, (a, b), t);
        made.iter().map(|e| (pts[e.0 as usize], pts[e.1 as usize])).collect()
    }
}
