//! The linked surface in the app (`nurbs_kernel::patch`; Romain, 20 nov.
//! 2026, 18 h 12: "une surface… peut avoir les trois états à la fois"): one
//! element that is a NURBS surface, a Poly mesh and a clay. Made from a
//! closed path of four points (one patch), or from a Forme Cube or Cylinder
//! (a closed object: a cube's six patches, a cylinder's fourteen, Romain 23 h: "pousser le Link surface à
//! cylindre et cube"); its own window switches its state (NURBS, POLY,
//! SCULPT), the tab following. In the NURBS state its contour's points are
//! picked in Vertex mode (1) and moved with the gizmo, the Poly layer
//! following them (`apply_binding`); the Poly layer's own shapes (an
//! extrusion) are drawn over the surface, not editable. In the Poly state it
//! is a Poly mesh like any other; back to NURBS, its points are bound to the
//! surface again (`rebind`). In the Sculpt state the mesh is a clay: the
//! Poly layer subdivided, the brushes' work kept as offsets over it
//! (`SculptLayer`), so the details ride on the contour and on the Poly
//! layer's edits, and show in the NURBS state. A Carve (box or cylinder)
//! after it in the history cuts it live in every state (`patch_solids`,
//! drawn and picked through `meshes`).

use crate::meshes::{refit, size, smoothed};
use crate::state::{AppState, Handle};
use glam::{DMat4, DVec3, Mat4, Vec3};
use nurbs_kernel::mesh::{MeshData, MeshKind};
use nurbs_kernel::patch::{self, Patch, PatchState, PolyKeep, SculptLayer, Solid, SolidKind};
use nurbs_kernel::{Feature, FeatureId, Op, Prim};

/// The contour's points picked (indices into the control points).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PatchSel {
    pub id: FeatureId,
    pub cvs: Vec<usize>,
}

/// A gizmo drag on the contour's points: the surface as it was, the points
/// it moves (a corner takes its tangent points with it).
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

/// Drawn and edited as the NURBS.
pub fn shows_nurbs(f: &Feature) -> bool {
    matches!(state_of(f), Some(PatchState::Nurbs))
}

/// The frame in millimetres to the world.
pub fn to_world(f: &Feature) -> Mat4 {
    DMat4::from_rotation_translation(f.rot, f.origin).as_mat4()
}

/// Its control points in the world (all of them, every patch's).
pub fn cvs_world(f: &Feature) -> Option<Vec<Vec3>> {
    let p = patch_of(f)?;
    let x = to_world(f);
    let s = size(f);
    Some(p.cv.iter().map(|c| x.transform_point3(Vec3::from(*c) * s)).collect())
}

/// The Poly layer's points (mm, the frame) where its binding puts them.
fn placed(p: &Patch, s: Vec3) -> Vec<Vec3> {
    let surfs = p.surfaces(s);
    p.bind.iter().enumerate().map(|(i, b)| patch::place(&surfs[p.on_of(i).min(surfs.len() - 1)], *b)).collect()
}

/// The Poly layer put back on the surface from its binding (after the
/// contour moved).
pub fn apply_binding(f: &mut Feature) {
    let s = size(f);
    let Some(m) = f.mesh.as_mut() else { return };
    let Some(p) = m.patch.as_ref() else { return };
    if p.bind.len() != m.verts.len() {
        return;
    }
    let unit: Vec<[f32; 3]> = placed(p, s).iter().map(|q| (*q / s).to_array()).collect();
    m.make_mut().verts = unit;
}

/// Each point of the Poly layer bound to the surface again (back from Poly);
/// what lies on its facets put on the surface itself (`patch::settle_on`).
pub fn rebind(f: &mut Feature) {
    let s = size(f);
    let Some(m) = f.mesh.as_mut() else { return };
    let Some(p) = m.patch.clone() else { return };
    if p.state == PatchState::Sculpt {
        return;
    }
    let surfs = p.surfaces(s);
    let seeds = patch::seeds_multi(&surfs, if surfs.len() == 1 { 24 } else { 12 });
    let (on, mut bind): (Vec<u16>, Vec<[f32; 5]>) = m.local(s).iter().map(|q| {
        let (k, b) = patch::bind_multi(&surfs, *q, &seeds);
        (k as u16, b)
    }).unzip();
    // what lies on the layer's facets rides on the surface itself
    let settled = patch::settle_on_multi(&surfs, &mut bind, &on, &m.faces);
    if let Some(pp) = m.make_mut().patch.as_mut() {
        pp.bind = bind;
        pp.on = if surfs.len() == 1 { vec![] } else { on };
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
    if p.state == PatchState::Sculpt {
        return 0.0;
    }
    let pts = m.local(s);
    if p.bind.len() != pts.len() {
        return f32::INFINITY;
    }
    pts.iter().zip(placed(p, s)).map(|(q, b)| b.distance(*q)).fold(0.0, f32::max)
}

/// The fit of the box to the points after the mesh's points changed: its
/// sizes and its origin (the points and control points stay in place).
fn store(f: &mut Feature, pts: &[Vec3]) {
    let s = size(f);
    let Some(m) = f.mesh.as_mut() else { return };
    let (s2, base) = m.make_mut().store(pts, s);
    f.origin += f.rot * base.as_dvec3();
    f.prim = Prim::Box { x: s2.x as f64, y: s2.y as f64, z: s2.z as f64 };
}

// ------------------------------------------------------------ the clay

/// The clay's levels of subdivision: about 30 000 corners at least (as
/// Send to Sculpt), 6 at most.
fn levels_for(faces: &[Vec<u32>]) -> u32 {
    let mut n = faces.iter().map(|q| q.len()).sum::<usize>();
    let mut levels = 0;
    while n < 30000 && levels < 6 {
        n *= 4;
        levels += 1;
    }
    levels
}

/// Each point's frame on a mesh: along its first edge, across, its normal.
fn frames(pts: &[Vec3], faces: &[Vec<u32>]) -> Vec<(Vec3, Vec3, Vec3)> {
    let tmp = MeshData { kind: MeshKind::Poly, verts: vec![], faces: faces.to_vec(), smooth: false, angle: 0.0, patch: None };
    let n = tmp.vertex_normals(pts);
    let mut next = vec![u32::MAX; pts.len()];
    for q in faces {
        for i in 0..q.len() {
            let v = q[i] as usize;
            if next[v] == u32::MAX {
                next[v] = q[(i + 1) % q.len()];
            }
        }
    }
    (0..pts.len())
        .map(|i| {
            let nn = n[i].normalize_or(Vec3::Y);
            let e = if next[i] == u32::MAX { nn.any_orthonormal_vector() } else { pts[next[i] as usize] - pts[i] };
            let t = (e - nn * e.dot(nn)).normalize_or(nn.any_orthonormal_vector());
            (t, nn.cross(t), nn)
        })
        .collect()
}

/// The clay's base (mm): the Poly layer cut `levels` times (each polygon
/// into quads round its middle, as Catmull-Clark's faces) without rounding
/// it: a new point between points lying on the surface goes on the surface
/// (their (u, v) mixed, the patch evaluated there), else between them. So
/// the clay keeps the object's shape (a cube stays a cube, its faces the
/// NURBS's), the Poly layer's own shapes (an extrusion) their flat walls.
/// `p`'s binding is the Poly layer's.
fn base_of(p: &Patch, s: Vec3, poly: &[Vec3], faces: &[Vec<u32>], levels: u32) -> (Vec<Vec3>, Vec<Vec<u32>>) {
    if p.bind.len() != poly.len() {
        let (q, f, _) = smoothed(poly, faces, levels as usize);
        return (q, f);
    }
    let surfs = p.surfaces(s);
    // each point: on which patch at (u, v), when it lies on the surface
    let mut at: Vec<Option<(usize, f32, f32)>> = p
        .bind
        .iter()
        .enumerate()
        .map(|(i, b)| (b[2].abs() + b[3].abs() + b[4].abs() <= patch::OFF).then(|| (p.on_of(i).min(surfs.len() - 1), b[0], b[1])))
        .collect();
    let mut pts = poly.to_vec();
    let mut fs = faces.to_vec();
    // a point among others: on the surface when they all are
    let mix = |idx: &[u32], pts: &[Vec3], at: &[Option<(usize, f32, f32)>]| -> (Vec3, Option<(usize, f32, f32)>) {
        let lin = idx.iter().map(|v| pts[*v as usize]).sum::<Vec3>() / idx.len() as f32;
        let all: Option<Vec<(usize, f32, f32)>> = idx.iter().map(|v| at[*v as usize]).collect();
        let Some(all) = all else { return (lin, None) };
        let k = all[0].0;
        if all.iter().all(|a| a.0 == k) {
            let (u, v) = (all.iter().map(|a| a.1).sum::<f32>() / all.len() as f32, all.iter().map(|a| a.2).sum::<f32>() / all.len() as f32);
            return (patch::eval(&surfs[k], u, v).0, Some((k, u, v)));
        }
        // across a seam: put on the first one's patch, from its place
        let b = patch::bind(&surfs[k], lin, &[(patch::eval(&surfs[k], all[0].1, all[0].2).0, [all[0].1, all[0].2])]);
        let reach = idx.iter().map(|v| pts[*v as usize].distance(lin)).fold(0.0f32, f32::max);
        if b[2].abs() + b[3].abs() < 1e-3 * (1.0 + reach) && b[4].abs() < 0.25 * reach + 1e-3 {
            (patch::eval(&surfs[k], b[0], b[1]).0, Some((k, b[0], b[1])))
        } else {
            (lin, None)
        }
    };
    for _ in 0..levels {
        let mut edge: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
        let mut nf = Vec::with_capacity(fs.iter().map(|q| q.len()).sum());
        for q in &fs {
            let n = q.len();
            let (c, ca) = mix(q, &pts, &at);
            pts.push(c);
            at.push(ca);
            let fc = (pts.len() - 1) as u32;
            let mut mids = Vec::with_capacity(n);
            for i in 0..n {
                let (a, b) = (q[i], q[(i + 1) % n]);
                let key = (a.min(b), a.max(b));
                let e = match edge.get(&key) {
                    Some(e) => *e,
                    None => {
                        let (m, ma) = mix(&[a, b], &pts, &at);
                        pts.push(m);
                        at.push(ma);
                        let e = (pts.len() - 1) as u32;
                        edge.insert(key, e);
                        e
                    }
                };
                mids.push(e);
            }
            for i in 0..n {
                nf.push(vec![q[i], mids[i], fc, mids[(i + n - 1) % n]]);
            }
        }
        fs = nf;
    }
    (pts, fs)
}

/// The base moved by the offsets (in the base's frames).
fn displaced(base: &[Vec3], faces: &[Vec<u32>], disp: &[[f32; 3]]) -> Vec<Vec3> {
    if disp.len() != base.len() || disp.iter().all(|d| d[0] == 0.0 && d[1] == 0.0 && d[2] == 0.0) {
        return base.to_vec();
    }
    let fr = frames(base, faces);
    base.iter().zip(&fr).zip(disp).map(|((b, (t, bb, n)), d)| *b + *t * d[0] + *bb * d[1] + *n * d[2]).collect()
}

/// The offsets made for a new base from the clay's last ones (by place on
/// the surface as it is now): the Poly layer's faces changed since.
fn carried_over(p: &Patch, s: Vec3, sl: &SculptLayer, base: &[Vec3], faces: &[Vec<u32>]) -> Vec<[f32; 3]> {
    if sl.base_bind.len() != sl.disp_s.len() || sl.base_bind.is_empty() {
        return vec![[0.0; 3]; base.len()];
    }
    let surfs = p.surfaces(s);
    let old: Vec<(Vec3, (Vec3, Vec3, Vec3))> = sl
        .base_bind
        .iter()
        .map(|b| {
            let cv = &surfs[(b[0] as usize).min(surfs.len() - 1)];
            let (q, fr) = patch::eval_frame(cv, b[1], b[2]);
            (q + fr.0 * b[3] + fr.1 * b[4] + fr.2 * b[5], fr)
        })
        .collect();
    let pos: Vec<Vec3> = old.iter().map(|o| o.0).collect();
    let near = patch::nearest_of(&pos, base);
    let fr = frames(base, faces);
    near.iter()
        .zip(&fr)
        .map(|(k, (t, b, n))| {
            let d = sl.disp_s[*k];
            let (tu, tv, nn) = old[*k].1;
            let w = tu * d[0] + tv * d[1] + nn * d[2];
            [w.dot(*t), w.dot(*b), w.dot(*n)]
        })
        .collect()
}

/// The clay shown in the NURBS state: the Poly layer as it is now (it rides
/// on the contour) subdivided and moved by the kept offsets (mm, the
/// frame). None when nothing was sculpted.
pub fn sculpt_shown(f: &Feature) -> Option<(Vec<Vec3>, Vec<Vec<u32>>)> {
    let m = f.mesh.as_ref()?;
    let p = m.patch.as_ref()?;
    let sl = p.sculpt.as_ref()?;
    if p.state != PatchState::Nurbs || sl.topo != patch::topo_key(&m.faces) || sl.disp.iter().all(|d| d[0].abs() + d[1].abs() + d[2].abs() < 1e-5) {
        return None;
    }
    let (base, bf) = base_of(p, size(f), &m.local(size(f)), &m.faces, sl.levels);
    if base.len() != sl.disp.len() {
        return None;
    }
    let pts = displaced(&base, &bf, &sl.disp);
    Some((pts, bf))
}

thread_local! {
    /// The clays last shown in the NURBS state, by what made them (the
    /// drawing and the picking ask for the same one many times).
    static SHOWN: std::cell::RefCell<Vec<(u64, std::sync::Arc<(Vec<Vec3>, Vec<Vec<u32>>)>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// `sculpt_shown`, kept while its surface, layer and offsets stay as they
/// are.
pub fn sculpt_shown_cached(f: &Feature) -> Option<std::sync::Arc<(Vec<Vec3>, Vec<Vec<u32>>)>> {
    use std::hash::{Hash, Hasher};
    let m = f.mesh.as_ref()?;
    let p = m.patch.as_ref()?;
    let sl = p.sculpt.as_ref()?;
    if p.state != PatchState::Nurbs {
        return None;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut put = |x: &[f32]| x.iter().for_each(|v| v.to_bits().hash(&mut h));
    put(&size(f).to_array());
    p.cv.iter().for_each(|c| put(c));
    p.bind.iter().for_each(|c| put(c));
    m.verts.iter().for_each(|c| put(c));
    sl.disp.iter().for_each(|c| put(c));
    (sl.levels, sl.topo, patch::topo_key(&m.faces), p.on.len()).hash(&mut h);
    p.on.hash(&mut h);
    let key = h.finish();
    if let Some(hit) = SHOWN.with(|c| c.borrow().iter().find(|e| e.0 == key).map(|e| e.1.clone())) {
        return Some(hit);
    }
    let made = std::sync::Arc::new(sculpt_shown(f)?);
    SHOWN.with(|c| {
        let mut c = c.borrow_mut();
        c.insert(0, (key, made.clone()));
        c.truncate(4);
    });
    Some(made)
}

/// The Sculpt layer carried over to the Poly layer's new faces (after the
/// Poly state changed them): by place, each new base point the offset of
/// the old one nearest it.
fn retarget_sculpt(f: &mut Feature) {
    let s = size(f);
    let Some(m) = f.mesh.as_ref() else { return };
    let Some(p) = m.patch.as_ref() else { return };
    let Some(sl) = p.sculpt.as_ref() else { return };
    let topo = patch::topo_key(&m.faces);
    if sl.topo == topo {
        return;
    }
    let levels = levels_for(&m.faces);
    let (base, bf) = base_of(p, s, &m.local(s), &m.faces, levels);
    let disp = carried_over(p, s, sl, &base, &bf);
    let shown = displaced(&base, &bf, &disp);
    let new = layer_of(p, s, levels, topo, &base, &bf, &shown);
    if let Some(pp) = f.mesh.as_mut().and_then(|m| m.make_mut().patch.as_mut()) {
        pp.sculpt = Some(new);
    }
}

/// The Sculpt layer of a clay over its base: the offsets in the base's
/// frames, and where each base point sits on the surface with the offset in
/// the surface's frame there (for carrying it over later).
fn layer_of(p: &Patch, s: Vec3, levels: u32, topo: u64, base: &[Vec3], bf: &[Vec<u32>], clay: &[Vec3]) -> SculptLayer {
    let fr = frames(base, bf);
    let disp: Vec<[f32; 3]> = base.iter().zip(clay).zip(&fr).map(|((b, c), (t, bb, n))| {
        let d = *c - *b;
        [d.dot(*t), d.dot(*bb), d.dot(*n)]
    }).collect();
    let surfs = p.surfaces(s);
    let seeds = patch::seeds_multi(&surfs, 8);
    let mut base_bind = Vec::with_capacity(base.len());
    let mut disp_s = Vec::with_capacity(base.len());
    for (b, c) in base.iter().zip(clay) {
        let (k, bb) = patch::bind_multi(&surfs, *b, &seeds);
        base_bind.push([k as f32, bb[0], bb[1], bb[2], bb[3], bb[4]]);
        let (_, (tu, tv, n)) = patch::eval_frame(&surfs[k], bb[0], bb[1]);
        let d = *c - *b;
        disp_s.push([d.dot(tu), d.dot(tv), d.dot(n)]);
    }
    SculptLayer { levels, topo, disp, base_bind, disp_s }
}

/// Into the Sculpt state: the Poly layer set aside, the mesh made the clay
/// (the layer subdivided, the kept offsets on it).
fn enter_sculpt(f: &mut Feature) {
    let s = size(f);
    let Some(m) = f.mesh.as_ref() else { return };
    let Some(p) = m.patch.clone() else { return };
    if p.state == PatchState::Sculpt || p.keep.is_some() {
        return;
    }
    let poly = m.local(s);
    let faces = m.faces.clone();
    let topo = patch::topo_key(&faces);
    let levels = match &p.sculpt {
        Some(sl) if sl.topo == topo => sl.levels,
        _ => levels_for(&faces),
    };
    let (base, bf) = base_of(&p, s, &poly, &faces, levels);
    let disp = match &p.sculpt {
        Some(sl) if sl.topo == topo && sl.disp.len() == base.len() => sl.disp.clone(),
        Some(sl) => carried_over(&p, s, sl, &base, &bf),
        None => vec![[0.0; 3]; base.len()],
    };
    let clay = displaced(&base, &bf, &disp);
    let keep = PolyKeep { verts: m.verts.clone(), faces, bind: p.bind.clone(), on: p.on.clone() };
    {
        let md = f.mesh.as_mut().unwrap().make_mut();
        md.kind = MeshKind::Sculpt;
        md.faces = bf;
        md.smooth = false;
        let pp = md.patch.as_mut().unwrap();
        pp.keep = Some(keep);
        pp.bind = vec![];
        pp.on = vec![];
        pp.state = PatchState::Sculpt;
        // the levels and faces now; the offsets are read back on the way out
        let mut sl = pp.sculpt.clone().unwrap_or(SculptLayer { levels, topo, disp: vec![], base_bind: vec![], disp_s: vec![] });
        sl.levels = levels;
        sl.topo = topo;
        pp.sculpt = Some(sl);
    }
    store(f, &clay);
}

/// Out of the Sculpt state: the clay's work read as offsets over its base,
/// the Poly layer put back as the mesh.
fn leave_sculpt(f: &mut Feature, to: PatchState) {
    let s = size(f);
    let Some(m) = f.mesh.as_ref() else { return };
    let Some(p) = m.patch.clone() else { return };
    let Some(keep) = p.keep.clone() else { return };
    let clay = m.local(s);
    let poly: Vec<Vec3> = keep.verts.iter().map(|v| Vec3::from(*v) * s).collect();
    let topo = patch::topo_key(&keep.faces);
    let levels = p.sculpt.as_ref().map(|l| l.levels).unwrap_or_else(|| levels_for(&keep.faces));
    // the surface the base is bound to: the Poly layer's binding is the
    // one kept
    let mut bound = p.clone();
    bound.bind = keep.bind.clone();
    bound.on = keep.on.clone();
    let (base, bf) = base_of(&bound, s, &poly, &keep.faces, levels);
    let layer = if base.len() == clay.len() { Some(layer_of(&bound, s, levels, topo, &base, &bf, &clay)) } else { None };
    {
        let md = f.mesh.as_mut().unwrap().make_mut();
        md.kind = MeshKind::Poly;
        md.faces = keep.faces.clone();
        let pp = md.patch.as_mut().unwrap();
        pp.keep = None;
        pp.bind = keep.bind;
        pp.on = keep.on;
        pp.state = to;
        if layer.is_some() {
            pp.sculpt = layer;
        }
    }
    store(f, &poly);
}

/// A linked object made on a Cube or a Cylinder's place and sizes, added
/// to the document (the shape itself left as it is): its faces six NURBS
/// patches sharing their seams, its Poly layer a grid on each (about 8
/// quads along its largest side, the same count on both patches of each
/// seam) welded at the seams and bound to them.
pub fn linked_solid(d: &mut nurbs_kernel::Doc, src: &Feature) -> Option<FeatureId> {
    let (mut p, name) = match src.prim {
        Prim::Box { x, y, z } => (Patch::closed(&patch::box_patches(Vec3::new(x as f32, y as f32, z as f32))), "Linked cube"),
        Prim::Cylinder { radius, height } => (Patch::cylinder(radius as f32, height as f32), "Linked cylinder"),
        _ => return None,
    };
    let surfs = p.surfaces(Vec3::ONE);
    let ext = surfs.iter().flat_map(|q| q.iter()).fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
    let cell = (ext.1 - ext.0).max_element() / 8.0;
    let counts = p.closed_counts(Vec3::ONE, cell);
    let (pts, faces, on, uv) = patch::grid_closed(&surfs, &counts);
    let made = d.add_mesh(MeshKind::Poly, &pts, faces, src.origin, src.rot, name);
    let f = d.feature_mut(made)?;
    f.material = src.material;
    let s = size(f);
    let base = {
        let lo = pts.iter().fold(Vec3::splat(f32::MAX), |a, p| a.min(*p));
        let hi = pts.iter().fold(Vec3::splat(f32::MIN), |a, p| a.max(*p));
        Vec3::new((lo.x + hi.x) * 0.5, lo.y, (lo.z + hi.z) * 0.5)
    };
    p.remap(Vec3::ONE, base, s);
    p.bind = uv.iter().map(|t| [t[0], t[1], 0.0, 0.0, 0.0]).collect();
    p.on = on;
    f.mesh.as_mut()?.make_mut().patch = Some(p);
    Some(made)
}

/// The state switched on a feature (the document's side of
/// `AppState::set_patch_state`): out of Sculpt the clay kept as offsets,
/// out of Poly bound again and the clay carried to the new faces, into
/// Sculpt the clay made.
pub fn switch_state(f: &mut Feature, s: PatchState) {
    let Some(from) = state_of(f) else { return };
    if from == s {
        return;
    }
    if from == PatchState::Sculpt {
        leave_sculpt(f, s);
    }
    if s != PatchState::Poly && from == PatchState::Poly {
        rebind(f);
        retarget_sculpt(f);
    }
    if s == PatchState::Sculpt {
        if let Some(p) = f.mesh.as_mut().and_then(|m| m.make_mut().patch.as_mut()) {
            p.state = PatchState::Poly;
        }
        enter_sculpt(f);
    } else if let Some(p) = f.mesh.as_mut().and_then(|m| m.make_mut().patch.as_mut()) {
        p.state = s;
    }
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

    /// A Forme Cube or Cylinder that can become a linked object: a plain
    /// Add shape (no outline of its own), shown.
    pub fn linkable_solid(&self) -> Option<&Feature> {
        let f = self.single()?;
        (!f.is_mesh() && f.visible && f.is_solid() && f.op == Op::Add && f.profile.is_none() && f.curve.is_none() && matches!(f.prim, Prim::Box { .. } | Prim::Cylinder { .. })).then_some(f)
    }

    /// The selected Cube or Cylinder made a linked object (one undo step):
    /// its faces six NURBS patches sharing their seams (a cylinder's side in
    /// four quarters, its caps discs of the four arcs), its Poly layer a
    /// grid on each welded at the seams, bound to them; the shape is hidden
    /// (undo or show it to have it back). Its operations (fillets) are not
    /// carried.
    pub fn make_linked_solid(&mut self) -> bool {
        let Some(src) = self.linkable_solid().cloned() else {
            self.toast("Linked object: select a Cube or a Cylinder (Add)", true);
            return false;
        };
        let name = if matches!(src.prim, Prim::Box { .. }) { "Linked cube" } else { "Linked cylinder" };
        let mut made = 0;
        self.edit(name, |d| {
            if let Some(id) = linked_solid(d, &src) {
                made = id;
                if let Some(sf) = d.feature_mut(src.id) {
                    sf.visible = false;
                }
            }
        });
        if made == 0 {
            return false;
        }
        self.selection = vec![made];
        self.poly_sel = None;
        self.patch_sel = None;
        self.mesh_gen += 1;
        self.toast(&format!("{name}: NURBS, Poly and Sculpt in one; the {} is hidden", if matches!(src.prim, Prim::Box { .. }) { "Cube" } else { "Cylinder" }), false);
        true
    }

    /// The linked surface the window and the contour's points work on: the
    /// single selected one.
    pub fn linked_target(&self) -> Option<FeatureId> {
        let f = self.single()?;
        patch_of(f).map(|_| f.id)
    }

    /// Switches its state, one undo step; the tab follows (NURBS: Forme,
    /// Poly: Poly, Sculpt: Sculpt). Back to NURBS, the Poly layer is bound
    /// to the surface again: its own shapes (an extrusion) then ride on the
    /// contour. Into Sculpt, the Poly layer subdivided is the clay, the
    /// details sculpted before back on it; out of it, they are kept.
    pub fn set_patch_state(&mut self, id: FeatureId, s: PatchState) {
        use crate::workspace::Workspace;
        let Some(f) = self.doc.feature(id) else { return };
        let Some(from) = state_of(f) else { return };
        if from == s {
            return;
        }
        self.commit_gestures(self.principal());
        let label = match s {
            PatchState::Nurbs => "To NURBS",
            PatchState::Poly => "To Poly",
            PatchState::Sculpt => "To Sculpt",
        };
        self.edit(label, |d| {
            if let Some(f) = d.feature_mut(id) {
                switch_state(f, s);
            }
        });
        self.poly_sel = None;
        self.patch_sel = None;
        self.patch_hover = None;
        self.stroke = None;
        self.sculpt_nb = None;
        self.selection = vec![id];
        self.mesh_gen += 1;
        self.set_workspace(match s {
            PatchState::Nurbs => Workspace::Forme,
            PatchState::Poly => Workspace::Poly,
            PatchState::Sculpt => Workspace::Sculpt,
        });
        self.selection = vec![id];
        match s {
            PatchState::Poly => self.toast("Poly: the mesh is yours (Vertex, Edge, Face modes); back to NURBS it rides on the surface", false),
            PatchState::Sculpt => self.toast("Sculpt: the Poly layer as clay; the brushes' work rides on the contour and the Poly layer", false),
            PatchState::Nurbs => {
                let f = self.doc.feature(id);
                let off = f.and_then(patch_of).map(|p| p.off_surface().iter().filter(|o| **o).count()).unwrap_or(0);
                let clay = f.and_then(sculpt_shown).is_some();
                let mut t = "NURBS: move the contour's points (Vertex mode, 1)".to_string();
                if off > 0 {
                    t += &format!("; {off} points of the Poly layer ride on it");
                }
                if clay {
                    t += "; the clay rides on it";
                }
                self.toast(&t, false);
            }
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

    /// The control points of the surface worked on, in the world, and the
    /// contour's (the ones edited, see `Patch::contour`).
    pub fn patch_points(&self) -> Option<(FeatureId, Vec<Vec3>, Vec<usize>)> {
        let id = self.linked_target()?;
        let f = self.doc.feature(id)?;
        Some((id, cvs_world(f)?, patch_of(f)?.contour()))
    }

    /// Is this control point a corner (of the surface worked on)?
    pub fn patch_is_corner(&self, k: usize) -> bool {
        self.linked_target().and_then(|id| self.doc.feature(id)).and_then(patch_of).map(|p| p.corner(k)).unwrap_or(false)
    }

    /// The contour's point under the pointer (within 14 points), the
    /// nearest to the eye first when several are as near on screen (the
    /// back of a closed object).
    pub fn patch_hover_at(&self, p: [f32; 2]) -> Option<usize> {
        let (id, cv, contour) = self.patch_points()?;
        let f = self.doc.feature(id)?;
        let pt = patch_of(f)?;
        let v = self.view();
        let mut best: Option<(f32, usize)> = None;
        for k in contour {
            let Some(s) = v.project(cv[k]) else { continue };
            let d = (s[0] - p[0]).hypot(s[1] - p[1]);
            // a corner wins over a tangent point as near, the nearer to the
            // eye over one behind
            let d = if pt.corner(k) { d - 2.0 } else { d } + (cv[k] - v.eye).length() * 1e-4;
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
        let Some((id, cv, contour)) = self.patch_points() else { return };
        let v = self.view();
        let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
        let mut s = match self.patch_sel.clone() {
            Some(s) if s.id == id && (shift || ctrl) => s,
            _ => PatchSel { id, cvs: vec![] },
        };
        for k in contour {
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
        let (_, cv, _) = self.patch_points()?;
        let k = self.patch_picked();
        if k.is_empty() {
            return None;
        }
        Some((k.iter().map(|i| cv[*i]).sum::<Vec3>() / k.len() as f32).as_dvec3())
    }

    /// The points a move of the picked ones takes: a corner its tangent
    /// points with it (a Bézier anchor).
    fn patch_moved(&self, f: &Feature, picked: &[usize]) -> Vec<usize> {
        let mut cvs = picked.to_vec();
        if let Some(p) = patch_of(f) {
            for k in picked {
                if p.corner(*k) {
                    for t in p.handles(*k) {
                        if !cvs.contains(&t) {
                            cvs.push(t);
                        }
                    }
                }
            }
        }
        cvs
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
        let cvs = self.patch_moved(&f, &picked);
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
        let s = size(&d.before);
        let local = (d.before.rot.inverse() * delta).as_vec3() / s;
        let mut g = d.before.clone();
        if let Some(pp) = g.mesh.as_mut().and_then(|m| m.make_mut().patch.as_mut()) {
            pp.move_points(&d.cvs, local, s);
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
    /// capture harness, the link); a corner takes its tangent points.
    pub fn move_patch_points(&mut self, id: FeatureId, cvs: &[usize], delta: Vec3) {
        let Some(f) = self.doc.feature(id).cloned() else { return };
        let cvs = self.patch_moved(&f, cvs);
        let s = size(&f);
        let local = (f.rot.inverse().as_quat() * delta) / s;
        self.edit("Move contour points", |doc| {
            let Some(g) = doc.feature_mut(id) else { return };
            if bound_off(g) > 0.01 {
                rebind(g);
            }
            if let Some(pp) = g.mesh.as_mut().and_then(|m| m.make_mut().patch.as_mut()) {
                pp.move_points(&cvs, local, s);
            }
            apply_binding(g);
            refit(g);
        });
        self.mesh_gen += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DQuat;
    use nurbs_kernel::Doc;

    /// A linked cube 40 x 20 x 30 at the origin.
    fn cube() -> (Doc, FeatureId) {
        let mut d = Doc::default();
        let src = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let f = d.feature(src).unwrap().clone();
        let id = linked_solid(&mut d, &f).unwrap();
        (d, id)
    }

    /// The highest point (world) of what the feature shows: the clay in
    /// Sculpt, the clay over the surface in NURBS, else the layer.
    fn top(f: &Feature) -> f32 {
        let x = to_world(f);
        match sculpt_shown(f) {
            Some((p, _)) => p.iter().map(|q| x.transform_point3(*q).y).fold(f32::MIN, f32::max),
            None => crate::meshes::world_points(f).iter().map(|q| q.y).fold(f32::MIN, f32::max),
        }
    }

    #[test]
    fn a_cube_made_linked() {
        let (d, id) = cube();
        let f = d.feature(id).unwrap();
        let p = patch_of(f).unwrap();
        assert_eq!(p.count(), 6);
        assert_eq!(p.bind.len(), f.mesh.as_ref().unwrap().verts.len());
        // the layer where its binding puts it, on the box
        assert!(bound_off(f) < 1e-3, "{}", bound_off(f));
        let pts = crate::meshes::world_points(f);
        let (lo, hi) = pts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), q| (a.min(*q), b.max(*q)));
        assert!(lo.distance(Vec3::new(-20.0, 0.0, -15.0)) < 1e-3 && hi.distance(Vec3::new(20.0, 20.0, 15.0)) < 1e-3, "{lo} {hi}");
    }

    #[test]
    fn a_cylinder_made_linked() {
        let mut d = Doc::default();
        let src = d.add(Op::Add, Prim::Cylinder { radius: 12.0, height: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let f = d.feature(src).unwrap().clone();
        let id = linked_solid(&mut d, &f).unwrap();
        let f = d.feature(id).unwrap();
        let p = patch_of(f).unwrap();
        // the side's four quarters, each end five patches round a square
        assert_eq!(p.count(), 14);
        // 32 on the rims and the side, each end's middle
        assert_eq!(p.contour().len(), 34);
        assert!(bound_off(f) < 1e-3, "{}", bound_off(f));
        // the layer: closed, all quads, none thin
        let m = f.mesh.as_ref().unwrap();
        let pts = crate::meshes::world_points(f);
        assert!(m.faces.iter().all(|q| q.len() == 4));
        for q in &m.faces {
            let e: Vec<f32> = (0..4).map(|i| pts[q[i] as usize].distance(pts[q[(i + 1) % 4] as usize])).collect();
            let (lo, hi) = e.iter().fold((f32::MAX, 0.0f32), |(a, b), x| (a.min(*x), b.max(*x)));
            assert!(lo > hi * 0.2, "a thin cell: {e:?}");
        }
        let mut count = std::collections::HashMap::new();
        for q in &m.faces {
            for i in 0..4 {
                let (a, b) = (q[i], q[(i + 1) % 4]);
                *count.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        assert!(count.values().all(|c| *c == 2), "the layer is open");
        for q in &pts {
            let r = Vec3::new(q.x, 0.0, q.z).length();
            assert!(r < 12.03 && q.y > -1e-3 && q.y < 30.0 + 1e-3);
        }
    }

    #[test]
    fn the_clay_is_what_a_click_hits_in_nurbs() {
        let (mut d, id) = cube();
        let f = d.feature_mut(id).unwrap();
        switch_state(f, PatchState::Sculpt);
        // the clay on the top raised 3 mm round its middle
        let mut pts = crate::meshes::world_points(f);
        for q in pts.iter_mut() {
            let r = Vec3::new(q.x, 0.0, q.z).length();
            if q.y > 19.99 && r < 8.0 {
                q.y += 3.0 * (1.0 - r / 8.0);
            }
        }
        store(f, &pts);
        switch_state(f, PatchState::Nurbs);
        let f = d.feature(id).unwrap();
        // straight down on the middle: the bump's top, not the surface
        let hit = crate::meshes::ray_linked(f, &[], Vec3::new(0.0, 50.0, 0.0), -Vec3::Y).unwrap();
        assert!((hit.point.y - 23.0).abs() < 0.1, "{}", hit.point);
        // level with the bump, from the side: it is hit over the top face
        let hit = crate::meshes::ray_linked(f, &[], Vec3::new(-60.0, 21.0, 0.0), Vec3::X).unwrap();
        assert!(hit.point.x < 0.0 && hit.point.x > -8.0, "{}", hit.point);
        // a Carve through the middle: the ray goes on through its hole
        let s = Solid { to_local: Mat4::IDENTITY, kind: SolidKind::Cylinder { r: 3.0, h: 100.0 } };
        let hit = crate::meshes::ray_linked(f, &[s], Vec3::new(0.0, 50.0, 0.0), -Vec3::Y);
        assert!(hit.map(|h| h.point.y < 1.0).unwrap_or(true), "{:?}", hit.map(|h| h.point));
    }

    #[test]
    fn the_clay_rides_on_the_contour_and_the_poly_layer() {
        let (mut d, id) = cube();
        let f = d.feature_mut(id).unwrap();
        let poly_faces = f.mesh.as_ref().unwrap().faces.len();
        // into Sculpt: the layer subdivided as clay, nothing moved yet
        switch_state(f, PatchState::Sculpt);
        let m = f.mesh.as_ref().unwrap();
        assert_eq!(m.kind, MeshKind::Sculpt);
        assert!(m.faces.len() >= poly_faces * 16, "{} clay faces", m.faces.len());
        assert!((top(f) - 20.0).abs() < 1e-3);
        // a dab: the clay round the top's middle raised 3 mm
        let mut pts = crate::meshes::world_points(f);
        for q in pts.iter_mut() {
            let r = Vec3::new(q.x, 0.0, q.z).length();
            if q.y > 19.99 && r < 8.0 {
                q.y += 3.0 * (1.0 - r / 8.0);
            }
        }
        store(f, &pts);
        assert!((top(f) - 23.0).abs() < 0.05, "{}", top(f));
        // back to NURBS: the clay kept, shown over the surface
        switch_state(f, PatchState::Nurbs);
        assert_eq!(f.mesh.as_ref().unwrap().kind, MeshKind::Poly);
        assert_eq!(f.mesh.as_ref().unwrap().faces.len(), poly_faces);
        assert!(sculpt_shown(f).is_some());
        assert!((top(f) - 23.0).abs() < 0.05, "{}", top(f));
        // the top's four corners raised 10 mm with their tangent points:
        // the layer follows the contour, the bump rides on it
        let s = size(f);
        let p = patch_of(f).unwrap().clone();
        let mut moved = vec![];
        for k in p.contour() {
            let c = Vec3::from(p.cv[k]) * s;
            let w = to_world(f).transform_point3(c);
            if w.y > 19.99 && p.corner(k) {
                moved.push(k);
                for t in p.handles(k) {
                    let tw = to_world(f).transform_point3(Vec3::from(p.cv[t]) * s);
                    if tw.y > 19.99 && !moved.contains(&t) {
                        moved.push(t);
                    }
                }
            }
        }
        assert_eq!(moved.len(), 4 + 8);
        {
            let pp = f.mesh.as_mut().unwrap().make_mut().patch.as_mut().unwrap();
            for k in &moved {
                pp.cv[*k][1] += 10.0 / s.y;
            }
            pp.fill_inside();
        }
        apply_binding(f);
        refit(f);
        assert!((top(f) - 33.0).abs() < 0.1, "{}", top(f));
        // into Sculpt again: the clay as it shows
        let shown = top(f);
        switch_state(f, PatchState::Sculpt);
        assert!((top(f) - shown).abs() < 1e-3, "{} vs {shown}", top(f));
        // Poly, its faces subdivided (new faces): back to NURBS the bump is
        // carried over to them
        switch_state(f, PatchState::Poly);
        {
            let s = size(f);
            let m = f.mesh.as_ref().unwrap();
            let (p2, f2) = nurbs_kernel::mesh::catmull_clark(&m.local(s), &m.faces);
            let md = f.mesh.as_mut().unwrap().make_mut();
            md.faces = f2;
            let _ = md;
            store(f, &p2);
        }
        switch_state(f, PatchState::Nurbs);
        assert!(sculpt_shown(f).is_some());
        let t = top(f);
        // Catmull-Clark rounds the cube a little: the bump over it
        assert!(t > 31.5, "{t}");
    }
}
