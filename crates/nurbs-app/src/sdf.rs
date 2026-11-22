//! The SDF tab (Romain, 20 nov. 2026, 23 h 47: "reprendre le moteur SDF
//! qu'on avait développé puis stoppé… avec l'UI et outils actuels… la
//! couleur serait le vert"): Forme SDF's engine (`forme-core`: its scene,
//! its compiler, its dual contouring) brought back as a fourth tab.
//!
//! An SDF object is a mesh feature (`MeshKind::Sdf`) whose `MeshData.sdf`
//! holds its shapes (Cube, Cylinder, Sphere, Capsule, Torus) combined in
//! order (Add, Carve, Intersect, each with a blend radius). Its mesh is the
//! surface of that scene, made again at each change: coarse at once (also
//! while a drag is live), fine in a thread when nothing moves. So the
//! object moves, turns, scales, copies, hides, takes materials and sits in
//! the outliner like any mesh; the kernel never sees it.
//!
//! In the SDF tab a click on an SDF object picks the shape under the pointer
//! (`pick_node`, Carves first on their walls), the gizmo then works on that
//! shape: W moves it, E turns it, R its cage sizes it; the blend follows
//! live. The rail adds shapes (into the picked object, else a new one) and
//! shows the picked shape's settings.

use crate::meshes::size;
use crate::state::{AppState, Handle, Tool};
use forme_core::{CombineMode, NodeId, NodeKind, NodeSpec, PrimKind, Scene, Shape};
use glam::{DQuat, DVec3, Mat4, Quat, Vec3};
use nurbs_kernel::mesh::{MeshKind, SdfModel};
use nurbs_kernel::{Feature, FeatureId, Prim};
use std::sync::mpsc::{channel, Receiver};

/// The shapes the tab offers (Romain, 10 nov.: Cube, Cylinder + Sphere,
/// Capsule, Torus; the engine keeps the others).
pub const SDF_PRIMS: [PrimKind; 5] = [PrimKind::Box, PrimKind::Cylinder, PrimKind::Sphere, PrimKind::Capsule, PrimKind::Torus];

/// The operations the tab offers for a shape.
pub const SDF_MODES: [CombineMode; 3] = [CombineMode::Add, CombineMode::Carve, CombineMode::Intersect];

pub fn prim_label(k: PrimKind) -> &'static str {
    match k {
        PrimKind::Box => "Cube",
        PrimKind::Cylinder => "Cylinder",
        PrimKind::Sphere => "Sphere",
        PrimKind::Capsule => "Capsule",
        PrimKind::Torus => "Torus",
        _ => "Shape",
    }
}

pub fn prim_key(k: PrimKind) -> &'static str {
    match k {
        PrimKind::Box => "B",
        PrimKind::Cylinder => "C",
        PrimKind::Sphere => "O",
        PrimKind::Capsule => "P",
        PrimKind::Torus => "T",
        _ => "",
    }
}

pub fn prim_icon(k: PrimKind) -> crate::icons::Icon {
    use crate::icons::Icon;
    match k {
        PrimKind::Box => Icon::Box,
        PrimKind::Cylinder => Icon::Cylinder,
        PrimKind::Sphere => Icon::SdfSphere,
        PrimKind::Capsule => Icon::SdfCapsule,
        _ => Icon::SdfTorus,
    }
}

/// The tab's settings for the next shapes.
#[derive(Clone, Debug)]
pub struct SdfSettings {
    pub mode: CombineMode,
    /// The blend radius of a new shape with what is before it (mm).
    pub blend: f32,
}

impl Default for SdfSettings {
    fn default() -> Self {
        SdfSettings { mode: CombineMode::Add, blend: 2.0 }
    }
}

/// A gizmo drag on a picked shape.
#[derive(Clone, Debug)]
pub struct SdfDrag {
    pub handle: Handle,
    pub id: FeatureId,
    pub node: NodeId,
    pub grab: DVec3,
    pub pivot: DVec3,
    /// The object and the shape's world frame when the drag began.
    pub before: Feature,
    pub cage: Option<crate::cage::CageBox>,
}

/// A fine meshing under way in a thread.
pub struct SdfJob {
    pub id: FeatureId,
    pub key: u64,
    pub rx: Receiver<Meshed>,
}

/// A scene meshed: the model it was made from (its scale taken in), the
/// points in the feature's frame, the triangles, and each triangle's
/// material as the engine gives it (the shape's own, a Carve's on the walls
/// it cuts, blended ones by the nearer; 0: the object's).
pub struct Meshed {
    pub model: SdfModel,
    pub pts: Vec<Vec3>,
    pub faces: Vec<Vec<u32>>,
    pub mats: Vec<u32>,
}

/// What the mesh was made from: the scene and the box's sizes.
pub fn key_of(f: &Feature) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let md = f.mesh.as_ref()?.sdf.as_ref()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(&md.scene).unwrap_or_default().hash(&mut h);
    md.marker.hash(&mut h);
    for v in size(f).to_array() {
        v.to_bits().hash(&mut h);
    }
    Some(h.finish())
}

/// The cell for a scene this big: coarse while it moves, fine after.
fn cell_for(lo: Vec3, hi: Vec3, fine: bool) -> f32 {
    let d = (hi - lo).length().max(1.0);
    if fine {
        (d / 200.0).max(0.1)
    } else {
        (d / 80.0).max(0.25)
    }
}

/// The scene meshed (its box's scale since the last meshing taken into
/// its shapes first).
pub fn mesh_model(model: &SdfModel, now: Vec3, fine: bool) -> Meshed {
    let mut model = model.clone();
    model.take_scale(now);
    // the shapes past the history marker left out
    let built = model.built_scene();
    let (lo, hi) = built.bounds();
    if !(lo.x <= hi.x) {
        return Meshed { model, pts: vec![], faces: vec![], mats: vec![] };
    }
    let prog = forme_core::compile(&built);
    let m = forme_core::mesh::dual_contour(&prog, lo, hi, cell_for(lo, hi, fine));
    let mats = m
        .indices
        .chunks(3)
        .map(|t| {
            let c = t.iter().map(|i| m.positions[*i as usize]).fold(Vec3::ZERO, |a, b| a + b) / 3.0;
            prog.eval(c).1
        })
        .collect();
    let pts = m.positions.iter().map(|p| model.to_frame(*p)).collect();
    let faces = m.indices.chunks(3).map(|t| t.to_vec()).collect();
    Meshed { model, pts, faces, mats }
}

/// A meshing put into the feature: the unit box fitted to the surface, the
/// frame's base moved there, the shapes kept where they were.
pub fn apply(f: &mut Feature, mut m: Meshed) -> Vec<u32> {
    let mats = std::mem::take(&mut m.mats);
    let now = size(f);
    let Some(mesh) = f.mesh.as_mut() else { return mats };
    let md = mesh.make_mut();
    let mut model = m.model;
    if m.pts.is_empty() {
        // nothing to show (all carved away): the box kept, empty
        md.verts.clear();
        md.faces.clear();
        model.size = now.to_array();
        md.sdf = Some(model);
        return mats;
    }
    md.faces = m.faces;
    let (s2, base) = md.store(&m.pts, now);
    model.offset = (Vec3::from(model.offset) - base).to_array();
    model.size = s2.to_array();
    md.sdf = Some(model);
    f.origin += f.rot * base.as_dvec3();
    f.prim = Prim::Box { x: s2.x as f64, y: s2.y as f64, z: s2.z as f64 };
    mats
}

/// The feature's frame with its box's stretch since the last meshing, from
/// the scene to the world.
pub fn frame_matrix(f: &Feature) -> Mat4 {
    let Some(md) = f.mesh.as_ref().and_then(|m| m.sdf.as_ref()) else { return Mat4::IDENTITY };
    let was = Vec3::from(md.size);
    let stretch = if was.min_element() > 1e-4 { size(f) / was } else { Vec3::ONE };
    Mat4::from_scale_rotation_translation(stretch, f.rot.as_quat(), f.origin.as_vec3()) * Mat4::from_translation(Vec3::from(md.offset))
}

/// A shape's frame in the world (its centre, its turn).
pub fn node_world(f: &Feature, scene: &Scene, id: NodeId) -> Mat4 {
    frame_matrix(f) * scene.world_matrix(id)
}

pub fn scene_of(f: &Feature) -> Option<&Scene> {
    f.mesh.as_ref()?.sdf.as_ref().map(|m| &m.scene)
}

pub fn is_sdf(f: &Feature) -> bool {
    f.mesh.as_ref().map(|m| m.kind == MeshKind::Sdf).unwrap_or(false)
}

/// The shapes of a scene in order (no groups).
pub fn shapes(scene: &Scene) -> Vec<NodeId> {
    nurbs_kernel::mesh::sdf_shapes(scene)
}

/// The object's model (its shapes, its marker).
pub fn model_of(f: &Feature) -> Option<&SdfModel> {
    f.mesh.as_ref()?.sdf.as_ref()
}

/// The shapes built (before the object's history marker).
pub fn built_shapes(f: &Feature) -> Vec<NodeId> {
    let Some(md) = model_of(f) else { return vec![] };
    let all = md.shapes();
    let n = md.marker.unwrap_or(all.len()).min(all.len());
    all[..n].to_vec()
}

/// A shape's outline as lines in the world: its box's 12 edges, and for a
/// round shape its rings.
pub fn node_lines(f: &Feature, scene: &Scene, id: NodeId) -> Vec<Vec<[f32; 3]>> {
    let Some(n) = scene.node(id) else { return vec![] };
    let Some(sh) = n.shape() else { return vec![] };
    let m = node_world(f, scene, id);
    let p = |v: Vec3| m.transform_point3(v).to_array();
    let ring = |r: f32, y: f32| -> Vec<[f32; 3]> { (0..=48).map(|i| i as f32 / 48.0 * std::f32::consts::TAU).map(|a| p(Vec3::new(r * a.cos(), y, r * a.sin()))).collect() };
    let s = sh.size;
    match sh.kind {
        PrimKind::Box => {
            let c = |i: usize| Vec3::new(if i & 1 == 0 { -s.x } else { s.x }, if i & 2 == 0 { -s.y } else { s.y }, if i & 4 == 0 { -s.z } else { s.z });
            let mut out = vec![];
            for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
                out.push(vec![p(c(a)), p(c(b))]);
            }
            out
        }
        PrimKind::Cylinder => {
            let mut out = vec![ring(s.x, s.y), ring(s.x, -s.y)];
            for a in [0.0f32, 0.5, 1.0, 1.5] {
                let a = a * std::f32::consts::PI;
                out.push(vec![p(Vec3::new(s.x * a.cos(), s.y, s.x * a.sin())), p(Vec3::new(s.x * a.cos(), -s.y, s.x * a.sin()))]);
            }
            out
        }
        PrimKind::Sphere => {
            let r = s.x;
            let mut out = vec![ring(r, 0.0)];
            out.push((0..=48).map(|i| i as f32 / 48.0 * std::f32::consts::TAU).map(|a| p(Vec3::new(r * a.cos(), r * a.sin(), 0.0))).collect());
            out.push((0..=48).map(|i| i as f32 / 48.0 * std::f32::consts::TAU).map(|a| p(Vec3::new(0.0, r * a.sin(), r * a.cos()))).collect());
            out
        }
        PrimKind::Capsule => {
            let (r, h) = (s.x, s.y);
            let mut out = vec![ring(r, h), ring(r, -h)];
            for k in [0.0f32, 0.5] {
                let a = k * std::f32::consts::PI;
                let (cx, cz) = (a.cos(), a.sin());
                let mut l = vec![];
                for i in 0..=24 {
                    let t = i as f32 / 24.0 * std::f32::consts::PI;
                    l.push(p(Vec3::new(r * t.cos() * cx, h + r * t.sin(), r * t.cos() * cz)));
                }
                for i in 0..=24 {
                    let t = std::f32::consts::PI + i as f32 / 24.0 * std::f32::consts::PI;
                    l.push(p(Vec3::new(r * t.cos() * cx, -h + r * t.sin(), r * t.cos() * cz)));
                }
                l.push(l[0]);
                out.push(l);
            }
            out
        }
        PrimKind::Torus => vec![ring(s.x + s.z, 0.0), ring(s.x - s.z, 0.0), ring(s.x, s.z), ring(s.x, -s.z)],
        _ => {
            let (lo, hi) = sh.local_bounds();
            let c = |i: usize| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
            [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)].iter().map(|(a, b)| vec![p(c(*a)), p(c(*b))]).collect()
        }
    }
}

/// A shape's outline as the eye sees it (the Studio interface, the mock-up
/// D3: a shape drawn by its silhouette, not by its rings): a sphere's
/// circle seen from the eye, a capsule's two side lines and the ends past
/// them, a cylinder's rims and its two side lines; a box and a torus keep
/// their lines. `eye` in the world.
pub fn node_silhouette(f: &Feature, scene: &Scene, id: NodeId, eye: Vec3) -> Vec<Vec<[f32; 3]>> {
    let Some(n) = scene.node(id) else { return vec![] };
    let Some(sh) = n.shape() else { return vec![] };
    let m = node_world(f, scene, id);
    let e = m.inverse().transform_point3(eye);
    let p = |v: Vec3| m.transform_point3(v).to_array();
    let s = sh.size;
    const N: usize = 72;
    // a sphere's circle seen from `e` (centre c, radius r): on the plane
    // square to the eye's line, where the lines from the eye touch it
    let circle = |c: Vec3, r: f32| -> Option<(Vec3, f32, Vec3, Vec3)> {
        let d = e - c;
        let l = d.length();
        if l <= r * 1.001 {
            return None;
        }
        let w = d / l;
        let centre = c + w * (r * r / l);
        let rad = r * (1.0 - (r / l).powi(2)).sqrt();
        let a = w.any_orthonormal_vector();
        Some((centre, rad, a, w.cross(a)))
    };
    let ring_pts = |(c, r, a, b): (Vec3, f32, Vec3, Vec3)| -> Vec<Vec3> { (0..=N).map(|i| i as f32 / N as f32 * std::f32::consts::TAU).map(|t| c + (a * t.cos() + b * t.sin()) * r).collect() };
    // the two lines along Y touching a cylinder of radius r round it, as
    // seen from the eye (exact: the eye's line to the axis is the same
    // across Y for every height)
    let sides = |r: f32| -> Option<[Vec3; 2]> {
        let h = Vec3::new(e.x, 0.0, e.z);
        let l = h.length();
        if l <= r * 1.001 {
            return None;
        }
        let w = h / l;
        let sd = Vec3::Y.cross(w);
        let along = r * r / l;
        let across = r * (1.0 - (r / l).powi(2)).sqrt();
        Some([w * along + sd * across, w * along - sd * across])
    };
    match sh.kind {
        PrimKind::Sphere => circle(Vec3::ZERO, s.x).map(|c| vec![ring_pts(c).into_iter().map(p).collect()]).unwrap_or_default(),
        PrimKind::Cylinder => {
            let ring = |y: f32| -> Vec<[f32; 3]> { (0..=N).map(|i| i as f32 / N as f32 * std::f32::consts::TAU).map(|a| p(Vec3::new(s.x * a.cos(), y, s.x * a.sin()))).collect() };
            let mut out = vec![ring(s.y), ring(-s.y)];
            if let Some(q) = sides(s.x) {
                for k in q {
                    out.push(vec![p(k + Vec3::Y * s.y), p(k - Vec3::Y * s.y)]);
                }
            }
            out
        }
        PrimKind::Capsule => {
            let (r, h) = (s.x, s.y);
            let mut out = vec![];
            if let Some(q) = sides(r) {
                for k in q {
                    out.push(vec![p(k + Vec3::Y * h), p(k - Vec3::Y * h)]);
                }
            }
            // each end's circle, its part past the side lines' ends
            for y in [h, -h] {
                let Some(c) = circle(Vec3::Y * y, r) else { continue };
                let pts = ring_pts(c);
                let past = |v: &Vec3| if y > 0.0 { v.y >= y - 1e-3 } else { v.y <= y + 1e-3 };
                // the run of points past the end, taken round the circle from
                // a point that is not
                let Some(start) = pts.iter().position(|v| !past(v)) else {
                    out.push(pts.into_iter().map(p).collect());
                    continue;
                };
                let mut run: Vec<[f32; 3]> = vec![];
                for i in 0..N {
                    let v = pts[(start + i) % N];
                    if past(&v) {
                        run.push(p(v));
                    } else if run.len() > 1 {
                        out.push(std::mem::take(&mut run));
                    } else {
                        run.clear();
                    }
                }
                if run.len() > 1 {
                    out.push(run);
                }
            }
            if out.is_empty() {
                return node_lines(f, scene, id);
            }
            out
        }
        _ => node_lines(f, scene, id),
    }
}

/// A new shape of this kind, about 20 mm (sizes are half sizes).
fn new_shape(k: PrimKind) -> Shape {
    let mut s = Shape::new(k);
    match k {
        PrimKind::Box => s.size = Vec3::splat(10.0),
        PrimKind::Cylinder => s.size = Vec3::new(8.0, 10.0, 0.0),
        PrimKind::Sphere => s.size = Vec3::splat(10.0),
        PrimKind::Capsule => s.size = Vec3::new(5.0, 6.0, 0.0),
        PrimKind::Torus => s.size = Vec3::new(8.0, 0.0, 3.0),
        _ => {}
    }
    s
}

/// A shape's half height along its Y (to stand it on something).
fn half_height(s: &Shape) -> f32 {
    s.local_half_extents().y
}

impl AppState {
    /// The SDF tab with a shape picked: the gizmo works on it.
    pub fn sdf_comp(&self) -> bool {
        self.workspace == crate::workspace::Workspace::Sdf
            && self.sdf_sel.map(|(id, n)| self.doc.feature(id).and_then(scene_of).map(|s| s.node(n).is_some()).unwrap_or(false)).unwrap_or(false)
    }

    /// The picked shape and its object.
    pub fn sdf_picked(&self) -> Option<(&Feature, NodeId)> {
        let (id, n) = self.sdf_sel?;
        let f = self.doc.feature(id)?;
        scene_of(f)?.node(n)?;
        Some((f, n))
    }

    /// The SDF object worked on: the picked shape's, else the one selected.
    pub fn sdf_target(&self) -> Option<FeatureId> {
        if let Some((f, _)) = self.sdf_picked() {
            return Some(f.id);
        }
        match self.selection.as_slice() {
            [id] if self.doc.feature(*id).map(is_sdf).unwrap_or(false) => Some(*id),
            _ => None,
        }
    }

    /// The picked shape's centre in the world (the gizmo's place).
    pub fn sdf_pivot(&self) -> Option<DVec3> {
        let (f, n) = self.sdf_picked()?;
        let m = node_world(f, scene_of(f)?, n);
        Some(m.transform_point3(Vec3::ZERO).as_dvec3())
    }

    /// The picked shape's cage: its box in its own frame.
    pub fn sdf_cage(&self) -> Option<crate::cage::CageBox> {
        let (f, n) = self.sdf_picked()?;
        let scene = scene_of(f)?;
        let sh = scene.node(n)?.shape()?;
        let m = node_world(f, scene, n);
        let (_, rot, _) = m.to_scale_rotation_translation();
        let frame = rot.as_dquat();
        let (lo, hi) = sh.local_bounds();
        let back = frame.inverse();
        let mut l = DVec3::splat(f64::MAX);
        let mut h = DVec3::splat(f64::MIN);
        for i in 0..8 {
            let c = Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
            let q = back * m.transform_point3(c).as_dvec3();
            l = l.min(q);
            h = h.max(q);
        }
        Some(crate::cage::CageBox { frame, lo: l, hi: h })
    }

    /// The SDF object and shape under the pointer (in front of the part
    /// and the other meshes).
    pub fn sdf_pick_at(&self, p: [f32; 2]) -> Option<(FeatureId, NodeId)> {
        let hit = self.pick_mesh(p, None)?;
        if let Some(k) = self.pick_at(p) {
            if k.t < hit.t {
                return None;
            }
        }
        let f = self.doc.feature(hit.id)?;
        // the shapes built (those past the marker or turned off are not seen)
        let scene = model_of(f)?.built_scene();
        let q = frame_matrix(f).inverse().transform_point3(hit.point);
        let d = (size(f).length() * 0.04).clamp(0.3, 4.0);
        let n = forme_core::pick_node(&scene, q, d)?;
        Some((f.id, n))
    }

    /// A click in the SDF tab: the shape under the pointer, else the
    /// element as in Object mode, else nothing.
    pub fn sdf_click(&mut self, p: [f32; 2]) {
        if let Some((id, n)) = self.sdf_pick_at(p) {
            self.sdf_sel = Some((id, n));
            self.selection = vec![id];
            return;
        }
        self.sdf_sel = None;
        self.selection = self.pick_object(p).map(|id| vec![id]).unwrap_or_default();
    }

    /// A shape of this kind added: into the SDF object worked on (on top of
    /// the picked shape, or at its middle for a Carve or Intersect), else a
    /// new SDF object on the view's centre, standing on the ground.
    pub fn sdf_add(&mut self, k: PrimKind) {
        self.commit_gestures(self.principal());
        self.tool = Tool::Select;
        let mode = self.sdf.mode;
        let blend = self.sdf.blend;
        let sh = new_shape(k);
        let name = prim_label(k);
        if let Some(id) = self.sdf_target() {
            let picked = self.sdf_sel.filter(|(f, _)| *f == id).map(|(_, n)| n);
            let mut made = 0;
            self.edit(&format!("Add {name}"), |d| {
                let Some(f) = d.feature_mut(id) else { return };
                let Some(md) = f.mesh.as_mut().and_then(|m| m.make_mut().sdf.as_mut()) else { return };
                let s = &mut md.scene;
                // where: on the picked shape's top (Add), in its middle (the others)
                let pos = match picked.and_then(|n| s.node(n).map(|nd| (nd, s.world_matrix(n)))) {
                    Some((nd, w)) => {
                        let top = nd.shape().map(half_height).unwrap_or(0.0);
                        let c = w.transform_point3(Vec3::ZERO);
                        if mode == CombineMode::Add {
                            c + Vec3::Y * (top + half_height(&sh) * 0.6)
                        } else {
                            c + Vec3::Y * top
                        }
                    }
                    None => {
                        let (lo, hi) = s.bounds();
                        if lo.x <= hi.x {
                            Vec3::new((lo.x + hi.x) * 0.5, hi.y, (lo.z + hi.z) * 0.5)
                        } else {
                            Vec3::Y * half_height(&sh)
                        }
                    }
                };
                let label = d_name(s, name);
                let spec = NodeSpec::shape(&label, sh.clone()).at(pos).mode(mode).blend(if mode == CombineMode::Add || mode == CombineMode::Carve { blend } else { blend.min(1.0) });
                // the history marker back: the new shape goes in at the
                // marker, before those left out (as an operation on a NURBS
                // shape, Fusion's timeline)
                match md.marker {
                    Some(m) => {
                        let root = s.root;
                        let at = s.node(root).map(|r| r.children().len()).unwrap_or(0);
                        let shapes = nurbs_kernel::mesh::sdf_shapes(s);
                        let index = shapes.get(m).and_then(|x| s.node(root).and_then(|r| r.children().iter().position(|c| c == x))).unwrap_or(at);
                        made = s.insert(root, spec, Some(index));
                        md.marker = Some(m + 1);
                    }
                    None => made = s.add(s.root, spec),
                }
            });
            if made != 0 {
                self.sdf_sel = Some((id, made));
                self.selection = vec![id];
                self.set_gizmo(crate::state::GizmoMode::Move);
            }
            return;
        }
        // a new object, its first shape an Add
        let empty = !self.doc.features.iter().any(|f| f.visible && (f.is_solid() || f.is_mesh()));
        let at = self.camera.target.as_dvec3();
        let at = DVec3::new(at.x, 0.0, at.z);
        let mut fid = 0;
        let mut node = 0;
        self.edit(&format!("New SDF {}", name.to_lowercase()), |d| {
            fid = d.add_mesh(MeshKind::Sdf, &[], vec![], at, DQuat::IDENTITY, "SDF object");
            if let Some(f) = d.feature_mut(fid) {
                // the frame's base at `at` (add_mesh with no points puts it there)
                f.origin = at;
                if let Some(m) = f.mesh.as_mut() {
                    let mut model = SdfModel::new();
                    node = model.scene.add(model.scene.root, NodeSpec::shape(name, sh.clone()).at(Vec3::Y * half_height(&sh)));
                    m.make_mut().sdf = Some(model);
                }
            }
        });
        self.sdf_sel = Some((fid, node));
        self.selection = vec![fid];
        self.sdf_tick();
        self.set_gizmo(crate::state::GizmoMode::Move);
        // the first thing in the scene: the view comes to it, the shape about
        // a third of the view (22 nov., Romain's iPad: a 12 mm bean lost in
        // a view 160 mm away)
        if empty {
            let c = at.as_vec3() + glam::Vec3::Y * half_height(&sh);
            self.fly_frame(c - glam::Vec3::splat(22.0), c + glam::Vec3::splat(22.0));
        }
    }

    /// Changes the picked shape (its node), one undo step.
    pub fn sdf_edit_node(&mut self, label: &str, f: impl FnOnce(&mut forme_core::Node)) {
        let Some((id, n)) = self.sdf_sel else { return };
        self.edit(label, |d| {
            if let Some(node) = d.feature_mut(id).and_then(|ft| ft.mesh.as_mut()).and_then(|m| m.make_mut().sdf.as_mut()).and_then(|md| md.scene.node_mut(n)) {
                f(node);
            }
        });
    }

    /// The same as a live edit (a slider): one undo step when it ends.
    pub fn sdf_live_node(&mut self, label: &str, f: impl FnOnce(&mut forme_core::Node)) {
        let Some((id, n)) = self.sdf_sel else { return };
        self.begin_live(label);
        self.live(|d| {
            if let Some(node) = d.feature_mut(id).and_then(|ft| ft.mesh.as_mut()).and_then(|m| m.make_mut().sdf.as_mut()).and_then(|md| md.scene.node_mut(n)) {
                f(node);
            }
        });
    }

    /// Del on a picked shape: it leaves its object (the last one: the object).
    pub fn sdf_delete(&mut self) -> bool {
        let Some((f, n)) = self.sdf_picked() else { return false };
        let id = f.id;
        let left = scene_of(f).map(|s| shapes(s).len()).unwrap_or(0);
        if left <= 1 {
            self.sdf_sel = None;
            self.selection = vec![id];
            return false;
        }
        self.delete_item(crate::common::Item::Shape(id, n));
        self.sdf_sel = None;
        true
    }

    /// Ctrl + D on a picked shape: a copy beside it, picked.
    pub fn sdf_duplicate(&mut self) -> bool {
        let Some((id, n)) = self.sdf_sel else { return false };
        if !self.sdf_comp() {
            return false;
        }
        let mut made = None;
        self.edit("Duplicate shape", |d| {
            if let Some(md) = d.feature_mut(id).and_then(|ft| ft.mesh.as_mut()).and_then(|m| m.make_mut().sdf.as_mut()) {
                let at = md.shapes().iter().position(|x| *x == n);
                made = md.scene.duplicate(n);
                // the copy right after it: the marker stays after the same shapes
                if let (Some(m), Some(at)) = (md.marker, at) {
                    if at < m {
                        md.marker = Some(m + 1);
                    }
                }
            }
        });
        if let Some(m) = made {
            self.sdf_sel = Some((id, m));
            self.set_gizmo(crate::state::GizmoMode::Move);
        }
        true
    }

    // ------------------------------------------------------- gizmo drags

    pub fn begin_sdf_drag(&mut self, h: Handle, p: [f32; 2]) {
        let Some((f, n)) = self.sdf_picked() else { return };
        let before = f.clone();
        let cage = if matches!(h, Handle::Cage(_)) { self.sdf_cage() } else { None };
        let Some(pivot) = (match h {
            Handle::Cage(g) => cage.as_ref().map(|c| c.point(g)),
            _ => self.pivot(),
        }) else {
            return;
        };
        let Some(grab) = self.handle_param(h, pivot, p) else { return };
        self.begin_live(match h {
            Handle::Rot(_) => "Rotate shape",
            Handle::Cage(_) => "Size shape",
            _ => "Move shape",
        });
        self.sdf_drag = Some(SdfDrag { handle: h, id: before.id, node: n, grab, pivot, before, cage });
    }

    pub fn update_sdf_drag(&mut self, p: [f32; 2]) {
        let Some(d) = self.sdf_drag.clone() else { return };
        let Some(now) = self.handle_param(d.handle, d.pivot, p) else { return };
        let Some(scene0) = scene_of(&d.before).cloned() else { return };
        let Some(node0) = scene0.node(d.node).cloned() else { return };
        // the scene's parent frame of the shape, in the world
        let parent = frame_matrix(&d.before) * scene0.parent_matrix(d.node);
        let inv = parent.inverse();
        let w0 = node_world(&d.before, &scene0, d.node);
        let (_, wrot, wpos) = w0.to_scale_rotation_translation();
        let mut node = node0.clone();
        match d.handle {
            Handle::Rot(a) => {
                let ax = DVec3::AXES[a];
                let (u, w) = (d.grab - d.pivot, now - d.pivot);
                let (u, w) = (u - ax * u.dot(ax), w - ax * w.dot(ax));
                let mut ang = u.cross(w).dot(ax).atan2(u.dot(w));
                if self.settings.snap {
                    let st = 15f64.to_radians();
                    ang = (ang / st).round() * st;
                }
                self.gizmo_readout = Some(format!("{:.0}°", ang.to_degrees()));
                let q = Quat::from_axis_angle(ax.as_vec3(), ang as f32);
                let (_, prot, _) = parent.to_scale_rotation_translation();
                node.transform.rot = (prot.inverse() * q * wrot).normalize();
            }
            Handle::Cage(g) => {
                let Some(c) = d.cage else { return };
                let Some((k, about)) = self.cage_scale(&c, g, p, &[]) else { return };
                let kf = k.as_vec3();
                if let NodeKind::Shape(sh) = &mut node.kind {
                    scale_shape(sh, kf);
                }
                // the centre moves with the side across from the grip kept
                let c0 = wpos.as_dvec3();
                let local = c.frame.inverse() * (c0 - about);
                let c1 = about + c.frame * (local * k);
                node.transform.pos = inv.transform_point3(c1.as_vec3());
            }
            h => {
                let Some(mask) = crate::state::move_mask(h) else { return };
                let mut delta = (now - d.grab) * mask;
                if self.settings.snap && !self.snap_off {
                    delta = delta.map(|x| self.snap(x));
                }
                // the pointer on a point of interest (the part's corners and
                // centres, the other shapes' centres, tops and bases): the
                // shape's centre lined up with it along what the handle moves
                let own = wpos.as_dvec3();
                let h0 = node0.shape().map(|s| s.local_half_extents().y).unwrap_or(0.0);
                let mine = [Vec3::ZERO, Vec3::Y * h0, -Vec3::Y * h0].map(|q| w0.transform_point3(q).as_dvec3());
                let caught = self.pick_snap(p, &[]).filter(|s| mine.iter().all(|m| s.pos.distance(*m) > 1e-3)).map(|s| (s.pos, s.kind));
                if let Some((pos, kind)) = caught {
                    delta = (pos - own) * mask;
                    self.gizmo_readout = Some(format!("{} · {} mm", kind.label(), crate::widgets::fmt_mm(delta.length() as f32)));
                } else {
                    self.gizmo_readout = Some(format!("{} mm", crate::widgets::fmt_mm(delta.length() as f32)));
                }
                node.transform.pos = inv.transform_point3(wpos + delta.as_vec3());
            }
        }
        let (id, nid) = (d.id, d.node);
        self.live(|doc| {
            if let Some(n) = doc.feature_mut(id).and_then(|ft| ft.mesh.as_mut()).and_then(|m| m.make_mut().sdf.as_mut()).and_then(|md| md.scene.node_mut(nid)) {
                *n = node;
            }
        });
        // the surface follows at once (coarse)
        self.sdf_tick();
    }

    pub fn end_sdf_drag(&mut self) {
        if self.sdf_drag.take().is_none() {
            return;
        }
        self.gizmo_readout = None;
        self.end_live();
        self.mesh_gen += 1;
    }

    // ------------------------------------------------------- meshing

    /// Each SDF object's mesh kept in step with its shapes: coarse at once
    /// when they changed, fine in a thread when nothing moves.
    pub fn sdf_tick(&mut self) {
        let live = self.is_live() || self.sdf_drag.is_some();
        // the fine meshings done
        let mut done = vec![];
        self.sdf_jobs.retain(|j| match j.rx.try_recv() {
            Ok(m) => {
                done.push((j.id, j.key, m));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
        for (id, key, m) in done {
            let Some(f) = self.doc.feature_mut(id) else { continue };
            if key_of(f) != Some(key) {
                continue;
            }
            let mats = apply(f, m);
            let k = key_of(f).unwrap_or(0);
            self.sdf_meshed.insert(id, (k, true));
            self.sdf_mats.insert(id, mats);
            self.mesh_gen += 1;
        }
        let ids: Vec<FeatureId> = self.doc.features.iter().filter(|f| is_sdf(f) && f.mesh.as_ref().map(|m| m.sdf.is_some()).unwrap_or(false)).map(|f| f.id).collect();
        for id in ids {
            let Some(f) = self.doc.feature(id) else { continue };
            let Some(key) = key_of(f) else { continue };
            let state = self.sdf_meshed.get(&id).copied();
            if state.map(|s| s.0) != Some(key) {
                // changed (or never meshed here): coarse now
                let m = mesh_model(f.mesh.as_ref().unwrap().sdf.as_ref().unwrap(), size(f), false);
                let f = self.doc.feature_mut(id).unwrap();
                let mats = apply(f, m);
                let k = key_of(f).unwrap_or(0);
                self.sdf_meshed.insert(id, (k, false));
                self.sdf_mats.insert(id, mats);
                self.mesh_gen += 1;
                continue;
            }
            if live || state.map(|s| s.1).unwrap_or(false) || self.sdf_jobs.iter().any(|j| j.id == id && j.key == key) {
                continue;
            }
            let model = f.mesh.as_ref().unwrap().sdf.clone().unwrap();
            let now = size(f);
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let _ = tx.send(mesh_model(&model, now, true));
            });
            self.sdf_jobs.push(SdfJob { id, key, rx });
        }
        self.sdf_meshed.retain(|id, _| self.doc.feature(*id).is_some());
        let alive: std::collections::HashSet<FeatureId> = self.sdf_meshed.keys().copied().collect();
        self.sdf_mats.retain(|id, _| alive.contains(id));
        if self.sdf_sel.is_some() && !self.sdf_comp() && self.workspace == crate::workspace::Workspace::Sdf {
            self.sdf_sel = None;
        }
    }

    /// Waits for the fine meshings under way (the capture harness).
    pub fn sdf_settle(&mut self) {
        for _ in 0..400 {
            self.sdf_tick();
            if self.sdf_jobs.is_empty() && self.doc.features.iter().filter(|f| is_sdf(f)).all(|f| self.sdf_meshed.get(&f.id).map(|s| s.1 && Some(s.0) == key_of(f)).unwrap_or(false)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

/// A shape's sizes times `k` along its own axes.
pub fn scale_shape(sh: &mut Shape, k: Vec3) {
    let across = (k.x * k.z).sqrt();
    let mean = (k.x * k.y * k.z).cbrt();
    match sh.kind {
        PrimKind::Box => sh.size *= k,
        PrimKind::Sphere => sh.size *= mean,
        PrimKind::Cylinder | PrimKind::Capsule | PrimKind::Cone | PrimKind::Tube | PrimKind::Prism => {
            sh.size = Vec3::new(sh.size.x * across, sh.size.y * k.y, sh.size.z * across);
        }
        PrimKind::Torus => {
            sh.size = Vec3::new(sh.size.x * across, sh.size.y, sh.size.z * k.y);
        }
        _ => sh.size *= mean,
    }
}

/// A fresh name in the scene: "Cube", "Cube 2"…
fn d_name(s: &Scene, base: &str) -> String {
    let taken = |n: &str| s.nodes.values().any(|x| x.name == n);
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use nurbs_kernel::client::Kernel;
    use nurbs_kernel::Doc;

    fn app() -> AppState {
        let mut st = AppState::new(Doc::default(), "t", Kernel::start(None));
        st.set_workspace(Workspace::Sdf);
        st
    }

    fn object_box(st: &AppState, id: FeatureId) -> (Vec3, Vec3) {
        crate::meshes::world_bounds(st.doc.feature(id).unwrap()).expect("meshed")
    }

    /// Every shape in every operation: added, meshed, the object's box sane.
    #[test]
    fn every_shape_in_every_mode() {
        for k in SDF_PRIMS {
            for mode in SDF_MODES {
                let mut st = app();
                st.sdf_add(PrimKind::Box);
                let id = st.sdf_target().unwrap();
                st.sdf.mode = mode;
                st.sdf_add(k);
                st.sdf_tick();
                let f = st.doc.feature(id).unwrap();
                let n = f.mesh.as_ref().unwrap().faces.len();
                let what = format!("{} {}", prim_label(k), mode.label());
                assert!(n > 50, "{what}: {n} triangles");
                let (lo, hi) = object_box(&st, id);
                let d = hi - lo;
                assert!(d.min_element() > 1.0 && d.max_element() < 80.0, "{what}: box {d:?}");
                // standing on the ground (the cube's base at y 0; an Intersect
                // keeps the common part only, higher)
                assert!(mode == forme_core::CombineMode::Intersect || lo.y.abs() < 1.0, "{what}: base at {}", lo.y);
            }
        }
    }

    /// A Carve through the cube leaves a hole: the ray down its axis
    /// reaches the ground.
    #[test]
    fn a_carve_makes_a_hole() {
        let mut st = app();
        st.sdf_add(PrimKind::Box);
        let id = st.sdf_target().unwrap();
        st.sdf.mode = forme_core::CombineMode::Carve;
        st.sdf.blend = 0.0;
        st.sdf_sel = None;
        st.sdf_add(PrimKind::Cylinder);
        st.sdf_edit_node("Size", |n| {
            if let Some(s) = n.shape_mut() {
                s.size = Vec3::new(3.0, 30.0, 0.0);
            }
        });
        let f = st.doc.feature(id).unwrap();
        let prog = forme_core::compile(&model_of(f).unwrap().built_scene());
        let cube_c = model_of(f).unwrap().scene.node(shapes(scene_of(f).unwrap())[0]).unwrap().transform.pos;
        assert!(prog.dist(cube_c) > 0.0, "the cube's centre is carved");
        assert!(prog.dist(cube_c + Vec3::new(7.0, 0.0, 0.0)) < 0.0, "the rest is solid");
    }

    /// Undo and redo go through the shapes; the mesh follows.
    #[test]
    fn undo_redo_follow() {
        let mut st = app();
        st.sdf_add(PrimKind::Box);
        let id = st.sdf_target().unwrap();
        st.sdf_tick();
        let one = object_box(&st, id);
        st.sdf_add(PrimKind::Sphere);
        st.sdf_tick();
        let two = object_box(&st, id);
        assert!(two.1.y > one.1.y + 5.0, "the sphere on top: {two:?} vs {one:?}");
        st.undo();
        st.sdf_tick();
        assert_eq!(shapes(scene_of(st.doc.feature(id).unwrap()).unwrap()).len(), 1);
        assert!((object_box(&st, id).1.y - one.1.y).abs() < 0.5);
        st.redo();
        st.sdf_tick();
        assert_eq!(shapes(scene_of(st.doc.feature(id).unwrap()).unwrap()).len(), 2);
    }

    /// The last shape is not deleted alone (its object is); a copy is
    /// picked, the cage scales a shape along its axes.
    #[test]
    fn delete_copy_scale() {
        let mut st = app();
        st.sdf_add(PrimKind::Box);
        assert!(!st.sdf_delete(), "the last shape: the object instead");
        st.sdf_sel = st.selection.first().and_then(|id| st.doc.feature(*id)).and_then(scene_of).map(|s| (st.selection[0], shapes(s)[0]));
        assert!(st.sdf_duplicate());
        let id = st.selection[0];
        assert_eq!(shapes(scene_of(st.doc.feature(id).unwrap()).unwrap()).len(), 2);
        let mut s = Shape::new(PrimKind::Cylinder);
        s.size = Vec3::new(5.0, 10.0, 0.0);
        scale_shape(&mut s, Vec3::new(2.0, 3.0, 2.0));
        assert_eq!(s.size, Vec3::new(10.0, 30.0, 0.0));
        let mut t = Shape::new(PrimKind::Torus);
        t.size = Vec3::new(8.0, 0.0, 2.0);
        scale_shape(&mut t, Vec3::new(2.0, 0.5, 2.0));
        assert_eq!(t.size, Vec3::new(16.0, 0.0, 1.0));
    }

    /// The object scaled as a whole (R in Object mode): its shapes take the
    /// scale at the next meshing, the surface where the stretched mesh was.
    #[test]
    fn object_scale_goes_into_the_shapes() {
        let mut st = app();
        st.sdf_add(PrimKind::Sphere);
        let id = st.sdf_target().unwrap();
        st.sdf_tick();
        let (lo, hi) = object_box(&st, id);
        st.edit("Scale", |d| {
            if let Some(f) = d.feature_mut(id) {
                if let Prim::Box { x, y, z } = &mut f.prim {
                    *x *= 2.0;
                    *y *= 2.0;
                    *z *= 2.0;
                }
            }
        });
        st.sdf_tick();
        let (lo2, hi2) = object_box(&st, id);
        let r = (hi2 - lo2) / (hi - lo);
        assert!((r - Vec3::splat(2.0)).abs().max_element() < 0.08, "scaled ×2: {r:?}");
        let sh = scene_of(st.doc.feature(id).unwrap()).unwrap();
        let r0 = sh.node(shapes(sh)[0]).unwrap().shape().unwrap().size.x;
        assert!((r0 - 20.0).abs() < 0.5, "the sphere's radius 10 → 20: {r0}");
    }

    /// The fine meshing in a thread arrives and is finer.
    #[test]
    fn fine_meshing_arrives() {
        let mut st = app();
        st.sdf_add(PrimKind::Torus);
        let id = st.sdf_target().unwrap();
        st.sdf_tick();
        let coarse = st.doc.feature(id).unwrap().mesh.as_ref().unwrap().faces.len();
        st.sdf_settle();
        let fine = st.doc.feature(id).unwrap().mesh.as_ref().unwrap().faces.len();
        assert!(st.sdf_meshed.get(&id).map(|s| s.1).unwrap_or(false), "fine");
        assert!(fine > coarse * 2, "{fine} vs {coarse}");
    }
}