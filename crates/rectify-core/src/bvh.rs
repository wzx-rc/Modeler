//! A bounding volume hierarchy over a mesh's triangles: the triangle a ray
//! meets first, and the closest point of the surface to a point, without
//! testing every triangle (a scan of 2 M triangles, under the pointer at
//! every move). Built once: the triangles never change.

use crate::mesh::{closest_on_tri, Mesh};
use glam::{DVec3, Vec3};

/// A box and what it holds: `n > 0` a leaf, triangles `order[a..a + n]`;
/// `n == 0` an inner node, its children `a` and `a + 1`.
#[derive(Clone, Copy, Debug)]
struct Node {
    lo: Vec3,
    hi: Vec3,
    a: u32,
    n: u32,
}

pub struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

const LEAF: usize = 4;

impl Bvh {
    pub fn new(m: &Mesh) -> Bvh {
        let nt = m.tris.len();
        let mut order: Vec<u32> = (0..nt as u32).collect();
        // each triangle's box (a little grown: f32 boxes round f64 points)
        let boxes: Vec<(Vec3, Vec3)> = m
            .tris
            .iter()
            .map(|t| {
                let p = t.map(|i| m.pos[i as usize].as_vec3());
                let lo = p[0].min(p[1]).min(p[2]);
                let hi = p[0].max(p[1]).max(p[2]);
                let e = (hi - lo).max_element().max(lo.abs().max_element()).max(hi.abs().max_element()) * 1e-6 + 1e-6;
                (lo - Vec3::splat(e), hi + Vec3::splat(e))
            })
            .collect();
        let centre: Vec<Vec3> = boxes.iter().map(|(lo, hi)| (*lo + *hi) * 0.5).collect();
        let mut nodes = Vec::with_capacity(2 * nt / LEAF + 1);
        if nt == 0 {
            return Bvh { nodes, order };
        }
        nodes.push(Node { lo: Vec3::ZERO, hi: Vec3::ZERO, a: 0, n: 0 });
        // (node, first, count): made top down, children next to each other
        let mut stack = vec![(0usize, 0usize, nt)];
        while let Some((at, first, count)) = stack.pop() {
            let ids = &mut order[first..first + count];
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let (mut clo, mut chi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for &t in ids.iter() {
                let (a, b) = boxes[t as usize];
                lo = lo.min(a);
                hi = hi.max(b);
                let c = centre[t as usize];
                clo = clo.min(c);
                chi = chi.max(c);
            }
            let ext = chi - clo;
            if count <= LEAF || ext.max_element() <= 0.0 {
                nodes[at] = Node { lo, hi, a: first as u32, n: count as u32 };
                continue;
            }
            let axis = if ext.x >= ext.y && ext.x >= ext.z {
                0
            } else if ext.y >= ext.z {
                1
            } else {
                2
            };
            let half = count / 2;
            ids.select_nth_unstable_by(half, |a, b| centre[*a as usize][axis].total_cmp(&centre[*b as usize][axis]));
            let left = nodes.len();
            nodes.push(Node { lo: Vec3::ZERO, hi: Vec3::ZERO, a: 0, n: 0 });
            nodes.push(Node { lo: Vec3::ZERO, hi: Vec3::ZERO, a: 0, n: 0 });
            nodes[at] = Node { lo, hi, a: left as u32, n: 0 };
            stack.push((left, first, half));
            stack.push((left + 1, first + half, count - half));
        }
        Bvh { nodes, order }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The whole mesh's box.
    pub fn bounds(&self) -> Option<(DVec3, DVec3)> {
        self.nodes.first().map(|n| (n.lo.as_dvec3(), n.hi.as_dvec3()))
    }

    /// The first triangle the ray `o + s d` (s > 0) meets, and `s`.
    pub fn ray(&self, m: &Mesh, o: DVec3, d: DVec3) -> Option<(u32, f64)> {
        if self.nodes.is_empty() {
            return None;
        }
        let of = o.as_vec3();
        let inv = Vec3::new(1.0 / d.x as f32, 1.0 / d.y as f32, 1.0 / d.z as f32);
        let slab = |n: &Node| -> Option<f32> {
            let t0 = (n.lo - of) * inv;
            let t1 = (n.hi - of) * inv;
            let near = t0.min(t1).max_element().max(0.0);
            let far = t0.max(t1).min_element();
            // (NaN when the ray runs inside a slab's face: kept)
            (near <= far * (1.0 + 1e-5) || near.is_nan() || far.is_nan()).then_some(near)
        };
        let mut best: Option<(u32, f64)> = None;
        let mut stack: Vec<(u32, f32)> = Vec::with_capacity(64);
        if let Some(t) = slab(&self.nodes[0]) {
            stack.push((0, t));
        }
        while let Some((i, t)) = stack.pop() {
            if let Some((_, s)) = best {
                if t as f64 > s * (1.0 + 1e-5) + 1e-6 {
                    continue;
                }
            }
            let n = &self.nodes[i as usize];
            if n.n > 0 {
                for &tri in &self.order[n.a as usize..(n.a + n.n) as usize] {
                    let [a, b, c] = m.tris[tri as usize].map(|v| m.pos[v as usize]);
                    if let Some(s) = ray_tri(o, d, a, b, c) {
                        if best.map(|(_, bs)| s < bs).unwrap_or(true) {
                            best = Some((tri, s));
                        }
                    }
                }
                continue;
            }
            // the nearer child last, so it is taken first
            let (l, r) = (n.a, n.a + 1);
            let (tl, tr) = (slab(&self.nodes[l as usize]), slab(&self.nodes[r as usize]));
            match (tl, tr) {
                (Some(a), Some(b)) => {
                    if a <= b {
                        stack.push((r, b));
                        stack.push((l, a));
                    } else {
                        stack.push((l, a));
                        stack.push((r, b));
                    }
                }
                (Some(a), None) => stack.push((l, a)),
                (None, Some(b)) => stack.push((r, b)),
                (None, None) => {}
            }
        }
        best
    }

    /// The closest point of the surface to `p` within `max`, and its triangle.
    pub fn closest(&self, m: &Mesh, p: DVec3, max: f64) -> Option<(u32, DVec3)> {
        if self.nodes.is_empty() {
            return None;
        }
        let pf = p.as_vec3();
        let gap = |n: &Node| -> f64 {
            let q = pf.clamp(n.lo, n.hi);
            (q - pf).length_squared() as f64
        };
        let mut best: (f64, Option<(u32, DVec3)>) = (max * max, None);
        let mut stack: Vec<(u32, f64)> = vec![(0, gap(&self.nodes[0]))];
        while let Some((i, g)) = stack.pop() {
            if g > best.0 * (1.0 + 1e-5) + 1e-9 {
                continue;
            }
            let n = &self.nodes[i as usize];
            if n.n > 0 {
                for &tri in &self.order[n.a as usize..(n.a + n.n) as usize] {
                    let [a, b, c] = m.tris[tri as usize].map(|v| m.pos[v as usize]);
                    let q = closest_on_tri(p, a, b, c);
                    let d = (q - p).length_squared();
                    if d < best.0 {
                        best = (d, Some((tri, q)));
                    }
                }
                continue;
            }
            let (l, r) = (n.a, n.a + 1);
            let (gl, gr) = (gap(&self.nodes[l as usize]), gap(&self.nodes[r as usize]));
            if gl <= gr {
                stack.push((r, gr));
                stack.push((l, gl));
            } else {
                stack.push((l, gl));
                stack.push((r, gr));
            }
        }
        best.1
    }
}

/// Where the ray `o + s d` (s > 0) crosses the triangle, either side.
fn ray_tri(o: DVec3, d: DVec3, a: DVec3, b: DVec3, c: DVec3) -> Option<f64> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-18 {
        return None;
    }
    let inv = 1.0 / det;
    let t = o - a;
    let u = t.dot(p) * inv;
    if !(-1e-9..=1.0 + 1e-9).contains(&u) {
        return None;
    }
    let q = t.cross(e1);
    let v = d.dot(q) * inv;
    if v < -1e-9 || u + v > 1.0 + 1e-9 {
        return None;
    }
    let s = e2.dot(q) * inv;
    (s > 0.0).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sphere of `n` × `2n` quads (two triangles each).
    fn sphere(r: f64, n: usize) -> Mesh {
        let mut m = Mesh { unit_mm: 1.0, ..Default::default() };
        for i in 0..=n {
            let th = std::f64::consts::PI * i as f64 / n as f64;
            for j in 0..2 * n {
                let ph = std::f64::consts::PI * j as f64 / n as f64;
                m.pos.push(DVec3::new(th.sin() * ph.cos(), th.cos(), th.sin() * ph.sin()) * r);
            }
        }
        let w = 2 * n as u32;
        for i in 0..n as u32 {
            for j in 0..w {
                let (a, b, c, d) = (i * w + j, i * w + (j + 1) % w, (i + 1) * w + j, (i + 1) * w + (j + 1) % w);
                m.tris.push([a, c, b]);
                m.tris.push([b, c, d]);
            }
        }
        m
    }

    #[test]
    fn rays_and_closest_points_match_brute_force() {
        let m = sphere(10.0, 60);
        let bvh = Bvh::new(&m);
        let mut seed = 7u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
        };
        for _ in 0..300 {
            let o = DVec3::new(rnd(), rnd(), rnd()) * 30.0;
            let d = (DVec3::new(rnd(), rnd(), rnd()) * 6.0 - o * 0.2).normalize();
            let mut want: Option<f64> = None;
            for t in &m.tris {
                let [a, b, c] = t.map(|v| m.pos[v as usize]);
                if let Some(s) = ray_tri(o, d, a, b, c) {
                    want = Some(want.map_or(s, |w: f64| w.min(s)));
                }
            }
            let got = bvh.ray(&m, o, d).map(|(_, s)| s);
            match (want, got) {
                (Some(w), Some(g)) => assert!((w - g).abs() < 1e-6, "{w} {g}"),
                (None, None) => {}
                other => panic!("{other:?}"),
            }
            // the closest point
            let p = DVec3::new(rnd(), rnd(), rnd()) * 15.0;
            let want = m.tris.iter().map(|t| {
                let [a, b, c] = t.map(|v| m.pos[v as usize]);
                (closest_on_tri(p, a, b, c) - p).length()
            });
            let want = want.fold(f64::MAX, f64::min);
            let got = bvh.closest(&m, p, 100.0).map(|(_, q)| (q - p).length()).unwrap();
            assert!((want - got).abs() < 1e-9, "{want} {got}");
        }
    }
}

#[cfg(test)]
mod timing {
    use super::*;

    /// `RECTIFY_MESH=GUN_A_ORIGIN.obj cargo test --release -p rectify-core bvh_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bvh_timing() {
        let Ok(path) = std::env::var("RECTIFY_MESH") else { return };
        let t = std::time::Instant::now();
        let m = Mesh::load_obj(std::path::Path::new(&path)).unwrap();
        eprintln!("load {:?}: {} vertices, {} triangles", t.elapsed(), m.pos.len(), m.tris.len());
        let t = std::time::Instant::now();
        let b = Bvh::new(&m);
        eprintln!("bvh {:?}, {} nodes", t.elapsed(), b.nodes.len());
        let (lo, hi) = b.bounds().unwrap();
        let c = (lo + hi) * 0.5;
        let t = std::time::Instant::now();
        let mut hits = 0;
        for i in 0..1000 {
            let a = i as f64 * 0.0137;
            let o = c + DVec3::new(a.cos(), 0.3, a.sin()) * (hi - lo).length();
            let d = (c + (DVec3::new((a * 7.0).sin(), (a * 3.0).cos(), (a * 5.0).sin()) * 0.3) * (hi - lo) - o).normalize();
            hits += b.ray(&m, o, d).is_some() as usize;
        }
        eprintln!("1000 rays {:?}, {hits} hits", t.elapsed());
        let t = std::time::Instant::now();
        for i in 0..1000 {
            let a = i as f64 * 0.0137;
            let p = lo + (hi - lo) * DVec3::new(0.5 + 0.5 * a.sin(), 0.5 + 0.5 * (a * 2.0).cos(), 0.5 + 0.5 * (a * 3.0).sin());
            b.closest(&m, p, 1e9);
        }
        eprintln!("1000 closest points {:?}", t.elapsed());
    }
}
