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
}

impl MeshKind {
    pub fn label(self) -> &'static str {
        match self {
            MeshKind::Sculpt => "Sculpt",
            MeshKind::Poly => "Poly",
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
        let (unit, s, base) = normalize(&self.local(size));
        self.verts = unit;
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

/// Each face moved in towards its centre by the fraction `t` (Maya's
/// Inset, faces apart): a ring of quads round each. Returns the inner faces.
pub fn inset_faces(pts: &mut Vec<Vec3>, faces: &mut Vec<Vec<u32>>, sel: &[u32], t: f32) -> Vec<u32> {
    let mut inner = vec![];
    for &fi in sel {
        let f = faces[fi as usize].clone();
        let c = poly_centre(pts, &f);
        let base = pts.len() as u32;
        for &v in &f {
            let p = pts[v as usize];
            pts.push(p.lerp(c, t));
        }
        let n = f.len() as u32;
        for i in 0..n {
            let j = (i + 1) % n;
            faces.push(vec![f[i as usize], f[j as usize], base + j, base + i]);
        }
        faces[fi as usize] = (0..n).map(|i| base + i).collect();
        inner.push(fi);
    }
    inner
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
    let (ring, _closed) = edge_ring(faces, start);
    let mut cut: HashMap<(u32, u32), u32> = HashMap::new();
    for e in &ring {
        cut.insert(*e, pts.len() as u32);
        pts.push((pts[e.0 as usize] + pts[e.1 as usize]) * 0.5);
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
        let mut m = MeshData { kind: MeshKind::Poly, verts: p.iter().map(|v| v.to_array()).collect(), faces: f, smooth: false };
        delete_faces(&mut m, &[3]);
        assert_eq!(m.faces.len(), 13);
        assert_eq!(m.verts.len(), 16);
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
    fn unit_box() {
        let (p, _) = poly_cube(20.0);
        let (u, s, base) = normalize(&p);
        assert!((s - Vec3::splat(20.0)).length() < 1e-4);
        assert!(base.length() < 1e-4);
        assert!(u.iter().all(|v| v[1] >= 0.0 && v[1] <= 1.0 && v[0].abs() <= 0.5));
    }
}
