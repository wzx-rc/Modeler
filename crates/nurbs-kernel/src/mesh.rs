//! Meshes that live in the scene beside the kernel's part (Romain, 20 nov.
//! 2026: "l'application devienne multi-mode… on travaille sur une même
//! scène… les trois composants vont cohabiter"): a sculpted mesh (the
//! Sculpt tab, a dense triangle mesh moved by brushes) or a polygonal one
//! (the Poly tab, quads and n-gons edited by components). The kernel never
//! builds them: the app draws, picks and edits them.
//!
//! A mesh is a feature's `mesh` (`Feature::is_mesh`), its `prim` a
//! `Prim::Box` whose sizes scale it: its points are kept in the unit box
//! (x and z from −0.5 to 0.5, y from 0 to 1, the base's centre at the
//! origin), so a move, a turn, a resize and the scale cage work on it as on
//! a cube. `renormalized` fits the box to the points again after an edit.
//!
//! The data sits behind an `Arc`: a document copied for undo shares it, the
//! first edit after copies it once (`MeshRef::make_mut`).

use glam::Vec3;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshKind {
    /// Brushes (Sculpt tab): many small triangles.
    Sculpt,
    /// Components (Poly tab): quads and n-gons.
    Poly,
    /// The SDF tab: the mesh is the surface of its `sdf` scene, made
    /// again by dual contouring at each change of it (never edited itself).
    Sdf,
}

impl MeshKind {
    pub fn label(self) -> &'static str {
        match self {
            MeshKind::Sculpt => "Sculpt",
            MeshKind::Poly => "Poly",
            MeshKind::Sdf => "SDF",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshData {
    pub kind: MeshKind,
    /// Points in the unit box of the feature (see the module's notes).
    pub verts: Vec<[f32; 3]>,
    /// Polygons, corners counter-clockwise seen from outside.
    pub faces: Vec<Vec<u32>>,
    /// Poly: drawn smoothed (Catmull-Clark twice, Maya's smooth preview),
    /// the cage over it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub smooth: bool,
    /// Poly: the smoothing angle in degrees (Maya's Set Normal Angle, Romain
    /// 20 nov., 17 h 15): two faces meeting at an edge within it are shaded
    /// as one smooth surface (a bevel's rows, a sphere, a cylinder's side),
    /// past it the edge stays hard (a cube's). 0: every edge hard (facets),
    /// 180: every edge soft.
    #[serde(default = "default_angle", skip_serializing_if = "is_default_angle")]
    pub angle: f32,
    /// A linked surface (`patch.rs`): this mesh is its Poly layer, bound to
    /// the NURBS patch kept here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<crate::patch::Patch>,
    /// An SDF object (the SDF tab, Romain 20 nov. 2026: "reprendre le
    /// moteur SDF… la couleur serait le vert"): its shapes, combined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdf: Option<SdfModel>,
}

/// An SDF object's shapes (Forme SDF's scene) and where they sit in the
/// feature's frame. The scene is in millimetres; `offset` is where its
/// origin is in the feature's frame (the unit box is fitted to the surface
/// at each meshing, which moves the frame's base: `offset` keeps the shapes
/// where they are); `size` the box's sizes when it was last meshed (a scale
/// of the box since is taken into the shapes at the next meshing).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SdfModel {
    pub scene: forme_core::Scene,
    pub offset: [f32; 3],
    pub size: [f32; 3],
    /// The object's history marker (the history strip, as a NURBS
    /// element's): only its first `n` shapes are built, the others kept
    /// past it. None: all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker: Option<usize>,
}

impl SdfModel {
    /// A scene with nothing in it yet, its origin at the frame's.
    pub fn new() -> SdfModel {
        SdfModel { scene: forme_core::Scene::new(), offset: [0.0; 3], size: [0.0; 3], marker: None }
    }

    /// The shapes in order (depth first, groups left out).
    pub fn shapes(&self) -> Vec<forme_core::NodeId> {
        sdf_shapes(&self.scene)
    }

    /// The scene as built: the shapes past the marker left out.
    pub fn built_scene(&self) -> forme_core::Scene {
        let mut s = self.scene.clone();
        if let Some(m) = self.marker {
            for id in self.shapes().into_iter().skip(m) {
                if let Some(n) = s.node_mut(id) {
                    n.visible = false;
                }
            }
        }
        s
    }

    /// The scene's point to the feature's frame (mm).
    pub fn to_frame(&self, p: Vec3) -> Vec3 {
        p + Vec3::from(self.offset)
    }

    /// The feature's frame (mm) to the scene.
    pub fn to_scene(&self, p: Vec3) -> Vec3 {
        p - Vec3::from(self.offset)
    }

    /// The box's sizes changed by a scale since the last meshing (`now`):
    /// the shapes scaled with it (positions along each axis, sizes by the
    /// mean when the shape is turned, else along its axes), so the next
    /// meshing gives what the stretched mesh showed.
    pub fn take_scale(&mut self, now: Vec3) {
        let was = Vec3::from(self.size);
        if was.min_element() <= 1e-4 {
            self.size = now.to_array();
            return;
        }
        let r = now / was;
        if (r - Vec3::ONE).abs().max_element() < 1e-4 {
            return;
        }
        let mean = (r.x * r.y * r.z).cbrt();
        self.offset = (Vec3::from(self.offset) * r).to_array();
        for n in self.scene.nodes.values_mut() {
            n.transform.pos *= r;
            let turned = n.transform.rot.angle_between(glam::Quat::IDENTITY) > 1e-3;
            if let forme_core::NodeKind::Shape(sh) = &mut n.kind {
                use forme_core::PrimKind as K;
                let k = if turned { Vec3::splat(mean) } else { r };
                match sh.kind {
                    K::Box => sh.size *= k,
                    K::Sphere => sh.size *= mean,
                    // radius across (x, z), height along y
                    K::Cylinder | K::Capsule | K::Cone | K::Tube | K::Prism => {
                        let across = (k.x * k.z).sqrt();
                        sh.size = Vec3::new(sh.size.x * across, sh.size.y * k.y, sh.size.z * across);
                    }
                    _ => sh.size *= mean,
                }
                sh.round *= mean;
                sh.chamfer *= mean;
            }
            n.combine.blend *= mean;
        }
        self.size = now.to_array();
    }
}

impl Default for SdfModel {
    fn default() -> Self {
        SdfModel::new()
    }
}

/// A scene's shapes in order (depth first, groups left out, folded or not).
pub fn sdf_shapes(scene: &forme_core::Scene) -> Vec<forme_core::NodeId> {
    let mut out = vec![];
    fn walk(s: &forme_core::Scene, id: forme_core::NodeId, out: &mut Vec<forme_core::NodeId>, depth: usize) {
        let Some(n) = s.node(id) else { return };
        if depth > 64 {
            return;
        }
        for c in n.children() {
            if s.node(*c).map(|x| x.shape().is_some()).unwrap_or(false) {
                out.push(*c);
            }
            walk(s, *c, out, depth + 1);
        }
    }
    walk(scene, scene.root, &mut out, 0);
    out
}

/// Maya's default normal angle.
pub const DEFAULT_ANGLE: f32 = 30.0;

fn default_angle() -> f32 {
    DEFAULT_ANGLE
}

fn is_default_angle(a: &f32) -> bool {
    *a == DEFAULT_ANGLE
}

/// The mesh of a feature, shared until changed.
#[derive(Clone, Debug)]
pub struct MeshRef(pub Arc<MeshData>);

impl PartialEq for MeshRef {
    fn eq(&self, o: &MeshRef) -> bool {
        Arc::ptr_eq(&self.0, &o.0) || *self.0 == *o.0
    }
}

impl Serialize for MeshRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for MeshRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<MeshRef, D::Error> {
        Ok(MeshRef(Arc::new(MeshData::deserialize(d)?)))
    }
}

impl std::ops::Deref for MeshRef {
    type Target = MeshData;
    fn deref(&self) -> &MeshData {
        &self.0
    }
}

impl MeshRef {
    pub fn new(m: MeshData) -> MeshRef {
        MeshRef(Arc::new(m))
    }
    /// The data to change: copied first when a document kept for undo
    /// shares it.
    pub fn make_mut(&mut self) -> &mut MeshData {
        Arc::make_mut(&mut self.0)
    }
    /// Same data (no copy since): what the renderer keys its upload on.
    pub fn ptr(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
}

/// Points given in millimetres (the feature's own frame) put in their unit
/// box: the unit points, the box's sizes, and where its base's centre is
/// in that frame.
pub fn normalize(pts: &[Vec3]) -> (Vec<[f32; 3]>, Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for p in pts {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    if pts.is_empty() {
        return (vec![], Vec3::ONE, Vec3::ZERO);
    }
    let size = (hi - lo).max(Vec3::splat(1e-3));
    let base = Vec3::new((lo.x + hi.x) * 0.5, lo.y, (lo.z + hi.z) * 0.5);
    let unit = pts.iter().map(|p| ((*p - base) / size).to_array()).collect();
    (unit, size, base)
}

impl MeshData {
    /// Its points in millimetres for these sizes (the feature's frame).
    pub fn local(&self, size: Vec3) -> Vec<Vec3> {
        self.verts.iter().map(|v| Vec3::from(*v) * size).collect()
    }

    /// Fits the unit box to the points again (they may have left it):
    /// the new sizes and the base's move in the feature's frame.
    pub fn renormalized(&mut self, size: Vec3) -> (Vec3, Vec3) {
        let pts = self.local(size);
        self.store(&pts, size)
    }

    /// Its points set from millimetres (the frame, for these sizes), the
    /// unit box fitted to them; a linked surface's control points stay
    /// where they are. The new sizes and the base's move in the frame.
    pub fn store(&mut self, pts: &[Vec3], size: Vec3) -> (Vec3, Vec3) {
        let (unit, s, base) = normalize(pts);
        self.verts = unit;
        if let Some(p) = self.patch.as_mut() {
            p.remap(size, base, s);
        }
        (s, base)
    }

    /// Each polygon as triangles (a fan; n-gons are convex enough here),
    /// with the polygon each comes from.
    pub fn triangles(&self) -> (Vec<[u32; 3]>, Vec<u32>) {
        let mut tris = Vec::with_capacity(self.faces.len() * 2);
        let mut of = Vec::with_capacity(self.faces.len() * 2);
        for (fi, f) in self.faces.iter().enumerate() {
            for k in 1..f.len().saturating_sub(1) {
                tris.push([f[0], f[k], f[k + 1]]);
                of.push(fi as u32);
            }
        }
        (tris, of)
    }

    /// Every edge once (low index first).
    pub fn edges(&self) -> Vec<(u32, u32)> {
        let mut seen = HashSet::new();
        let mut out = vec![];
        for f in &self.faces {
            for i in 0..f.len() {
                let e = edge_key(f[i], f[(i + 1) % f.len()]);
                if seen.insert(e) {
                    out.push(e);
                }
            }
        }
        out
    }

    /// Smooth normals per point (the sculpt's shading), from the points
    /// in millimetres.
    pub fn vertex_normals(&self, pts: &[Vec3]) -> Vec<Vec3> {
        let mut n = vec![Vec3::ZERO; pts.len()];
        for f in &self.faces {
            let fn_ = poly_normal(pts, f);
            for &v in f {
                n[v as usize] += fn_;
            }
        }
        n.iter().map(|v| v.normalize_or(Vec3::Y)).collect()
    }

    /// Each polygon corner's shading normal (Maya's normal angle): a corner
    /// takes the faces round its point that are joined to its own face
    /// through soft edges, those whose two faces meet within `angle`
    /// degrees (half a degree of slack, so a 90° bevel in 3 rows of 30°
    /// is smooth at 30); a hard edge, an open one or one of three faces
    /// or more parts them. Area-weighted. One list per polygon, in its
    /// corners' order.
    pub fn corner_normals(&self, pts: &[Vec3], angle: f32) -> Vec<Vec<Vec3>> {
        let fnorm: Vec<Vec3> = self.faces.iter().map(|f| poly_normal(pts, f)).collect();
        let unit: Vec<Vec3> = fnorm.iter().map(|n| n.normalize_or_zero()).collect();
        let mut off = Vec::with_capacity(self.faces.len());
        let mut total = 0usize;
        for f in &self.faces {
            off.push(total);
            total += f.len();
        }
        let mut parent: Vec<usize> = (0..total).collect();
        fn root(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        let cos = (angle.clamp(0.0, 180.0) + 0.5).min(180.0).to_radians().cos();
        let mut edges: std::collections::HashMap<(u32, u32), Vec<(usize, usize)>> = std::collections::HashMap::new();
        for (i, f) in self.faces.iter().enumerate() {
            for k in 0..f.len() {
                edges.entry(edge_key(f[k], f[(k + 1) % f.len()])).or_default().push((i, k));
            }
        }
        for (e, fs) in &edges {
            if fs.len() != 2 {
                continue;
            }
            let ((f, _), (g, _)) = (fs[0], fs[1]);
            if f == g || unit[f] == Vec3::ZERO || unit[g] == Vec3::ZERO || unit[f].dot(unit[g]) < cos {
                continue;
            }
            for v in [e.0, e.1] {
                let (Some(a), Some(b)) = (self.faces[f].iter().position(|x| *x == v), self.faces[g].iter().position(|x| *x == v)) else { continue };
                let (ra, rb) = (root(&mut parent, off[f] + a), root(&mut parent, off[g] + b));
                if ra != rb {
                    parent[ra] = rb;
                }
            }
        }
        let mut sum = vec![Vec3::ZERO; total];
        for (i, f) in self.faces.iter().enumerate() {
            for k in 0..f.len() {
                let r = root(&mut parent, off[i] + k);
                sum[r] += fnorm[i];
            }
        }
        self.faces
            .iter()
            .enumerate()
            .map(|(i, f)| {
                (0..f.len())
                    .map(|k| {
                        let r = root(&mut parent, off[i] + k);
                        let n = sum[r].normalize_or_zero();
                        if n == Vec3::ZERO { unit[i].normalize_or(Vec3::Y) } else { n }
                    })
                    .collect()
            })
            .collect()
    }

    /// Each point's neighbours (along the polygons' sides).
    pub fn neighbours(&self) -> Vec<Vec<u32>> {
        let mut nb: Vec<Vec<u32>> = vec![vec![]; self.verts.len()];
        for f in &self.faces {
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                if !nb[a as usize].contains(&b) {
                    nb[a as usize].push(b);
                }
                if !nb[b as usize].contains(&a) {
                    nb[b as usize].push(a);
                }
            }
        }
        nb
    }

    /// Drops the points no polygon uses (indices renumbered).
    pub fn compact(&mut self) {
        let mut used = vec![false; self.verts.len()];
        for f in &self.faces {
            for &v in f {
                used[v as usize] = true;
            }
        }
        let mut map = vec![u32::MAX; self.verts.len()];
        let mut verts = vec![];
        for (i, u) in used.iter().enumerate() {
            if *u {
                map[i] = verts.len() as u32;
                verts.push(self.verts[i]);
            }
        }
        for f in &mut self.faces {
            for v in f.iter_mut() {
                *v = map[*v as usize];
            }
        }
        self.verts = verts;
    }
}

pub fn edge_key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

/// A polygon's normal (Newell's, length = twice its area).
pub fn poly_normal(pts: &[Vec3], f: &[u32]) -> Vec3 {
    let mut n = Vec3::ZERO;
    for i in 0..f.len() {
        let a = pts[f[i] as usize];
        let b = pts[f[(i + 1) % f.len()] as usize];
        n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    n
}

pub fn poly_centre(pts: &[Vec3], f: &[u32]) -> Vec3 {
    f.iter().map(|&v| pts[v as usize]).sum::<Vec3>() / f.len().max(1) as f32
}

// -------------------------------------------------------------- shapes
//
// In millimetres, standing on y = 0, `s` across.

/// A box of 8 points and 6 quads.
pub fn poly_cube(s: f32) -> (Vec<Vec3>, Vec<Vec<u32>>) {
    let h = s * 0.5;
    let p = (0..8).map(|i| Vec3::new(if i & 1 == 0 { -h } else { h }, if i & 2 == 0 { 0.0 } else { s }, if i & 4 == 0 { -h } else { h })).collect();
    // outward, counter-clockwise
    let f = vec![vec![0, 4, 6, 2], vec![1, 3, 7, 5], vec![0, 1, 5, 4], vec![2, 6, 7, 3], vec![0, 2, 3, 1], vec![4, 5, 7, 6]];
    (p, f)
}

/// A cylinder of `n` sides with n-gon caps.
pub fn poly_cylinder(s: f32, n: usize) -> (Vec<Vec3>, Vec<Vec<u32>>) {
    let r = s * 0.5;
    let mut p = vec![];
    for k in 0..n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        p.push(Vec3::new(r * a.cos(), 0.0, -r * a.sin()));
    }
    for k in 0..n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        p.push(Vec3::new(r * a.cos(), s, -r * a.sin()));
    }
    let n32 = n as u32;
    let mut f = vec![];
    for k in 0..n32 {
        let k1 = (k + 1) % n32;
        f.push(vec![k, k1, n32 + k1, n32 + k]);
    }
    f.push((0..n32).rev().collect());
    f.push((n32..2 * n32).collect());
    (p, f)
}

/// A flat grid of `n` × `n` quads facing up.
pub fn poly_plane(s: f32, n: usize) -> (Vec<Vec3>, Vec<Vec<u32>>) {
    let mut p = vec![];
    for j in 0..=n {
        for i in 0..=n {
            p.push(Vec3::new((i as f32 / n as f32 - 0.5) * s, 0.0, (j as f32 / n as f32 - 0.5) * s));
        }
    }
    let w = n as u32 + 1;
    let mut f = vec![];
    for j in 0..n as u32 {
        for i in 0..n as u32 {
            let a = j * w + i;
            f.push(vec![a, a + w, a + w + 1, a + 1]);
        }
    }
    (p, f)
}

/// A cube of `n` × `n` quads per side, its points pushed onto the sphere
/// (all quads, no poles).
pub fn quad_sphere(s: f32, n: usize, round: f32) -> (Vec<Vec3>, Vec<Vec<u32>>) {
    let mut pts: Vec<Vec3> = vec![];
    let mut index: HashMap<[i32; 3], u32> = HashMap::new();
    let mut faces = vec![];
    let n = n as i32;
    // each side: a fixed axis and sign, its grid along the other two
    for axis in 0..3 {
        for sign in [-1i32, 1] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let mut id = |i: i32, j: i32| {
                let mut g = [0i32; 3];
                g[axis] = sign * n;
                g[u] = 2 * i - n;
                g[v] = 2 * j - n;
                *index.entry(g).or_insert_with(|| {
                    pts.push(Vec3::new(g[0] as f32, g[1] as f32, g[2] as f32) / n as f32);
                    pts.len() as u32 - 1
                })
            };
            for j in 0..n {
                for i in 0..n {
                    let q = [id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)];
                    faces.push(if sign > 0 { q.to_vec() } else { vec![q[0], q[3], q[2], q[1]] });
                }
            }
        }
    }
    // the cube's points (−1..1) towards the sphere by `round`
    let pts = pts.iter().map(|c| c.lerp(c.normalize(), round) * (s * 0.5) + Vec3::new(0.0, s * 0.5, 0.0)).collect();
    (pts, faces)
}

/// An icosphere `s` across, subdivided `level` times: the sculpt's sphere
/// (level 6: 40 962 points, 81 920 triangles).
pub fn icosphere(s: f32, level: u32) -> (Vec<Vec3>, Vec<Vec<u32>>) {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut pts: Vec<Vec3> = [[-1.0, t, 0.0], [1.0, t, 0.0], [-1.0, -t, 0.0], [1.0, -t, 0.0], [0.0, -1.0, t], [0.0, 1.0, t], [0.0, -1.0, -t], [0.0, 1.0, -t], [t, 0.0, -1.0], [t, 0.0, 1.0], [-t, 0.0, -1.0], [-t, 0.0, 1.0]]
        .iter()
        .map(|p| Vec3::from(*p).normalize())
        .collect();
    let mut tris: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..level {
        let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
        let mut next = Vec::with_capacity(tris.len() * 4);
        for t in &tris {
            let mut m = |a: u32, b: u32| {
                *mid.entry(edge_key(a, b)).or_insert_with(|| {
                    pts.push(((pts[a as usize] + pts[b as usize]) * 0.5).normalize());
                    pts.len() as u32 - 1
                })
            };
            let (ab, bc, ca) = (m(t[0], t[1]), m(t[1], t[2]), m(t[2], t[0]));
            next.extend_from_slice(&[[t[0], ab, ca], [t[1], bc, ab], [t[2], ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    let r = s * 0.5;
    let pts = pts.iter().map(|p| *p * r + Vec3::new(0.0, r, 0.0)).collect();
    (pts, tris.iter().map(|t| t.to_vec()).collect())
}

/// Quads cut in two triangles (the sculpt takes triangles).
pub fn triangulated(faces: &[Vec<u32>]) -> Vec<Vec<u32>> {
    let mut out = vec![];
    for f in faces {
        for k in 1..f.len().saturating_sub(1) {
            out.push(vec![f[0], f[k], f[k + 1]]);
        }
    }
    out
}

// ------------------------------------------------------------ editing
//
// On points in millimetres (`MeshData::local`); the caller puts them back
// in the unit box (`normalize`).

/// Catmull-Clark once (open borders kept as creases).
pub fn catmull_clark(pts: &[Vec3], faces: &[Vec<u32>]) -> (Vec<Vec3>, Vec<Vec<u32>>) {
    let nv = pts.len();
    // face points
    let fp: Vec<Vec3> = faces.iter().map(|f| poly_centre(pts, f)).collect();
    // edges and the faces round each
    let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (fi, f) in faces.iter().enumerate() {
        for i in 0..f.len() {
            edge_faces.entry(edge_key(f[i], f[(i + 1) % f.len()])).or_default().push(fi);
        }
    }
    let mut edge_index: HashMap<(u32, u32), u32> = HashMap::new();
    let mut out: Vec<Vec3> = Vec::with_capacity(nv + edge_faces.len() + faces.len());
    out.extend_from_slice(pts);
    let mut keys: Vec<&(u32, u32)> = edge_faces.keys().collect();
    keys.sort();
    for e in keys {
        let fs = &edge_faces[e];
        let (a, b) = (pts[e.0 as usize], pts[e.1 as usize]);
        let p = if fs.len() == 2 { (a + b + fp[fs[0]] + fp[fs[1]]) * 0.25 } else { (a + b) * 0.5 };
        edge_index.insert(*e, out.len() as u32);
        out.push(p);
    }
    let face_base = out.len() as u32;
    out.extend_from_slice(&fp);
    // the old points moved
    let mut v_faces: Vec<Vec<usize>> = vec![vec![]; nv];
    let mut v_edges: Vec<Vec<(u32, u32)>> = vec![vec![]; nv];
    for (fi, f) in faces.iter().enumerate() {
        for &v in f {
            v_faces[v as usize].push(fi);
        }
    }
    for e in edge_faces.keys() {
        v_edges[e.0 as usize].push(*e);
        v_edges[e.1 as usize].push(*e);
    }
    for v in 0..nv {
        let p = pts[v];
        let border: Vec<&(u32, u32)> = v_edges[v].iter().filter(|e| edge_faces[*e].len() < 2).collect();
        if !border.is_empty() {
            if border.len() == 2 {
                let other = |e: &(u32, u32)| if e.0 as usize == v { pts[e.1 as usize] } else { pts[e.0 as usize] };
                out[v] = p * 0.75 + (other(border[0]) + other(border[1])) * 0.125;
            }
            continue;
        }
        let n = v_faces[v].len() as f32;
        if n < 3.0 {
            continue;
        }
        let f = v_faces[v].iter().map(|fi| fp[*fi]).sum::<Vec3>() / n;
        let r = v_edges[v].iter().map(|e| (pts[e.0 as usize] + pts[e.1 as usize]) * 0.5).sum::<Vec3>() / v_edges[v].len() as f32;
        out[v] = (f + r * 2.0 + p * (n - 3.0)) / n;
    }
    let mut nf = Vec::with_capacity(faces.len() * 4);
    for (fi, f) in faces.iter().enumerate() {
        let c = face_base + fi as u32;
        for i in 0..f.len() {
            let prev = f[(i + f.len() - 1) % f.len()];
            let (v, next) = (f[i], f[(i + 1) % f.len()]);
            nf.push(vec![v, edge_index[&edge_key(v, next)], c, edge_index[&edge_key(prev, v)]]);
        }
    }
    (out, nf)
}

/// Faces pulled out as one region along their points' mean normals by
/// `d` (Maya's Extrude with Keep Faces Together): the region's border gets
/// a wall of quads. Returns the region's faces (same indices, now on new
/// points) and the new points.
pub fn extrude_faces(pts: &mut Vec<Vec3>, faces: &mut Vec<Vec<u32>>, sel: &[u32], d: f32) -> Vec<u32> {
    let sel_set: HashSet<u32> = sel.iter().copied().collect();
    // the region's sides: used once by its faces (direction kept)
    let mut count: HashMap<(u32, u32), i32> = HashMap::new();
    for &fi in sel {
        let f = &faces[fi as usize];
        for i in 0..f.len() {
            *count.entry(edge_key(f[i], f[(i + 1) % f.len()])).or_default() += 1;
        }
    }
    let mut border: Vec<(u32, u32)> = vec![];
    for &fi in sel {
        let f = &faces[fi as usize];
        for i in 0..f.len() {
            let (a, b) = (f[i], f[(i + 1) % f.len()]);
            if count[&edge_key(a, b)] == 1 {
                border.push((a, b));
            }
        }
    }
    // each point of the region: a copy, moved along its faces' mean normal
    let mut normal: HashMap<u32, Vec3> = HashMap::new();
    for &fi in sel {
        let n = poly_normal(pts, &faces[fi as usize]).normalize_or_zero();
        for &v in &faces[fi as usize] {
            *normal.entry(v).or_default() += n;
        }
    }
    let mut copy: HashMap<u32, u32> = HashMap::new();
    let mut olds: Vec<u32> = normal.keys().copied().collect();
    olds.sort();
    for v in olds {
        let n = normal[&v].normalize_or_zero();
        copy.insert(v, pts.len() as u32);
        pts.push(pts[v as usize] + n * d);
    }
    for (fi, f) in faces.iter_mut().enumerate() {
        if sel_set.contains(&(fi as u32)) {
            for v in f.iter_mut() {
                *v = copy[v];
            }
        }
    }
    // the wall: for each border side a → b (the region's direction), the
    // quad a, b, b', a' (its a → b against the old neighbour's b → a)
    for (a, b) in border {
        faces.push(vec![a, b, copy[&b], copy[&a]]);
    }
    sel.to_vec()
}

/// Inset (I): the picked faces kept together (Maya's default, Keep Faces
/// Together): only the region's border comes in, each border point along
/// its two border sides' inward directions (a mitre), by `t` times the
/// smallest of the region's faces' half widths, so a strip of one face
/// never folds; a ring of quads joins the old border to the new one. The
/// region's inner points stay. The picked faces are given back (their
/// indices do not change).
pub fn inset_faces(pts: &mut Vec<Vec3>, faces: &mut Vec<Vec<u32>>, sel: &[u32], t: f32) -> Vec<u32> {
    let sel: Vec<u32> = {
        let mut s: Vec<u32> = sel.iter().copied().filter(|f| (*f as usize) < faces.len()).collect();
        s.sort();
        s.dedup();
        s
    };
    if sel.is_empty() {
        return vec![];
    }
    // the region's border: its sides used once by its faces, in their
    // faces' direction, with the face each belongs to
    let mut count: HashMap<(u32, u32), i32> = HashMap::new();
    for &fi in &sel {
        let f = &faces[fi as usize];
        for i in 0..f.len() {
            *count.entry(edge_key(f[i], f[(i + 1) % f.len()])).or_default() += 1;
        }
    }
    let mut border: Vec<(u32, u32, u32)> = vec![];
    for &fi in &sel {
        let f = &faces[fi as usize];
        for i in 0..f.len() {
            let (a, b) = (f[i], f[(i + 1) % f.len()]);
            if count[&edge_key(a, b)] == 1 {
                border.push((a, b, fi));
            }
        }
    }
    // how far: t times the smallest half width of the region's faces (the
    // distance from a face's centre to its nearest side)
    let mut half = f32::MAX;
    for &fi in &sel {
        let f = &faces[fi as usize];
        let c = poly_centre(pts, f);
        for i in 0..f.len() {
            let (a, b) = (pts[f[i] as usize], pts[f[(i + 1) % f.len()] as usize]);
            let ab = b - a;
            let l2 = ab.length_squared();
            if l2 > 1e-12 {
                let k = ((c - a).dot(ab) / l2).clamp(0.0, 1.0);
                half = half.min((a + ab * k).distance(c));
            }
        }
    }
    if !half.is_finite() || half <= 0.0 {
        return sel;
    }
    let d = t.clamp(0.0, 0.98) * half;
    // each border side's inward way, in its face's plane
    let inward = |pts: &Vec<Vec3>, faces: &Vec<Vec<u32>>, a: u32, b: u32, fi: u32| -> Vec3 {
        let n = poly_normal(pts, &faces[fi as usize]).normalize_or_zero();
        n.cross(pts[b as usize] - pts[a as usize]).normalize_or_zero()
    };
    // a border point's sides: the one coming in, the one going out
    let mut into: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut out_of: HashMap<u32, Vec<usize>> = HashMap::new();
    for (k, (a, b, _)) in border.iter().enumerate() {
        out_of.entry(*a).or_default().push(k);
        into.entry(*b).or_default().push(k);
    }
    let mut moved: HashMap<u32, u32> = HashMap::new();
    let mut verts: Vec<u32> = border.iter().map(|e| e.0).collect();
    verts.sort();
    verts.dedup();
    for v in verts {
        let ins: Vec<Vec3> = into.get(&v).into_iter().flatten().chain(out_of.get(&v).into_iter().flatten()).map(|k| {
            let (a, b, fi) = border[*k];
            inward(pts, faces, a, b, fi)
        }).collect();
        let (i1, i2) = match (into.get(&v).and_then(|k| k.first()), out_of.get(&v).and_then(|k| k.first())) {
            (Some(k1), Some(k2)) => {
                let (a, b, f1) = border[*k1];
                let (c, e, f2) = border[*k2];
                (inward(pts, faces, a, b, f1), inward(pts, faces, c, e, f2))
            }
            _ => {
                let m = ins.iter().copied().sum::<Vec3>().normalize_or_zero();
                (m, m)
            }
        };
        // the mitre: d along each side's inward way, at most three times d
        let dot = i1.dot(i2).clamp(-1.0, 1.0);
        let mut m = if dot > -0.95 { (i1 + i2) * (d / (1.0 + dot)) } else { (i1 - i2).normalize_or_zero() * d };
        if m.length() > 3.0 * d {
            m = m.normalize_or_zero() * 3.0 * d;
        }
        let at = pts[v as usize] + m;
        pts.push(at);
        moved.insert(v, pts.len() as u32 - 1);
    }
    // the ring joining the old border to the new one
    for (a, b, _) in &border {
        faces.push(vec![*a, *b, moved[b], moved[a]]);
    }
    // the region's faces take the new border points
    for &fi in &sel {
        for v in faces[fi as usize].iter_mut() {
            if let Some(n) = moved.get(v) {
                *v = *n;
            }
        }
    }
    sel
}

/// Takes the faces out (their points too when nothing else uses them).
pub fn delete_faces(m: &mut MeshData, sel: &[u32]) {
    let gone: HashSet<u32> = sel.iter().copied().collect();
    let mut i = 0u32;
    m.faces.retain(|_| {
        let keep = !gone.contains(&i);
        i += 1;
        keep
    });
    m.compact();
}

/// The ring of quads across an edge (Maya's Insert Edge Loop): from each
/// side, the quad's opposite side, on until a face that is not a quad,
/// an open border, or back to the start. The edges crossed, in order.
pub fn edge_ring(faces: &[Vec<u32>], start: (u32, u32)) -> (Vec<(u32, u32)>, bool) {
    let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (fi, f) in faces.iter().enumerate() {
        for i in 0..f.len() {
            edge_faces.entry(edge_key(f[i], f[(i + 1) % f.len()])).or_default().push(fi);
        }
    }
    let start = edge_key(start.0, start.1);
    let Some(first) = edge_faces.get(&start) else { return (vec![], false) };
    let opposite = |f: &Vec<u32>, e: (u32, u32)| -> Option<(u32, u32)> {
        if f.len() != 4 {
            return None;
        }
        let i = (0..4).find(|&i| edge_key(f[i], f[(i + 1) % 4]) == e)?;
        Some(edge_key(f[(i + 2) % 4], f[(i + 3) % 4]))
    };
    let mut ring = vec![start];
    let mut closed = false;
    for (side, &f0) in first.iter().enumerate().take(2) {
        let mut seq = vec![];
        let (mut e, mut f) = (start, f0);
        let mut seen: HashSet<usize> = HashSet::new();
        loop {
            if !seen.insert(f) {
                break;
            }
            let Some(o) = opposite(&faces[f], e) else { break };
            if o == start {
                closed = true;
                break;
            }
            seq.push(o);
            let Some(next) = edge_faces[&o].iter().copied().find(|g| *g != f) else { break };
            e = o;
            f = next;
        }
        if closed {
            ring.extend(seq);
            break;
        }
        if side == 0 {
            seq.reverse();
            seq.extend(ring);
            ring = seq;
        } else {
            ring.extend(seq);
        }
    }
    (ring, closed)
}

/// An edge loop across the ring of `start` (Maya's Insert Edge Loop, at
/// the middle): each edge of the ring cut at its middle, each quad of the
/// ring cut in two; a face at the ring's end (not a quad) gets the new
/// point in its side. Returns the new loop's edges.
pub fn loop_cut(pts: &mut Vec<Vec3>, faces: &mut Vec<Vec<u32>>, start: (u32, u32)) -> Vec<(u32, u32)> {
    loop_cut_at(pts, faces, start, 0.5)
}

/// The same at `t` along `start` (from its first point), each edge of the
/// ring cut at the same fraction running the same way (Maya's Multi-Cut
/// with Ctrl).
pub fn loop_cut_at(pts: &mut Vec<Vec3>, faces: &mut Vec<Vec<u32>>, start: (u32, u32), t: f32) -> Vec<(u32, u32)> {
    let (ring, closed) = edge_ring(faces, start);
    // each ring edge the same way round as `start`: across a quad, its
    // side x0 → x1 runs as the opposite side's x3 → x2
    let mut sides: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (fi, f) in faces.iter().enumerate() {
        for i in 0..f.len() {
            sides.entry(edge_key(f[i], f[(i + 1) % f.len()])).or_default().push(fi);
        }
    }
    let pos = ring.iter().position(|e| *e == edge_key(start.0, start.1)).unwrap_or(0);
    let mut dirs: Vec<(u32, u32)> = ring.clone();
    dirs[pos] = start;
    let across = |from: (u32, u32), to: (u32, u32)| -> Option<(u32, u32)> {
        let shared: Vec<usize> = sides.get(&edge_key(from.0, from.1))?.iter().copied().filter(|f| sides.get(&to).map(|v| v.contains(f)).unwrap_or(false)).collect();
        let q = &faces[*shared.first()?];
        if q.len() != 4 {
            return None;
        }
        let i = (0..4).find(|&i| edge_key(q[i], q[(i + 1) % 4]) == edge_key(from.0, from.1))?;
        Some(if q[i] == from.0 { (q[(i + 3) % 4], q[(i + 2) % 4]) } else { (q[(i + 2) % 4], q[(i + 3) % 4]) })
    };
    for i in pos + 1..dirs.len() {
        if let Some(d) = across(dirs[i - 1], ring[i]) {
            dirs[i] = d;
        }
    }
    for i in (0..pos).rev() {
        if let Some(d) = across(dirs[i + 1], ring[i]) {
            dirs[i] = d;
        }
    }
    let _ = closed;
    let mut cut: HashMap<(u32, u32), u32> = HashMap::new();
    for d in &dirs {
        cut.insert(edge_key(d.0, d.1), pts.len() as u32);
        pts.push(pts[d.0 as usize].lerp(pts[d.1 as usize], t));
    }
    let mut out = vec![];
    let mut new_faces: Vec<Vec<u32>> = vec![];
    for q in faces.iter_mut() {
        let n = q.len();
        let hits: Vec<usize> = (0..n).filter(|&i| cut.contains_key(&edge_key(q[i], q[(i + 1) % n]))).collect();
        if n == 4 && hits.len() == 2 && hits[1] - hits[0] == 2 {
            let i = hits[0];
            let (c0, c1) = (cut[&edge_key(q[i], q[(i + 1) % 4])], cut[&edge_key(q[(i + 2) % 4], q[(i + 3) % 4])]);
            let a = vec![q[i], c0, c1, q[(i + 3) % 4]];
            let b = vec![c0, q[(i + 1) % 4], q[(i + 2) % 4], c1];
            out.push(edge_key(c0, c1));
            *q = a;
            new_faces.push(b);
        } else if !hits.is_empty() {
            // a face at the ring's end: the new points in its sides
            let mut r = vec![];
            for i in 0..n {
                r.push(q[i]);
                if let Some(k) = cut.get(&edge_key(q[i], q[(i + 1) % n])) {
                    r.push(*k);
                }
            }
            *q = r;
        }
    }
    faces.extend(new_faces);
    out
}

/// A new point on the edge a–b at `t` from a, put in every face along it
/// (Maya's Multi-Cut clicking an edge). Returns it.
pub fn split_edge(pts: &mut Vec<Vec3>, faces: &mut [Vec<u32>], a: u32, b: u32, t: f32) -> u32 {
    let id = pts.len() as u32;
    pts.push(pts[a as usize].lerp(pts[b as usize], t));
    for f in faces.iter_mut() {
        let n = f.len();
        if let Some(i) = (0..n).find(|&i| (f[i] == a && f[(i + 1) % n] == b) || (f[i] == b && f[(i + 1) % n] == a)) {
            f.insert(i + 1, id);
        }
    }
    id
}

/// The face holding both points, not as neighbours (a cut can join them).
pub fn common_face(faces: &[Vec<u32>], a: u32, b: u32) -> Option<usize> {
    if a == b {
        return None;
    }
    faces.iter().position(|f| {
        let n = f.len();
        match (f.iter().position(|&x| x == a), f.iter().position(|&x| x == b)) {
            (Some(i), Some(j)) => (i + 1) % n != j && (j + 1) % n != i,
            _ => false,
        }
    })
}

/// The face holding a and b cut in two along a–b: it keeps the half from a
/// to b, the other half is added. Returns that new face's index.
pub fn split_face(faces: &mut Vec<Vec<u32>>, a: u32, b: u32) -> Option<usize> {
    let fi = common_face(faces, a, b)?;
    let f = faces[fi].clone();
    let n = f.len();
    let ia = f.iter().position(|&x| x == a)?;
    let ib = f.iter().position(|&x| x == b)?;
    let walk = |from: usize, to: usize| -> Vec<u32> {
        let mut h = vec![];
        let mut i = from;
        loop {
            h.push(f[i]);
            if i == to {
                break;
            }
            i = (i + 1) % n;
        }
        h
    };
    faces[fi] = walk(ia, ib);
    faces.push(walk(ib, ia));
    Some(faces.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed(faces: &[Vec<u32>]) -> bool {
        let mut c: HashMap<(u32, u32), i32> = HashMap::new();
        for f in faces {
            for i in 0..f.len() {
                *c.entry(edge_key(f[i], f[(i + 1) % f.len()])).or_default() += 1;
            }
        }
        c.values().all(|n| *n == 2)
    }

    fn outward(pts: &[Vec3], faces: &[Vec<u32>]) -> bool {
        let c = pts.iter().sum::<Vec3>() / pts.len() as f32;
        faces.iter().all(|f| poly_normal(pts, f).dot(poly_centre(pts, f) - c) > 0.0)
    }

    /// The largest angle (degrees) between the shading normals of one point.
    fn spread(m: &MeshData, pts: &[Vec3], angle: f32) -> f32 {
        let cn = m.corner_normals(pts, angle);
        let mut at: HashMap<u32, Vec<Vec3>> = HashMap::new();
        for (f, ns) in m.faces.iter().zip(&cn) {
            for (v, n) in f.iter().zip(ns) {
                at.entry(*v).or_default().push(*n);
            }
        }
        let mut worst = 0.0f32;
        for ns in at.values() {
            for a in ns {
                for b in ns {
                    worst = worst.max(a.dot(*b).clamp(-1.0, 1.0).acos().to_degrees());
                }
            }
        }
        worst
    }

    #[test]
    fn smoothing_angle() {
        let data = |p: &[Vec3], f: Vec<Vec<u32>>| MeshData { kind: MeshKind::Poly, verts: p.iter().map(|v| v.to_array()).collect(), faces: f, smooth: false, angle: DEFAULT_ANGLE, patch: None, sdf: None };
        // a cube: hard at 30 (each corner its face's normal), soft at 180
        let (p, f) = poly_cube(20.0);
        let m = data(&p, f);
        for (q, ns) in m.faces.iter().zip(m.corner_normals(&p, 30.0)) {
            let n = poly_normal(&p, q).normalize();
            assert!(ns.iter().all(|c| c.dot(n) > 0.9999));
        }
        assert!(spread(&m, &p, 180.0) < 0.1);
        assert!((spread(&m, &p, 0.0) - 90.0).abs() < 0.1);
        // a cylinder of 16: its side soft (22.5°), its caps hard
        let (p, f) = poly_cylinder(20.0, 16);
        let m = data(&p, f);
        let cn = m.corner_normals(&p, 30.0);
        for (q, ns) in m.faces.iter().zip(&cn) {
            let n = poly_normal(&p, q).normalize();
            if n.y.abs() > 0.99 {
                assert!(ns.iter().all(|c| c.dot(n) > 0.9999), "a cap shaded flat");
            } else {
                assert!(ns.iter().all(|c| c.y.abs() < 1e-4), "the side shaded round, square to the axis");
            }
        }
        // a sphere: one normal per point
        let (p, f) = quad_sphere(20.0, 4, 1.0);
        let m = data(&p, f);
        assert!(spread(&m, &p, 30.0) < 0.1);
        // a cube bevelled round in 4 rows: smooth all over at 30, hard at 10
        let (mut p, mut f) = poly_cube(20.0);
        let mut all = vec![];
        for q in &f {
            for i in 0..q.len() {
                let e = edge_key(q[i], q[(i + 1) % q.len()]);
                if !all.contains(&e) {
                    all.push(e);
                }
            }
        }
        crate::mesh_bevel::bevel(&mut p, &mut f, &all, crate::mesh_bevel::BevelOptions { width: 3.0, segments: 4, chamfer: false }).unwrap();
        let m = data(&p, f);
        assert!(spread(&m, &p, 30.0) < 0.1, "{}", spread(&m, &p, 30.0));
        assert!(spread(&m, &p, 10.0) > 10.0);
    }

    #[test]
    fn shapes_closed_and_outward() {
        for (p, f) in [poly_cube(20.0), poly_cylinder(20.0, 16), quad_sphere(20.0, 4, 1.0), icosphere(20.0, 2)] {
            assert!(closed(&f));
            assert!(outward(&p, &f));
        }
        let (p, f) = icosphere(20.0, 6);
        assert_eq!((p.len(), f.len()), (40962, 81920));
        let (p, f) = poly_plane(20.0, 4);
        assert_eq!((p.len(), f.len()), (25, 16));
    }

    #[test]
    fn subdivide_cube() {
        let (p, f) = poly_cube(20.0);
        let (p2, f2) = catmull_clark(&p, &f);
        assert_eq!((p2.len(), f2.len()), (26, 24));
        assert!(closed(&f2) && outward(&p2, &f2));
        // smoothed: the corners come in
        assert!(p2[7].x < 10.0 && p2[7].x > 5.0);
    }

    #[test]
    fn extrude_inset_delete() {
        let (mut p, mut f) = poly_cube(20.0);
        // the top (+Y) face is index 3
        extrude_faces(&mut p, &mut f, &[3], 10.0);
        assert!(closed(&f) && outward(&p, &f));
        assert_eq!(f.len(), 10);
        let top = poly_centre(&p, &f[3]);
        assert!((top.y - 30.0).abs() < 1e-4);
        inset_faces(&mut p, &mut f, &[3], 0.25);
        assert!(closed(&f) && outward(&p, &f));
        let mut m = MeshData { kind: MeshKind::Poly, verts: p.iter().map(|v| v.to_array()).collect(), faces: f, smooth: false, angle: DEFAULT_ANGLE, patch: None, sdf: None };
        delete_faces(&mut m, &[3]);
        assert_eq!(m.faces.len(), 13);
        assert_eq!(m.verts.len(), 16);
    }

    #[test]
    fn inset_keeps_faces_together() {
        // the plane's four middle cells: one border round them, their
        // middle point kept
        let (mut p, mut f) = poly_plane(20.0, 4);
        let sel: Vec<u32> = (0..f.len() as u32).filter(|i| {
            let c = poly_centre(&p, &f[*i as usize]);
            c.x.abs() < 5.0 && c.z.abs() < 5.0
        }).collect();
        assert_eq!(sel.len(), 4);
        let (np, nf) = (p.len(), f.len());
        let up = |p: &Vec<Vec3>, f: &Vec<Vec<u32>>| f.iter().all(|q| poly_normal(p, q).y > 0.0);
        let inner = inset_faces(&mut p, &mut f, &sel, 0.5);
        assert_eq!(inner, sel);
        assert_eq!(p.len(), np + 8);
        assert_eq!(f.len(), nf + 8);
        assert!(up(&p, &f));
        let centre = p.iter().position(|q| q.length() < 1e-5).unwrap() as u32;
        assert!(sel.iter().all(|i| f[*i as usize].contains(&centre)));
        // 2.5 mm half cells, t = 0.5: the border in by 1.25 mm, corners too
        let mut xs: Vec<f32> = sel.iter().flat_map(|i| f[*i as usize].iter().map(|v| p[*v as usize].x.abs())).collect();
        xs.sort_by(|a, b| a.total_cmp(b));
        xs.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        assert_eq!(xs.len(), 2);
        assert!(xs[0].abs() < 1e-4 && (xs[1] - 3.75).abs() < 1e-4, "{xs:?}");
        // a strip one face wide never folds, even at its fullest
        let (mut p, mut f) = poly_plane(20.0, 4);
        let row: Vec<u32> = (0..f.len() as u32).filter(|i| {
            let c = poly_centre(&p, &f[*i as usize]);
            c.z > 0.0 && c.z < 5.0
        }).collect();
        assert_eq!(row.len(), 4);
        inset_faces(&mut p, &mut f, &row, 0.95);
        assert!(up(&p, &f));
    }

    #[test]
    fn loop_round_a_cube() {
        let (mut p, mut f) = poly_cube(20.0);
        // a vertical edge: the ring goes round the four sides
        let (ring, closed_ring) = edge_ring(&f, (0, 2));
        assert!(closed_ring);
        assert_eq!(ring.len(), 4);
        let new = loop_cut(&mut p, &mut f, (0, 2));
        assert_eq!(new.len(), 4);
        assert_eq!(f.len(), 10);
        assert!(closed(&f) && outward(&p, &f));
        // the new points at mid-height
        assert!(p[8..].iter().all(|q| (q.y - 10.0).abs() < 1e-4));
    }

    #[test]
    fn cut_across_faces() {
        let (mut p, mut f) = poly_cube(20.0);
        // a point on two edges of the front face (z = +10: 4 5 7 6 or so), joined
        let front = f.iter().position(|q| q.iter().all(|v| p[*v as usize].z > 9.0)).unwrap();
        let q = f[front].clone();
        let a = split_edge(&mut p, &mut f, q[0], q[1], 0.3);
        let b = split_edge(&mut p, &mut f, q[2], q[3], 0.6);
        assert!(closed(&f));
        let nf = split_face(&mut f, a, b).unwrap();
        assert_eq!(f.len(), 7);
        assert!(closed(&f) && outward(&p, &f));
        assert_eq!(f[nf].len(), 4);
        // neighbours cannot be joined
        assert!(split_face(&mut f, a, q[1]).is_none());
        // a loop at a quarter, the same way round
        let (mut p, mut f) = poly_cube(20.0);
        let new = loop_cut_at(&mut p, &mut f, (0, 2), 0.25);
        assert_eq!(new.len(), 4);
        assert!(closed(&f) && outward(&p, &f));
        assert!(p[8..].iter().all(|q| (q.y - 5.0).abs() < 1e-4));
    }

    #[test]
    fn unit_box() {
        let (p, _) = poly_cube(20.0);
        let (u, s, base) = normalize(&p);
        assert!((s - Vec3::splat(20.0)).length() < 1e-4);
        assert!(base.length() < 1e-4);
        assert!(u.iter().all(|v| v[1] >= 0.0 && v[1] <= 1.0 && v[0].abs() <= 0.5));
    }
}
