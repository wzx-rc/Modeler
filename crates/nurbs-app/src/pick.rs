//! Picking on the part's mesh: the ray hits a triangle, the triangle
//! belongs to a B-rep face, the face carries its tag (which feature, which
//! face of it). Exact on the kernel's triangles.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use glam::Vec3;
use nurbs_kernel::Model;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// Distance along the ray.
    pub t: f32,
    pub point: Vec3,
    /// Index in `Model::faces`.
    pub face: usize,
    /// The triangle's normal, towards the ray's origin.
    pub normal: Vec3,
    /// The mesh's normals at the hit, blended (a curved face's own way
    /// there, not its facet's), towards the ray's origin.
    pub smooth: Vec3,
}

/// The mesh's vertex normals of a triangle blended at a point on it,
/// turned towards `towards`' side (the facet's normal when they are
/// missing).
fn smooth_normal(m: &Model, tri: [u32; 3], p: Vec3, facet: Vec3) -> Vec3 {
    let at = |i: u32| Vec3::from(m.positions[i as usize]);
    let (a, b, c) = (at(tri[0]), at(tri[1]), at(tri[2]));
    let n = (b - a).cross(c - a);
    let area = n.length_squared();
    if area <= 0.0 || m.normals.len() != m.positions.len() {
        return facet;
    }
    let wa = (b - p).cross(c - p).dot(n) / area;
    let wb = (c - p).cross(a - p).dot(n) / area;
    let wc = 1.0 - wa - wb;
    let nn = |i: u32| Vec3::from(m.normals[i as usize]);
    let s = (nn(tri[0]) * wa + nn(tri[1]) * wb + nn(tri[2]) * wc).normalize_or_zero();
    if s == Vec3::ZERO {
        facet
    } else if s.dot(facet) < 0.0 {
        -s
    } else {
        s
    }
}

/// Per face: its triangles (contiguous, the kernel writes them face by
/// face) and their bounds. The rays go through a tree of boxes over all the
/// triangles (`Bvh`), built at the first ray on this model: a heavy scene
/// (Romain's of 21 nov.: 174 k triangles, 362 faces) took a face's every
/// triangle at each ray crossing its box, so Vertex mode's hover cost 70 ms
/// a frame.
#[derive(Default)]
pub struct Picker {
    ranges: Vec<(u32, u32)>,
    boxes: Vec<(Vec3, Vec3)>,
    /// The faces carrying each packed tag (after a Mirror or an Array
    /// several faces share one).
    by_tag: HashMap<i64, Vec<u32>>,
    bvh: OnceCell<Bvh>,
    /// The tree being built in a thread (`start`).
    job: RefCell<Option<Receiver<Bvh>>>,
    /// Rays asked of this model so far without the tree.
    asked: Cell<u32>,
    verts: OnceCell<Vec<Vec3>>,
}

/// Rays answered face by face before the tree is waited for (or built, when
/// no thread makes it): a model that lives one frame of a drag never pays
/// for one.
const PATIENCE: u32 = 32;
/// Under this many triangles the tree is built when needed, not in a thread.
const SMALL: usize = 20_000;

impl Picker {
    pub fn new(m: &Model) -> Picker {
        let mut p = Picker { ranges: vec![(u32::MAX, 0); m.faces.len()], boxes: vec![(Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)); m.faces.len()], ..Default::default() };
        for (t, tri) in m.triangles.iter().enumerate() {
            let f = m.tri_face[t] as usize;
            if f >= p.ranges.len() {
                continue;
            }
            let r = &mut p.ranges[f];
            r.0 = r.0.min(t as u32);
            r.1 = r.1.max(t as u32 + 1);
            let b = &mut p.boxes[f];
            for &v in tri {
                let q = Vec3::from(m.positions[v as usize]);
                b.0 = b.0.min(q);
                b.1 = b.1.max(q);
            }
        }
        for (f, face) in m.faces.iter().enumerate() {
            if let Some(t) = face.tag {
                p.by_tag.entry(t.pack()).or_default().push(f as u32);
            }
        }
        p
    }

    /// Starts the tree for this model in a thread (a heavy part's takes
    /// tens of ms: not on the interface's frame). Until it is there the
    /// rays go face by face.
    pub fn start(&self, m: &Arc<Model>) {
        if m.triangles.len() < SMALL || self.bvh.get().is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (m, faces) = (m.clone(), self.ranges.len());
        std::thread::spawn(move || {
            let _ = tx.send(Bvh::new(&m, faces));
        });
        *self.job.borrow_mut() = Some(rx);
    }

    /// The tree when it is there; after `PATIENCE` rays without it, waited
    /// for or built here.
    fn tree(&self, m: &Model) -> Option<&Bvh> {
        if self.bvh.get().is_none() {
            let n = self.asked.get() + 1;
            self.asked.set(n);
            let mut job = self.job.borrow_mut();
            let got = match job.as_ref() {
                Some(rx) if n > PATIENCE => rx.recv().ok().or_else(|| Some(Bvh::new(m, self.ranges.len()))),
                Some(rx) => rx.try_recv().ok(),
                None if n > PATIENCE => Some(Bvh::new(m, self.ranges.len())),
                None => None,
            };
            if let Some(b) = got {
                *job = None;
                let _ = self.bvh.set(b);
            }
        }
        self.bvh.get()
    }

    /// Nearest hit along the ray.
    pub fn pick(&self, m: &Model, ro: Vec3, rd: Vec3) -> Option<Hit> {
        let Some(tree) = self.tree(m) else { return self.pick_by_face(m, ro, rd) };
        let (t, d) = tree.ray(m, ro, rd)?;
        let tri = m.triangles[t as usize];
        let p0 = Vec3::from(m.positions[tri[0] as usize]);
        let p1 = Vec3::from(m.positions[tri[1] as usize]);
        let p2 = Vec3::from(m.positions[tri[2] as usize]);
        let mut n = (p1 - p0).cross(p2 - p0).normalize_or_zero();
        if n.dot(rd) > 0.0 {
            n = -n;
        }
        let point = ro + rd * d;
        Some(Hit { t: d, point, face: m.tri_face[t as usize] as usize, normal: n, smooth: smooth_normal(m, tri, point, n) })
    }

    /// Nearest hit along the ray, face by face (no tree yet).
    fn pick_by_face(&self, m: &Model, ro: Vec3, rd: Vec3) -> Option<Hit> {
        let inv = Vec3::new(1.0 / rd.x, 1.0 / rd.y, 1.0 / rd.z);
        let mut best: Option<Hit> = None;
        for (f, (&(a, b), &(lo, hi))) in self.ranges.iter().zip(&self.boxes).enumerate() {
            if a >= b {
                continue;
            }
            // slab test against the face's bounds (a little margin)
            let t0 = (lo - Vec3::splat(1e-3) - ro) * inv;
            let t1 = (hi + Vec3::splat(1e-3) - ro) * inv;
            let tmin = t0.min(t1).max_element();
            let tmax = t0.max(t1).min_element();
            if tmax < tmin.max(0.0) || best.map(|h| tmin > h.t).unwrap_or(false) {
                continue;
            }
            for t in a..b {
                if m.tri_face[t as usize] as usize != f {
                    continue;
                }
                let tri = m.triangles[t as usize];
                let p0 = Vec3::from(m.positions[tri[0] as usize]);
                let p1 = Vec3::from(m.positions[tri[1] as usize]);
                let p2 = Vec3::from(m.positions[tri[2] as usize]);
                if let Some(d) = ray_tri(ro, rd, p0, p1, p2) {
                    if best.map(|h| d < h.t).unwrap_or(true) {
                        let mut n = (p1 - p0).cross(p2 - p0).normalize_or_zero();
                        if n.dot(rd) > 0.0 {
                            n = -n;
                        }
                        let point = ro + rd * d;
                        best = Some(Hit { t: d, point, face: f, normal: n, smooth: smooth_normal(m, tri, point, n) });
                    }
                }
            }
        }
        best
    }

    /// Among the faces carrying the packed tag `tag`, the one whose
    /// triangles come nearest `at` (the only one when there is one).
    pub fn face_near(&self, m: &Model, tag: i64, at: Vec3) -> Option<usize> {
        let same = self.by_tag.get(&tag)?;
        if same.len() <= 1 {
            return same.first().map(|f| *f as usize);
        }
        let mut best: Option<(usize, f32)> = None;
        for &f in same {
            let f = f as usize;
            let (lo, hi) = self.boxes[f];
            // (no nearer than its box)
            let gap = (lo - at).max(at - hi).max(Vec3::ZERO).length();
            if best.is_some_and(|b| gap >= b.1) {
                continue;
            }
            let (a, b) = self.ranges[f];
            let mut d = f32::MAX;
            for t in a..b.max(a) {
                if m.tri_face[t as usize] as usize != f {
                    continue;
                }
                d = d.min(crate::faces::point_triangle(at, m.triangles[t as usize].map(|v| Vec3::from(m.positions[v as usize]))));
            }
            if best.map_or(true, |b| d < b.1) {
                best = Some((f, d));
            }
        }
        best.map(|b| b.0)
    }

    /// A face's triangles: a range of `Model::triangles` (those of another
    /// face inside it are not its own).
    pub fn range(&self, face: usize) -> std::ops::Range<usize> {
        let (a, b) = self.ranges.get(face).copied().unwrap_or((0, 0));
        a as usize..(b.max(a)) as usize
    }

    /// The part's B-rep vertices: the ends of its edges (seams left out),
    /// each once.
    pub fn vertices(&self, m: &Model) -> &[Vec3] {
        self.verts.get_or_init(|| {
            let mut out: Vec<Vec3> = vec![];
            // (by cell of 1 mm: an end within 1e-3 of one already there
            // lies in its cell or the next)
            let mut cells: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
            let cell = |p: Vec3| [p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32];
            for e in m.edges.iter().filter(|e| !e.seam) {
                let (Some(a), Some(b)) = (e.pts.first(), e.pts.last()) else { continue };
                for p in [Vec3::from(*a), Vec3::from(*b)] {
                    let c = cell(p);
                    let mut known = false;
                    'near: for dx in -1..=1 {
                        for dy in -1..=1 {
                            for dz in -1..=1 {
                                if let Some(l) = cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) {
                                    if l.iter().any(|i| out[*i as usize].distance(p) < 1e-3) {
                                        known = true;
                                        break 'near;
                                    }
                                }
                            }
                        }
                    }
                    if !known {
                        cells.entry(c).or_default().push(out.len() as u32);
                        out.push(p);
                    }
                }
            }
            out
        })
    }
}

/// A tree of boxes over the model's triangles (split at the median of
/// their middles along the longest side), flat: a node's children are next
/// to each other.
struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

#[derive(Clone, Copy)]
struct Node {
    lo: Vec3,
    hi: Vec3,
    /// A leaf: the first of its triangles in `order`; else its first child
    /// (the second is the next node).
    first: u32,
    /// How many triangles (0: not a leaf).
    count: u32,
}

const LEAF: usize = 4;

impl Bvh {
    fn new(m: &Model, faces: usize) -> Bvh {
        // (a triangle of no face is not picked)
        let mut order: Vec<u32> = (0..m.triangles.len() as u32).filter(|t| (m.tri_face[*t as usize] as usize) < faces).collect();
        let corners = |t: u32| m.triangles[t as usize].map(|v| Vec3::from(m.positions[v as usize]));
        let mids: Vec<Vec3> = m.triangles.iter().map(|tri| tri.iter().map(|v| Vec3::from(m.positions[*v as usize])).sum::<Vec3>() / 3.0).collect();
        let mut nodes: Vec<Node> = Vec::with_capacity(order.len() / 2 + 2);
        nodes.push(Node { lo: Vec3::ZERO, hi: Vec3::ZERO, first: 0, count: 0 });
        // (node, from, to) still to make
        let mut todo: Vec<(usize, usize, usize)> = vec![(0, 0, order.len())];
        while let Some((n, a, b)) = todo.pop() {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let (mut clo, mut chi) = (lo, hi);
            for &t in &order[a..b] {
                for q in corners(t) {
                    lo = lo.min(q);
                    hi = hi.max(q);
                }
                clo = clo.min(mids[t as usize]);
                chi = chi.max(mids[t as usize]);
            }
            let size = chi - clo;
            let axis = if size.x >= size.y && size.x >= size.z { 0 } else if size.y >= size.z { 1 } else { 2 };
            if b - a <= LEAF || size[axis] <= 0.0 {
                nodes[n] = Node { lo, hi, first: a as u32, count: (b - a) as u32 };
                continue;
            }
            let mid = (a + b) / 2;
            order[a..b].select_nth_unstable_by(mid - a, |x, y| mids[*x as usize][axis].total_cmp(&mids[*y as usize][axis]));
            let c = nodes.len();
            nodes[n] = Node { lo, hi, first: c as u32, count: 0 };
            nodes.push(Node { lo: Vec3::ZERO, hi: Vec3::ZERO, first: 0, count: 0 });
            nodes.push(Node { lo: Vec3::ZERO, hi: Vec3::ZERO, first: 0, count: 0 });
            todo.push((c, a, mid));
            todo.push((c + 1, mid, b));
        }
        Bvh { nodes, order }
    }

    /// The nearest triangle along the ray and its distance (of two at the
    /// same distance, the first in the model's order).
    fn ray(&self, m: &Model, ro: Vec3, rd: Vec3) -> Option<(u32, f32)> {
        if self.order.is_empty() {
            return None;
        }
        let inv = Vec3::new(1.0 / rd.x, 1.0 / rd.y, 1.0 / rd.z);
        // where the ray enters a node's box (a little margin), if it does
        // before `far`
        let enter = |n: &Node, far: f32| -> Option<f32> {
            let t0 = (n.lo - Vec3::splat(1e-3) - ro) * inv;
            let t1 = (n.hi + Vec3::splat(1e-3) - ro) * inv;
            let tmin = t0.min(t1).max_element();
            let tmax = t0.max(t1).min_element();
            (tmax >= tmin.max(0.0) && tmin <= far).then_some(tmin)
        };
        let mut best: Option<(u32, f32)> = None;
        let mut stack: Vec<(u32, f32)> = Vec::with_capacity(64);
        if let Some(t) = enter(&self.nodes[0], f32::MAX) {
            stack.push((0, t));
        }
        while let Some((n, tmin)) = stack.pop() {
            if best.is_some_and(|b| tmin > b.1) {
                continue;
            }
            let node = &self.nodes[n as usize];
            if node.count > 0 {
                for &t in &self.order[node.first as usize..(node.first + node.count) as usize] {
                    let tri = m.triangles[t as usize];
                    let p0 = Vec3::from(m.positions[tri[0] as usize]);
                    let p1 = Vec3::from(m.positions[tri[1] as usize]);
                    let p2 = Vec3::from(m.positions[tri[2] as usize]);
                    if let Some(d) = ray_tri(ro, rd, p0, p1, p2) {
                        if best.map_or(true, |b| d < b.1 || (d == b.1 && t < b.0)) {
                            best = Some((t, d));
                        }
                    }
                }
                continue;
            }
            let far = best.map_or(f32::MAX, |b| b.1);
            let (a, b) = (node.first, node.first + 1);
            match (enter(&self.nodes[a as usize], far), enter(&self.nodes[b as usize], far)) {
                // the nearer one first (on top of the stack)
                (Some(ta), Some(tb)) if ta <= tb => {
                    stack.push((b, tb));
                    stack.push((a, ta));
                }
                (Some(ta), Some(tb)) => {
                    stack.push((a, ta));
                    stack.push((b, tb));
                }
                (Some(ta), None) => stack.push((a, ta)),
                (None, Some(tb)) => stack.push((b, tb)),
                (None, None) => {}
            }
        }
        best
    }
}

/// Möller–Trumbore, both sides.
fn ray_tri(ro: Vec3, rd: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = rd.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = ro - a;
    let u = s.dot(p) * inv;
    if !(-1e-6..=1.0 + 1e-6).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = rd.dot(q) * inv;
    if v < -1e-6 || u + v > 1.0 + 1e-6 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-4).then_some(t)
}
