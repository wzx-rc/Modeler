//! The scene's meshes (Sculpt and Poly tabs) beside the kernel's part: their
//! frame, what the renderer draws of them, picking on them. A mesh is a
//! feature (`Feature::mesh`), so the outliner, the selection, Move / Rotate /
//! Scale, Duplicate, Delete, Hide and the materials work on it as on a
//! shape; the kernel never sees it (`Doc::for_kernel`).

use crate::render::{line_vertices, LineVertex, MeshVertex, NO_DEV};
use crate::state::AppState;
use glam::{Mat4, Vec3};
use nurbs_kernel::mesh::{catmull_clark, poly_normal, MeshData, MeshKind};
use nurbs_kernel::{Feature, FeatureId, Prim};

/// Its unit box to the world: its frame and its box's sizes.
pub fn matrix(f: &Feature) -> Mat4 {
    let size = match f.prim {
        Prim::Box { x, y, z } => Vec3::new(x as f32, y as f32, z as f32),
        _ => Vec3::ONE,
    };
    Mat4::from_scale_rotation_translation(size, f.rot.as_quat(), f.origin.as_vec3())
}

/// Its box's sizes (mm).
pub fn size(f: &Feature) -> Vec3 {
    match f.prim {
        Prim::Box { x, y, z } => Vec3::new(x as f32, y as f32, z as f32),
        _ => Vec3::ONE,
    }
}

/// Its points in the world.
pub fn world_points(f: &Feature) -> Vec<Vec3> {
    let Some(m) = &f.mesh else { return vec![] };
    let x = matrix(f);
    m.verts.iter().map(|v| x.transform_point3(Vec3::from(*v))).collect()
}

/// World points put back in the unit box.
pub fn to_unit(f: &Feature, pts: &[Vec3]) -> Vec<[f32; 3]> {
    let inv = matrix(f).inverse();
    pts.iter().map(|p| inv.transform_point3(*p).to_array()).collect()
}

/// The box fitted again to the points (after an edit that moved them out
/// of it): the sizes and origin change, the points stay where they are.
pub fn refit(f: &mut Feature) {
    let s = size(f);
    let Some(m) = f.mesh.as_mut() else { return };
    let (s2, base) = m.make_mut().renormalized(s);
    f.origin += f.rot * base.as_dvec3();
    f.prim = Prim::Box { x: s2.x as f64, y: s2.y as f64, z: s2.z as f64 };
}

/// Catmull-Clark `levels` times, each face knowing the polygon of the cage
/// it comes from (the smooth preview lights the cage's faces).
pub fn smoothed(pts: &[Vec3], faces: &[Vec<u32>], levels: usize) -> (Vec<Vec3>, Vec<Vec<u32>>, Vec<u32>) {
    let mut p = pts.to_vec();
    let mut f = faces.to_vec();
    let mut parent: Vec<u32> = (0..faces.len() as u32).collect();
    for _ in 0..levels {
        let (p2, f2) = catmull_clark(&p, &f);
        // catmull_clark writes each old face's quads one after the other
        let mut par = Vec::with_capacity(f2.len());
        for (i, old) in f.iter().enumerate() {
            par.extend(std::iter::repeat(parent[i]).take(old.len()));
        }
        p = p2;
        f = f2;
        parent = par;
    }
    (p, f, parent)
}

/// A mesh's triangles, its face slots starting at `base`.
pub fn mesh_vertices(f: &Feature, base: u32, out: &mut Vec<MeshVertex>) -> usize {
    let Some(m) = &f.mesh else { return 0 };
    let pts = world_points(f);
    match m.kind {
        MeshKind::Sculpt => {
            // smooth: a normal per point
            let n = m.vertex_normals(&pts);
            out.reserve(m.faces.len() * 3);
            for t in &m.faces {
                for k in 1..t.len().saturating_sub(1) {
                    for v in [t[0], t[k], t[k + 1]] {
                        out.push(MeshVertex { pos: pts[v as usize].to_array(), nrm: n[v as usize].to_array(), face: base, dev: NO_DEV });
                    }
                }
            }
            1
        }
        MeshKind::Poly => {
            if m.smooth {
                let (p, fs, parent) = smoothed(&pts, &m.faces, 2);
                let tmp = MeshData { kind: MeshKind::Poly, verts: vec![], faces: fs.clone(), smooth: false };
                let n = tmp.vertex_normals(&p);
                for (i, q) in fs.iter().enumerate() {
                    for k in 1..q.len().saturating_sub(1) {
                        for v in [q[0], q[k], q[k + 1]] {
                            out.push(MeshVertex { pos: p[v as usize].to_array(), nrm: n[v as usize].to_array(), face: base + parent[i], dev: NO_DEV });
                        }
                    }
                }
            } else {
                // flat: a normal per polygon (a cage reads its facets)
                for (i, q) in m.faces.iter().enumerate() {
                    let n = poly_normal(&pts, q).normalize_or(Vec3::Y).to_array();
                    for k in 1..q.len().saturating_sub(1) {
                        for v in [q[0], q[k], q[k + 1]] {
                            out.push(MeshVertex { pos: pts[v as usize].to_array(), nrm: n, face: base + i as u32, dev: NO_DEV });
                        }
                    }
                }
            }
            m.faces.len()
        }
    }
}

/// A Poly mesh's cage as lines (its edges; the picked ones apart).
pub fn cage_lines(f: &Feature, picked: &[(u32, u32)], strong: bool, accent: [u8; 4], out: &mut Vec<LineVertex>) {
    let Some(m) = &f.mesh else { return };
    if m.kind != MeshKind::Poly {
        return;
    }
    let pts = world_points(f);
    let mut plain = vec![];
    let mut lit = vec![];
    for e in m.edges() {
        let l = vec![pts[e.0 as usize].to_array(), pts[e.1 as usize].to_array()];
        if picked.contains(&e) {
            lit.push(l);
        } else {
            plain.push(l);
        }
    }
    let c = if strong { [20, 24, 30, 230] } else { [24, 26, 30, 150] };
    line_vertices(&plain, c, if strong { 1.0 } else { 0.8 }, out);
    line_vertices(&lit, accent, 2.2, out);
}

/// A hit on a mesh.
#[derive(Clone, Copy, Debug)]
pub struct MeshHit {
    pub id: FeatureId,
    pub t: f32,
    pub point: Vec3,
    /// The triangle's normal (world, towards the eye).
    pub normal: Vec3,
    /// The polygon hit.
    pub face: u32,
}

/// The ray (world) against one mesh: the nearest hit.
pub fn ray_mesh(f: &Feature, ro: Vec3, rd: Vec3) -> Option<MeshHit> {
    let m = f.mesh.as_ref()?;
    let x = matrix(f);
    let inv = x.inverse();
    // in the unit box (t is kept: the direction is not made unit)
    let lo = inv.transform_point3(ro);
    let ld = inv.transform_vector3(rd);
    // the unit box first (with a margin: points may have left it a little)
    let (bl, bh) = bounds(m);
    let invd = Vec3::new(1.0 / ld.x, 1.0 / ld.y, 1.0 / ld.z);
    let t0 = (bl - lo) * invd;
    let t1 = (bh - lo) * invd;
    if t0.min(t1).max_element() > t0.max(t1).min_element() || t0.max(t1).min_element() < 0.0 {
        return None;
    }
    let mut best: Option<(f32, u32, Vec3, Vec3, Vec3)> = None;
    for (fi, q) in m.faces.iter().enumerate() {
        for k in 1..q.len().saturating_sub(1) {
            let (a, b, c) = (Vec3::from(m.verts[q[0] as usize]), Vec3::from(m.verts[q[k] as usize]), Vec3::from(m.verts[q[k + 1] as usize]));
            if let Some(t) = ray_tri(lo, ld, a, b, c) {
                if best.map(|bb| t < bb.0).unwrap_or(true) {
                    best = Some((t, fi as u32, a, b, c));
                }
            }
        }
    }
    let (t, face, a, b, c) = best?;
    let (wa, wb, wc) = (x.transform_point3(a), x.transform_point3(b), x.transform_point3(c));
    let mut n = (wb - wa).cross(wc - wa).normalize_or(Vec3::Y);
    if n.dot(rd) > 0.0 {
        n = -n;
    }
    Some(MeshHit { id: f.id, t, point: ro + rd * t, normal: n, face })
}

/// The unit box of its points, a little grown.
pub fn bounds(m: &MeshData) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for v in &m.verts {
        lo = lo.min(Vec3::from(*v));
        hi = hi.max(Vec3::from(*v));
    }
    let g = (hi - lo).max_element() * 0.01 + 1e-4;
    (lo - Vec3::splat(g), hi + Vec3::splat(g))
}

/// Möller-Trumbore, both sides.
pub fn ray_tri(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-6).then_some(t)
}

/// The world box of a mesh's points.
pub fn world_bounds(f: &Feature) -> Option<(Vec3, Vec3)> {
    let pts = world_points(f);
    if pts.is_empty() {
        return None;
    }
    Some(pts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p))))
}

impl AppState {
    /// The meshes shown (visible, and kept by Isolate).
    pub fn shown_meshes(&self) -> Vec<&Feature> {
        let iso = self.isolate.as_ref().map(|_| self.isolated_ids());
        self.doc.features.iter().filter(|f| f.is_mesh() && f.visible && iso.as_ref().map(|k| k.contains(&f.id)).unwrap_or(true)).collect()
    }

    /// The mesh under the pointer, nearest (of `kind` when given).
    pub fn pick_mesh(&self, p: [f32; 2], kind: Option<MeshKind>) -> Option<MeshHit> {
        let (ro, rd) = self.view().ray(p[0], p[1]);
        self.shown_meshes()
            .into_iter()
            .filter(|f| kind.map(|k| f.mesh.as_ref().map(|m| m.kind == k).unwrap_or(false)).unwrap_or(true))
            .filter_map(|f| ray_mesh(f, ro, rd))
            .min_by(|a, b| a.t.total_cmp(&b.t))
    }

    /// The element under the pointer in Object mode: a mesh when it is in
    /// front of the part (or no part there), else None (the part decides).
    pub fn mesh_in_front(&self, p: [f32; 2]) -> Option<FeatureId> {
        let h = self.pick_mesh(p, None)?;
        match self.pick_at(p) {
            Some(k) if k.t < h.t => None,
            _ => Some(h.id),
        }
    }

    /// What a click picks in Object mode: a curve, a mesh in front of the
    /// part, else the element of the part's face under the pointer.
    pub fn pick_object(&self, p: [f32; 2]) -> Option<FeatureId> {
        if let Some(id) = self.pick_path(p) {
            return Some(id);
        }
        if let Some(id) = self.mesh_in_front(p) {
            return Some(id);
        }
        self.pick_at(p).and_then(|h| self.feature_of_face(h.face))
    }

    /// What the renderer keys the meshes' upload on.
    pub fn meshes_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.mesh_gen.hash(&mut h);
        self.workspace.index().hash(&mut h);
        for f in self.shown_meshes() {
            f.id.hash(&mut h);
            f.mesh.as_ref().map(|m| (m.ptr(), m.smooth)).hash(&mut h);
            for v in [f.origin.x, f.origin.y, f.origin.z, f.rot.x, f.rot.y, f.rot.z, f.rot.w] {
                v.to_bits().hash(&mut h);
            }
            for v in [size(f).x, size(f).y, size(f).z] {
                v.to_bits().hash(&mut h);
            }
        }
        if let Some(s) = &self.poly_sel {
            s.edges.hash(&mut h);
        }
        self.poly_target().hash(&mut h);
        h.finish()
    }
}
