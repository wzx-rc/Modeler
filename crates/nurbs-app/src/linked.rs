//! The linked surface in the app (`nurbs_kernel::patch`; Romain, 20 nov.
//! 2026, 18 h 12: "une surface… peut avoir les trois états à la fois"): one
//! element that is a NURBS patch and a Poly mesh. Made from a closed path of
//! four points (its window's Linked surface button); its own window switches
//! its state (NURBS, POLY; SCULPT is the next step), the tab following. In
//! the NURBS state its contour's 12 points are picked in Vertex mode (1) and
//! moved with the gizmo, the Poly layer following them (`apply_binding`);
//! the Poly layer's own shapes (an extrusion) are drawn over the surface,
//! not editable. In the Poly state it is a Poly mesh like any other; back to
//! NURBS, its points are bound to the surface again (`rebind`). A Carve
//! (box or cylinder) after it in the history cuts it live in both states
//! (`patch_solids`, drawn and picked through `meshes::linked_shown`).

use crate::meshes::{refit, size};
use crate::state::{AppState, Handle};
use glam::{DMat4, DVec3, Mat4, Vec3};
use nurbs_kernel::mesh::MeshKind;
use nurbs_kernel::patch::{self, Patch, PatchState, Solid, SolidKind, CONTOUR};
use nurbs_kernel::{Feature, FeatureId, Op, Prim};

/// The contour's points picked (indices into the 16 control points).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PatchSel {
    pub id: FeatureId,
    pub cvs: Vec<usize>,
}

/// A gizmo drag on the contour's points: the surface as it was, the points
/// it moves (a corner takes its two tangent points with it).
#[derive(Clone, Debug)]
pub struct PatchDrag {
    pub handle: Handle,
    pub grab: DVec3,
    pub pivot: DVec3,
    pub before: Feature,
    pub cvs: Vec<usize>,
}

pub fn patch_of(f: &Feature) -> Option<&Patch> {
    f.mesh.as_ref().and_then(|m| m.patch.as_ref())
}

/// Its state, when it is a linked surface.
pub fn state_of(f: &Feature) -> Option<PatchState> {
    patch_of(f).map(|p| p.state)
}

/// Drawn and edited as the NURBS (the Sculpt state is not made yet).
pub fn shows_nurbs(f: &Feature) -> bool {
    matches!(state_of(f), Some(PatchState::Nurbs | PatchState::Sculpt))
}

/// The frame in millimetres to the world.
pub fn to_world(f: &Feature) -> Mat4 {
    DMat4::from_rotation_translation(f.rot, f.origin).as_mat4()
}

/// Its control points in the world.
pub fn cvs_world(f: &Feature) -> Option<[Vec3; 16]> {
    let p = patch_of(f)?;
    let x = to_world(f);
    let cv = p.cvs(size(f));
    Some(cv.map(|c| x.transform_point3(c)))
}

/// The tangent points a corner carries.
fn handles(cv: usize) -> [usize; 2] {
    match cv {
        0 => [1, 4],
        3 => [2, 7],
        15 => [14, 11],
        _ => [13, 8],
    }
}

/// The Poly layer put back on the surface from its binding (after the
/// contour moved).
pub fn apply_binding(f: &mut Feature) {
    let s = size(f);
    let Some(m) = f.mesh.as_mut() else { return };
    let Some(p) = m.patch.clone() else { return };
    if p.bind.len() != m.verts.len() {
        return;
    }
    let cv = p.cvs(s);
    let unit: Vec<[f32; 3]> = p.bind.iter().map(|b| (patch::place(&cv, *b) / s).to_array()).collect();
    m.make_mut().verts = unit;
}

/// Each point of the Poly layer bound to the surface again (back from Poly);
/// what lies on its facets put on the surface itself (`patch::settle_on`).
pub fn rebind(f: &mut Feature) {
    let s = size(f);
    let Some(m) = f.mesh.as_mut() else { return };
    let Some(p) = m.patch.clone() else { return };
    let cv = p.cvs(s);
    let seeds = patch::seeds(&cv, 24);
    let mut bind: Vec<[f32; 5]> = m.local(s).iter().map(|q| patch::bind(&cv, *q, &seeds)).collect();
    // what lies on the layer's facets rides on the surface itself
    let settled = patch::settle_on(&cv, &mut bind, &m.faces);
    if let Some(pp) = m.make_mut().patch.as_mut() {
        pp.bind = bind;
    }
    if settled > 0 {
        apply_binding(f);
    }
}

/// How far (mm) the layer's points are from where their binding puts
/// them: about 0 unless something else moved them (an Object-mode scale,
/// a resize), which a contour move must not undo.
pub fn bound_off(f: &Feature) -> f32 {
    let s = size(f);
    let Some(m) = f.mesh.as_ref() else { return 0.0 };
    let Some(p) = m.patch.as_ref() else { return 0.0 };
    let pts = m.local(s);
    if p.bind.len() != pts.len() {
        return f32::INFINITY;
    }
    let cv = p.cvs(s);
    pts.iter().zip(&p.bind).map(|(q, b)| patch::place(&cv, *b).distance(*q)).fold(0.0, f32::max)
}

impl AppState {
    /// The closed path of four points selected, if any.
    pub fn four_point_path(&self) -> Option<&Feature> {
        let f = self.single()?;
        let pr = f.profile.as_ref()?;
        // a Path or a curve (a Rectangle, a 4-sided Polygon) of four sharp
        // corners
        let sharp = !pr.smooth.iter().any(|s| *s) && pr.corner.iter().all(|c| *c == 0.0);
        (f.is_path() && !pr.open && pr.points.len() == 4 && sharp && f.shape_bool.is_none()).then_some(f)
    }

    /// The linked surface on the selected closed path of four points: a
    /// NURBS patch on its corners, sides straight (they bend by their
    /// tangent points), and its Poly layer, an iso grid bound to it. The
    /// path is hidden. One undo step.
    pub fn make_linked_surface(&mut self) -> bool {
        let Some(path) = self.four_point_path().cloned() else {
            self.toast("Linked surface: select a closed path of 4 points", true);
            return false;
        };
        let (Prim::Path { x, z }, Some(pr)) = (path.prim, path.profile.as_ref()) else { return false };
        // the corners in the path's frame (mm), the normal along its +Y
        let mut c: Vec<Vec3> = pr.scaled(x, z).iter().map(|q| Vec3::new(q[0] as f32, 0.0, q[1] as f32)).collect();
        let tmp = Patch::new([c[0], c[1], c[2], c[3]]);
        let cv = tmp.cvs(Vec3::ONE);
        let (_, su, sv) = patch::eval(&cv, 0.5, 0.5);
        if su.cross(sv).y < 0.0 {
            c = vec![c[0], c[3], c[2], c[1]];
        }
        let corners = [c[0], c[1], c[2], c[3]];
        let mm = Patch::new(corners);
        let cv = mm.cvs(Vec3::ONE);
        // the grid: about 12 quads along the longer side
        let lu = (corners[1] - corners[0]).length().max((corners[2] - corners[3]).length());
        let lv = (corners[3] - corners[0]).length().max((corners[2] - corners[1]).length());
        let cell = lu.max(lv) / 12.0;
        let nu = ((lu / cell).round() as usize).clamp(3, 24);
        let nv = ((lv / cell).round() as usize).clamp(3, 24);
        let (pts, faces, uv) = patch::grid(&cv, nu, nv);
        let mut made = 0;
        self.edit("Linked surface", |d| {
            made = d.add_mesh(MeshKind::Poly, &pts, faces, path.origin, path.rot, "Surface");
            let Some(f) = d.feature_mut(made) else { return };
            // the control points in the mesh's unit box, as its points
            let s = size(f);
            let base = {
                let lo = pts.iter().fold(Vec3::splat(f32::MAX), |a, p| a.min(*p));
                let hi = pts.iter().fold(Vec3::splat(f32::MIN), |a, p| a.max(*p));
                Vec3::new((lo.x + hi.x) * 0.5, lo.y, (lo.z + hi.z) * 0.5)
            };
            let mut p = mm.clone();
            p.remap(Vec3::ONE, base, s);
            p.bind = uv.iter().map(|t| [t[0], t[1], 0.0, 0.0, 0.0]).collect();
            if let Some(m) = f.mesh.as_mut() {
                m.make_mut().patch = Some(p);
            }
            if let Some(pf) = d.feature_mut(path.id) {
                pf.visible = false;
            }
        });
        self.selection = vec![made];
        self.poly_sel = None;
        self.patch_sel = None;
        self.mesh_gen += 1;
        self.toast("Linked surface: NURBS and Poly, switch in its window", false);
        true
    }

    /// The linked surface the window and the contour's points work on: the
    /// single selected one.
    pub fn linked_target(&self) -> Option<FeatureId> {
        let f = self.single()?;
        patch_of(f).map(|_| f.id)
    }

    /// Switches its state, one undo step; the tab follows (NURBS: Forme,
    /// Poly: Poly). Back to NURBS, the Poly layer is bound to the surface
    /// again: its own shapes (an extrusion) then ride on the contour.
    pub fn set_patch_state(&mut self, id: FeatureId, s: PatchState) {
        use crate::workspace::Workspace;
        if s == PatchState::Sculpt {
            self.toast("Sculpt state: the next step (a sculpt layer kept as offsets over the surface)", true);
            return;
        }
        let Some(f) = self.doc.feature(id) else { return };
        if state_of(f) == Some(s) || state_of(f).is_none() {
            return;
        }
        self.commit_gestures(self.principal());
        self.edit(if s == PatchState::Nurbs { "To NURBS" } else { "To Poly" }, |d| {
            let Some(f) = d.feature_mut(id) else { return };
            if s == PatchState::Nurbs {
                rebind(f);
            }
            if let Some(p) = f.mesh.as_mut().and_then(|m| m.make_mut().patch.as_mut()) {
                p.state = s;
            }
        });
        self.poly_sel = None;
        self.patch_sel = None;
        self.patch_hover = None;
        self.selection = vec![id];
        self.mesh_gen += 1;
        self.set_workspace(if s == PatchState::Nurbs { Workspace::Forme } else { Workspace::Poly });
        self.selection = vec![id];
        if s == PatchState::Poly {
            self.toast("Poly: the mesh is yours (Vertex, Edge, Face modes); back to NURBS it rides on the surface", false);
        } else {
            let off = self.doc.feature(id).and_then(patch_of).map(|p| p.off_surface().iter().filter(|o| **o).count()).unwrap_or(0);
            self.toast(&format!("NURBS: move the contour's points (Vertex mode, 1){}", if off > 0 { format!("; {off} points of the Poly layer ride on it") } else { String::new() }), false);
        }
    }

    /// Vertex mode on a linked surface in its NURBS state: the contour's
    /// points are picked and moved.
    pub fn patch_comp(&self) -> bool {
        self.sel_mode == crate::state::SelMode::Vertex && self.tool == crate::state::Tool::Select && self.linked_target().and_then(|id| self.doc.feature(id)).map(shows_nurbs).unwrap_or(false)
    }

    /// The Carves after it in the history that cut it: boxes and cylinders,
    /// shown, whose box meets its own.
    pub fn patch_solids(&self, id: FeatureId) -> Vec<Solid> {
        let Some(at) = self.doc.features.iter().position(|f| f.id == id) else { return vec![] };
        let Some((lo, hi)) = self.doc.features.get(at).and_then(crate::meshes::world_bounds) else { return vec![] };
        let (lo, hi) = (lo - Vec3::splat(1.0), hi + Vec3::splat(1.0));
        let mut out = vec![];
        for f in &self.doc.features[at + 1..] {
            if !f.visible || f.suspended || f.op != Op::Carve || !f.is_solid() || f.is_mesh() || f.is_path() || f.profile.is_some() {
                continue;
            }
            // its box in the world must meet the surface's
            let cs = f.corners();
            let (blo, bhi) = cs.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), c| (a.min(c.as_vec3()), b.max(c.as_vec3())));
            if blo.cmpgt(hi).any() || bhi.cmplt(lo).any() {
                continue;
            }
            let to_local = DMat4::from_rotation_translation(f.rot, f.origin).inverse().as_mat4();
            let kind = match f.prim {
                Prim::Box { x, y, z } => SolidKind::Box { half: Vec3::new(x as f32, y as f32, z as f32) * 0.5 },
                Prim::Cylinder { radius, height } => SolidKind::Cylinder { r: radius as f32, h: height as f32 },
                _ => continue,
            };
            out.push(Solid { to_local, kind });
        }
        out
    }

    /// The contour's points of the surface worked on, in the world (the
    /// indices are control points', see `CONTOUR`).
    pub fn patch_points(&self) -> Option<(FeatureId, [Vec3; 16])> {
        let id = self.linked_target()?;
        Some((id, cvs_world(self.doc.feature(id)?)?))
    }

    /// The contour's point under the pointer (within 14 points).
    pub fn patch_hover_at(&self, p: [f32; 2]) -> Option<usize> {
        let (_, cv) = self.patch_points()?;
        let v = self.view();
        let mut best: Option<(f32, usize)> = None;
        for k in CONTOUR {
            let Some(s) = v.project(cv[k]) else { continue };
            let d = (s[0] - p[0]).hypot(s[1] - p[1]);
            // a corner wins over a tangent point as near
            let d = if patch::is_corner(k) { d - 2.0 } else { d };
            if d < 14.0 && best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, k));
            }
        }
        best.map(|b| b.1)
    }

    /// A click in Vertex mode on the surface: the point picked (Shift:
    /// added or taken out, Ctrl: taken out); nowhere: none.
    pub fn patch_click(&mut self, p: [f32; 2], shift: bool, ctrl: bool) {
        let Some(id) = self.linked_target() else { return };
        let mut s = match self.patch_sel.clone() {
            Some(s) if s.id == id && (shift || ctrl) => s,
            _ => PatchSel { id, cvs: vec![] },
        };
        if let Some(k) = self.patch_hover_at(p) {
            let has = s.cvs.contains(&k);
            if ctrl || (shift && has) {
                s.cvs.retain(|x| *x != k);
            } else if !has {
                s.cvs.push(k);
            }
        }
        self.patch_sel = Some(s);
    }

    /// A rectangle in Vertex mode: the contour's points inside it.
    pub fn patch_marquee(&mut self, a: [f32; 2], b: [f32; 2], shift: bool, ctrl: bool) {
        let Some((id, cv)) = self.patch_points() else { return };
        let v = self.view();
        let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
        let mut s = match self.patch_sel.clone() {
            Some(s) if s.id == id && (shift || ctrl) => s,
            _ => PatchSel { id, cvs: vec![] },
        };
        for k in CONTOUR {
            let Some(q) = v.project(cv[k]) else { continue };
            if q[0] >= lo[0] && q[0] <= hi[0] && q[1] >= lo[1] && q[1] <= hi[1] {
                if ctrl {
                    s.cvs.retain(|x| *x != k);
                } else if !s.cvs.contains(&k) {
                    s.cvs.push(k);
                }
            }
        }
        self.patch_sel = Some(s);
    }

    /// The picked points, of the surface worked on.
    pub fn patch_picked(&self) -> Vec<usize> {
        match (&self.patch_sel, self.linked_target()) {
            (Some(s), Some(id)) if s.id == id => s.cvs.clone(),
            _ => vec![],
        }
    }

    /// The gizmo's place: the picked points' middle.
    pub fn patch_pivot(&self) -> Option<DVec3> {
        let (_, cv) = self.patch_points()?;
        let k = self.patch_picked();
        if k.is_empty() {
            return None;
        }
        Some((k.iter().map(|i| cv[*i]).sum::<Vec3>() / k.len() as f32).as_dvec3())
    }

    /// A gizmo drag on the picked points begins (moves only).
    pub fn begin_patch_drag(&mut self, h: Handle, p: [f32; 2]) {
        if crate::state::move_mask(h).is_none() {
            return;
        }
        let Some(id) = self.linked_target() else { return };
        let Some(mut f) = self.doc.feature(id).cloned() else { return };
        // scaled or resized since it was bound: bound again where it is
        if bound_off(&f) > 0.01 {
            rebind(&mut f);
        }
        let picked = self.patch_picked();
        if picked.is_empty() {
            return;
        }
        let Some(pivot) = self.pivot() else { return };
        let Some(grab) = self.handle_param(h, pivot, p) else { return };
        // a corner takes its tangent points with it (a Bézier anchor)
        let mut cvs = picked.clone();
        for k in &picked {
            if patch::is_corner(*k) {
                for t in handles(*k) {
                    if !cvs.contains(&t) {
                        cvs.push(t);
                    }
                }
            }
        }
        self.begin_live("Move contour points");
        self.patch_drag = Some(PatchDrag { handle: h, grab, pivot, before: f, cvs });
    }

    pub fn update_patch_drag(&mut self, p: [f32; 2]) {
        let Some(d) = self.patch_drag.clone() else { return };
        let Some(now) = self.handle_param(d.handle, d.pivot, p) else { return };
        let Some(mask) = crate::state::move_mask(d.handle) else { return };
        let mut delta = (now - d.grab) * mask;
        if self.settings.snap && !self.snap_off {
            delta = delta.map(|x| self.snap(x));
        }
        self.gizmo_readout = Some(format!("{:.2} mm", delta.length()));
        // the move in the frame's millimetres, then in its unit box
        let local = (d.before.rot.inverse() * delta).as_vec3() / size(&d.before);
        let mut g = d.before.clone();
        if let Some(pp) = g.mesh.as_mut().and_then(|m| m.make_mut().patch.as_mut()) {
            for k in &d.cvs {
                pp.cv[*k] = (Vec3::from(pp.cv[*k]) + local).to_array();
            }
            pp.fill_inside();
        }
        apply_binding(&mut g);
        let id = g.id;
        self.live(|doc| {
            if let Some(f) = doc.feature_mut(id) {
                *f = g;
            }
        });
        self.mesh_gen += 1;
    }

    pub fn end_patch_drag(&mut self) {
        let Some(d) = self.patch_drag.take() else { return };
        if let Some(f) = self.doc.feature_mut(d.before.id) {
            refit(f);
        }
        self.gizmo_readout = None;
        self.mesh_gen += 1;
        self.end_live();
    }

    /// Moves contour points by a vector (world), one undo step (the
    /// capture harness, the link).
    pub fn move_patch_points(&mut self, id: FeatureId, cvs: &[usize], delta: Vec3) {
        let Some(f) = self.doc.feature(id).cloned() else { return };
        let local = (f.rot.inverse().as_quat() * delta) / size(&f);
        self.edit("Move contour points", |doc| {
            let Some(g) = doc.feature_mut(id) else { return };
            if bound_off(g) > 0.01 {
                rebind(g);
            }
            if let Some(pp) = g.mesh.as_mut().and_then(|m| m.make_mut().patch.as_mut()) {
                for k in cvs {
                    pp.cv[*k] = (Vec3::from(pp.cv[*k]) + local).to_array();
                }
                pp.fill_inside();
            }
            apply_binding(g);
            refit(g);
        });
        self.mesh_gen += 1;
    }
}
