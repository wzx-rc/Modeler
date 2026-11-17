//! A first polygon converter (Romain, 17 nov. 2026: "un premier
//! convertisseur polygonal sur un objet simple avec fillet"): the B-rep
//! solid as quads with a clean flow, watertight.
//!
//! Each face becomes one four-sided patch:
//! - its corners are where its outline turns sharply; with fewer than four
//!   (a side face whose corner a fillet rounded, a top face rounded all
//!   around), the longest stretch between corners is cut in its middle
//!   until there are four;
//! - every piece of edge gets a number of segments (by length, a fillet's
//!   arc by angle, like a bevel's rows), raised until opposite sides of
//!   every face have the same count;
//! - the points along an edge are shared by the two faces on it, so the
//!   mesh is closed; inside, a Coons grid in the face's own (u, v)
//!   coordinates, put on the surface (a fillet stays round).
//!
//! Not yet: faces with a hole, faces with more than four sharp corners,
//! three-sided corner patches. They are listed in `skipped`.

use forme_occt::{QuadTopo, Shape};
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct QuadMesh {
    pub positions: Vec<[f64; 3]>,
    pub quads: Vec<[u32; 4]>,
    /// The face (index in the shape's face map) of each quad.
    pub quad_face: Vec<u32>,
    /// Faces left out, and why.
    pub skipped: Vec<(usize, String)>,
}

/// Turning angle above which a point of a face's outline is a corner.
const CORNER: f64 = 25.0 * std::f64::consts::PI / 180.0;
/// A fillet's arc: one segment every this many radians (90° → 4 rows).
const ARC_STEP: f64 = std::f64::consts::PI / 8.0;

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: [f64; 3]) -> [f64; 3] {
    let l = len(a).max(1e-30);
    [a[0] / l, a[1] / l, a[2] / l]
}

/// An edge's length along its samples, and the parameter at a fraction of
/// that length.
struct Arc {
    cum: Vec<f64>,
    t: [f64; 2],
}

impl Arc {
    fn new(pts: &[[f64; 3]], t: [f64; 2]) -> Arc {
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            cum.push(cum.last().unwrap() + len(sub(w[1], w[0])));
        }
        Arc { cum, t }
    }
    fn length(&self) -> f64 {
        *self.cum.last().unwrap()
    }
    /// The parameter at fraction `f` of the length (0 at t0, 1 at t1).
    fn param(&self, f: f64) -> f64 {
        let total = self.length();
        let n = self.cum.len() - 1;
        let u = if total <= 1e-12 {
            f
        } else {
            let want = f.clamp(0.0, 1.0) * total;
            let k = self.cum.partition_point(|c| *c < want).clamp(1, n);
            let (a, b) = (self.cum[k - 1], self.cum[k]);
            let s = if b - a > 1e-12 { (want - a) / (b - a) } else { 0.0 };
            ((k - 1) as f64 + s) / n as f64
        };
        self.t[0] + (self.t[1] - self.t[0]) * u
    }
}

/// A place on a face's outline: wire entry `k`, at fraction `f` of its
/// length in the direction of travel (1 = the junction with the next one).
#[derive(Clone, Copy, Debug)]
struct Pos {
    k: usize,
    f: f64,
}

/// A piece of an edge: between two of its cut fractions (natural direction).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PieceId {
    edge: i32,
    j: usize,
}

/// Where a point of the mesh lies on the edges, to share it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key {
    /// A B-rep vertex.
    Vertex(i32),
    /// A cut of an edge (index in its cut list).
    Cut(i32, usize),
    /// A point inside a piece.
    Inner(PieceId, usize),
}

struct Edges<'a> {
    topo: &'a QuadTopo,
    arcs: Vec<Arc>,
    /// Inner cut fractions of each edge (natural direction), sorted.
    cuts: Vec<Vec<f64>>,
}

impl Edges<'_> {
    fn arc(&self, e: i32) -> &Arc {
        &self.arcs[(e - 1) as usize]
    }
    /// Fractions bounding the pieces of an edge: 0, the cuts, 1.
    fn bounds(&self, e: i32) -> Vec<f64> {
        let mut b = vec![0.0];
        b.extend(self.cuts[(e - 1) as usize].iter().copied());
        b.push(1.0);
        b
    }
    fn piece_len(&self, p: PieceId) -> f64 {
        let b = self.bounds(p.edge);
        self.arc(p.edge).length() * (b[p.j + 1] - b[p.j])
    }
    /// The key of the point at the start (`end` false) or end of a piece,
    /// natural direction.
    fn end_key(&self, p: PieceId, end: bool) -> Key {
        let e = &self.topo.edges[(p.edge - 1) as usize];
        let n = self.cuts[(p.edge - 1) as usize].len();
        let j = if end { p.j + 1 } else { p.j };
        if j == 0 {
            Key::Vertex(e.v[0])
        } else if j == n + 1 {
            Key::Vertex(e.v[1])
        } else {
            Key::Cut(p.edge, j - 1)
        }
    }
}

/// The points of a face's outline in travel order, for the tangents.
fn travel(topo: &QuadTopo, e: i32, rev: bool) -> Vec<[f64; 3]> {
    let mut p = topo.edges[(e - 1) as usize].pts.clone();
    if rev {
        p.reverse();
    }
    p
}

/// The four corners of a face's outline (sorted along the loop), or why not.
fn corners(edges: &Edges, wire: &[(i32, bool)]) -> Result<Vec<Pos>, String> {
    let n = wire.len();
    let mut out = vec![];
    for k in 0..n {
        let a = travel(edges.topo, wire[k].0, wire[k].1);
        let b = travel(edges.topo, wire[(k + 1) % n].0, wire[(k + 1) % n].1);
        let tin = unit(sub(a[a.len() - 1], a[a.len() - 2]));
        let tout = unit(sub(b[1], b[0]));
        if dot(tin, tout).clamp(-1.0, 1.0).acos() > CORNER {
            out.push(Pos { k, f: 1.0 });
        }
    }
    if out.len() > 4 {
        return Err(format!("{} sharp corners (4 at most for now)", out.len()));
    }
    let lens: Vec<f64> = wire.iter().map(|(e, _)| edges.arc(*e).length()).collect();
    let total: f64 = lens.iter().sum();
    // a position as a length along the loop from the start of entry 0
    let at = |p: Pos| lens[..p.k].iter().sum::<f64>() + lens[p.k] * p.f;
    let pos_of = |mut s: f64| -> Pos {
        s = s.rem_euclid(total);
        for k in 0..n {
            if s <= lens[k] + 1e-12 {
                return Pos { k, f: if lens[k] > 0.0 { (s / lens[k]).min(1.0) } else { 1.0 } };
            }
            s -= lens[k];
        }
        Pos { k: n - 1, f: 1.0 }
    };
    if out.is_empty() {
        // rounded all around: start in the middle of the longest curved edge
        let k = (0..n).filter(|k| edges.topo.edges[(wire[*k].0 - 1) as usize].kind != 0).max_by(|a, b| lens[*a].total_cmp(&lens[*b])).unwrap_or(0);
        out.push(Pos { k, f: 0.5 });
    }
    while out.len() < 4 {
        out.sort_by(|a, b| at(*a).total_cmp(&at(*b)));
        // the longest stretch between two corners, cut in its middle
        let m = out.len();
        let (i, run) = (0..m)
            .map(|i| {
                let (a, b) = (at(out[i]), at(out[(i + 1) % m]));
                (i, if b > a + 1e-9 { b - a } else { b + total - a })
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        let mut p = pos_of(at(out[i]) + run * 0.5);
        // close to a junction (a fifth of the edge's length): cut there
        if p.f < 0.2 {
            p = Pos { k: (p.k + n - 1) % n, f: 1.0 };
        } else if p.f > 0.8 {
            p.f = 1.0;
        }
        // the end of entry k and the start of entry k + 1 are the same place
        let p = if p.f <= 1e-9 { Pos { k: (p.k + n - 1) % n, f: 1.0 } } else { p };
        out.push(p);
    }
    out.sort_by(|a, b| at(*a).total_cmp(&at(*b)));
    Ok(out)
}

/// One face's outline as oriented pieces, cut into its four sides.
fn sides(edges: &Edges, wire: &[(i32, bool)], corners: &[Pos]) -> Option<[Vec<(PieceId, bool)>; 4]> {
    // the pieces in travel order, and after each one whether a corner ends it
    let mut seq: Vec<(PieceId, bool, bool)> = vec![];
    for (k, &(e, rev)) in wire.iter().enumerate() {
        let b = edges.bounds(e);
        let np = b.len() - 1;
        for step in 0..np {
            let j = if rev { np - 1 - step } else { step };
            // the travel fraction where this piece ends
            let end = if rev { 1.0 - b[j] } else { b[j + 1] };
            let corner = corners.iter().any(|c| c.k == k && (c.f - end).abs() < 1e-6);
            seq.push((PieceId { edge: e, j }, rev, corner));
        }
    }
    let first = seq.iter().position(|s| s.2)?;
    seq.rotate_left(first + 1);
    let mut out: [Vec<(PieceId, bool)>; 4] = Default::default();
    let mut side = 0;
    for (p, rev, corner) in seq {
        if side > 3 {
            return None;
        }
        out[side].push((p, rev));
        if corner {
            side += 1;
        }
    }
    (side == 4 && out.iter().all(|s| !s.is_empty())).then_some(out)
}

/// The quads of a solid; `target` is the length of a segment (mm).
pub fn convert(shape: &Shape, target: f64) -> Result<QuadMesh, String> {
    let topo = shape.quad_topo()?;
    let arcs: Vec<Arc> = topo.edges.iter().map(|e| Arc::new(&e.pts, e.t)).collect();
    let mut edges = Edges { topo: &topo, arcs, cuts: vec![vec![]; topo.edges.len()] };
    let mut mesh = QuadMesh::default();

    // 1. each face's corners (cuts added to the edges where needed)
    let mut faces: Vec<(usize, Vec<(i32, bool)>, Vec<Pos>)> = vec![];
    for (fi, f) in topo.faces.iter().enumerate() {
        if f.wires.len() != 1 {
            mesh.skipped.push((fi, if f.wires.is_empty() { "no outline".into() } else { "a face with a hole".into() }));
            continue;
        }
        let wire = f.wires[0].clone();
        if wire.iter().any(|(e, _)| topo.edges[(*e - 1) as usize].degenerated) {
            mesh.skipped.push((fi, "a pole (degenerated edge)".into()));
            continue;
        }
        match corners(&edges, &wire) {
            Ok(c) => {
                for p in &c {
                    if p.f < 1.0 - 1e-9 {
                        let (e, rev) = wire[p.k];
                        let g = if rev { 1.0 - p.f } else { p.f };
                        let cuts = &mut edges.cuts[(e - 1) as usize];
                        if !cuts.iter().any(|c| (c - g).abs() < 1e-6) {
                            cuts.push(g);
                            cuts.sort_by(|a, b| a.total_cmp(b));
                        }
                    }
                }
                faces.push((fi, wire, c));
            }
            Err(e) => mesh.skipped.push((fi, e)),
        }
    }

    // 2. the four sides of each face, as pieces
    let mut patches: Vec<(usize, [Vec<(PieceId, bool)>; 4])> = vec![];
    for (fi, wire, c) in &faces {
        match sides(&edges, wire, c) {
            Some(s) => patches.push((*fi, s)),
            None => mesh.skipped.push((*fi, "its corners could not be placed".into())),
        }
    }

    // 3. segments per piece: by length (an arc by angle), then raised until
    // opposite sides match
    let mut count: HashMap<PieceId, usize> = HashMap::new();
    for (_, s) in &patches {
        for (p, _) in s.iter().flatten() {
            count.entry(*p).or_insert_with(|| {
                let l = edges.piece_len(*p);
                let e = &topo.edges[(p.edge - 1) as usize];
                let by_len = (l / target).round() as usize;
                let n = if e.kind == 1 && e.radius > 0.0 { by_len.max((l / e.radius / ARC_STEP).round() as usize) } else { by_len };
                n.max(1)
            });
        }
    }
    let mut balanced = false;
    for _ in 0..500 {
        let mut changed = false;
        for (_, s) in &patches {
            for (a, b) in [(0, 2), (1, 3)] {
                let sum = |k: usize| s[k].iter().map(|(p, _)| count[p]).sum::<usize>();
                let (na, nb) = (sum(a), sum(b));
                if na != nb {
                    let short = if na < nb { a } else { b };
                    // the coarsest piece of the short side gets one more
                    let (p, _) = s[short].iter().max_by(|x, y| (edges.piece_len(x.0) / count[&x.0] as f64).total_cmp(&(edges.piece_len(y.0) / count[&y.0] as f64))).unwrap();
                    *count.get_mut(p).unwrap() += 1;
                    changed = true;
                }
            }
        }
        if !changed {
            balanced = true;
            break;
        }
    }
    if !balanced || std::env::var("FORME_QUADS_DEBUG").is_ok() {
        {
            for (fi, s) in &patches {
                let d: Vec<String> = s.iter().map(|side| side.iter().map(|(p, r)| format!("{}.{}{}={}", p.edge, p.j, if *r { "r" } else { "" }, count[p])).collect::<Vec<_>>().join("+")).collect();
                eprintln!("face {fi}: {}", d.join(" | "));
            }
        }
    }
    if !balanced {
        return Err("the segment counts do not settle (opposite sides of the faces)".into());
    }

    // 4. the shared points along the edges
    let mut index: HashMap<Key, u32> = HashMap::new();
    let mut add = |mesh: &mut QuadMesh, key: Key, p: [f64; 3]| -> u32 {
        *index.entry(key).or_insert_with(|| {
            mesh.positions.push(p);
            (mesh.positions.len() - 1) as u32
        })
    };

    // 5. each face: its four sides, a Coons grid inside
    for (fi, s) in &patches {
        let fi = *fi;
        // each side: (point index, uv), from its first corner to its last
        let mut side_pts: Vec<Vec<(u32, [f64; 2])>> = vec![];
        let mut failed = None;
        for side in s.iter() {
            let mut pts: Vec<(u32, [f64; 2])> = vec![];
            for &(p, rev) in side {
                let n = count[&p];
                let b = edges.bounds(p.edge);
                let arc = edges.arc(p.edge);
                // natural direction: fractions from b[j] to b[j + 1]
                let fr: Vec<f64> = (0..=n).map(|i| b[p.j] + (b[p.j + 1] - b[p.j]) * i as f64 / n as f64).collect();
                let ts: Vec<f64> = fr.iter().map(|f| arc.param(*f)).collect();
                let (xyz, uv) = match shape.edge_on_face(fi, p.edge, rev, &ts) {
                    Ok(r) => r,
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                };
                let keys: Vec<Key> = (0..=n).map(|i| if i == 0 { edges.end_key(p, false) } else if i == n { edges.end_key(p, true) } else { Key::Inner(p, i) }).collect();
                let mut ids: Vec<(u32, [f64; 2])> = (0..=n).map(|i| (add(&mut mesh, keys[i], xyz[i]), uv[i])).collect();
                if rev {
                    ids.reverse();
                }
                // the first point is the previous piece's last one
                if !pts.is_empty() {
                    ids.remove(0);
                }
                pts.extend(ids);
            }
            side_pts.push(pts);
        }
        if let Some(e) = failed {
            mesh.skipped.push((fi, e));
            continue;
        }
        let nn = side_pts[0].len() - 1;
        let mm = side_pts[1].len() - 1;
        if nn == 0 || mm == 0 || side_pts[2].len() - 1 != nn || side_pts[3].len() - 1 != mm {
            mesh.skipped.push((fi, "opposite sides differ".into()));
            continue;
        }
        let bottom = |i: usize| side_pts[0][i];
        let right = |j: usize| side_pts[1][j];
        let top = |i: usize| side_pts[2][nn - i];
        let left = |j: usize| side_pts[3][mm - j];
        let (c00, c10, c11, c01) = (bottom(0).1, bottom(nn).1, top(nn).1, top(0).1);
        let mut grid = vec![vec![(0u32, [0.0f64; 2]); mm + 1]; nn + 1];
        let mut inner = vec![];
        for i in 0..=nn {
            for j in 0..=mm {
                let boundary = if j == 0 {
                    Some(bottom(i))
                } else if j == mm {
                    Some(top(i))
                } else if i == 0 {
                    Some(left(j))
                } else if i == nn {
                    Some(right(j))
                } else {
                    None
                };
                grid[i][j] = match boundary {
                    Some(b) => b,
                    None => {
                        let (u, v) = (i as f64 / nn as f64, j as f64 / mm as f64);
                        let (b, t, l, r) = (bottom(i).1, top(i).1, left(j).1, right(j).1);
                        let c = |k: usize| (1.0 - v) * b[k] + v * t[k] + (1.0 - u) * l[k] + u * r[k] - ((1.0 - u) * (1.0 - v) * c00[k] + u * (1.0 - v) * c10[k] + (1.0 - u) * v * c01[k] + u * v * c11[k]);
                        inner.push((i, j));
                        (u32::MAX, [c(0), c(1)])
                    }
                };
            }
        }
        // on the surface: the inner points, and every point's normal
        let all: Vec<[f64; 2]> = (0..=nn).flat_map(|i| (0..=mm).map(move |j| (i, j))).map(|(i, j)| grid[i][j].1).collect();
        let eval = match shape.face_eval(fi, &all) {
            Ok(e) => e,
            Err(e) => {
                mesh.skipped.push((fi, e));
                continue;
            }
        };
        for &(i, j) in &inner {
            mesh.positions.push(eval[i * (mm + 1) + j].0);
            grid[i][j].0 = (mesh.positions.len() - 1) as u32;
        }
        // the quads, turned to face outwards
        let mut qs = vec![];
        let mut agree = 0.0;
        for i in 0..nn {
            for j in 0..mm {
                let q = [grid[i][j].0, grid[i + 1][j].0, grid[i + 1][j + 1].0, grid[i][j + 1].0];
                let p = |k: usize| mesh.positions[q[k] as usize];
                let n = cross(sub(p(2), p(0)), sub(p(3), p(1)));
                let s = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)].iter().map(|(a, b)| eval[a * (mm + 1) + b].1).fold([0.0; 3], |x, y| [x[0] + y[0], x[1] + y[1], x[2] + y[2]]);
                agree += dot(n, s);
                qs.push(q);
            }
        }
        for mut q in qs {
            if agree < 0.0 {
                q.reverse();
            }
            mesh.quads.push(q);
            mesh.quad_face.push(fi as u32);
        }
    }
    Ok(mesh)
}

impl QuadMesh {
    /// Wavefront OBJ: one group per face, fillet faces named so.
    pub fn obj(&self, face_name: impl Fn(usize) -> String) -> String {
        let mut s = String::from("# Forme NURBS: quads from the B-rep\n");
        for p in &self.positions {
            s.push_str(&format!("v {:.5} {:.5} {:.5}\n", p[0], p[1], p[2]));
        }
        let mut last = u32::MAX;
        for (q, f) in self.quads.iter().zip(&self.quad_face) {
            if *f != last {
                s.push_str(&format!("g {}\n", face_name(*f as usize)));
                last = *f;
            }
            s.push_str(&format!("f {} {} {} {}\n", q[0] + 1, q[1] + 1, q[2] + 1, q[3] + 1));
        }
        s
    }

    /// Every edge of the quads is used by exactly two of them.
    pub fn is_closed(&self) -> bool {
        let mut uses: HashMap<(u32, u32), i32> = HashMap::new();
        for q in &self.quads {
            for k in 0..4 {
                let (a, b) = (q[k], q[(k + 1) % 4]);
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        uses.values().all(|n| *n == 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge_between(s: &Shape, a: [f64; 3], b: [f64; 3]) -> i32 {
        // the edge whose middle sample is nearest the middle of a and b
        let m = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5];
        let d = |e: &forme_occt::Edge| len(sub(e.pts[e.pts.len() / 2], m));
        s.edges(9).iter().min_by(|x, y| d(x).total_cmp(&d(y))).unwrap().index
    }

    #[test]
    fn cube_with_fillets() {
        // a 20 mm cube (centred), one edge filleted at 4 mm
        let cube = Shape::cuboid(20.0, 20.0, 20.0).unwrap();
        let e = edge_between(&cube, [-10.0, 10.0, 10.0], [10.0, 10.0, 10.0]);
        let one = cube.fillet(&[e], 4.0, 99).unwrap();
        let q = convert(&one, 2.5).unwrap();
        assert!(q.skipped.is_empty(), "{:?}", q.skipped);
        assert_eq!(q.quad_face.iter().collect::<std::collections::HashSet<_>>().len(), 7, "7 faces");
        assert!(q.is_closed(), "every quad edge is shared by two quads");
        // the fillet's quads lie on its circle (radius 4 around the axis)
        let fillet_face = (0..one.face_count()).find(|f| one.face_info(*f).map(|i| i.kind == forme_occt::SurfKind::Cylinder).unwrap_or(false)).unwrap();
        let info = one.face_info(fillet_face).unwrap();
        for (_, c) in q.quads.iter().enumerate().filter(|(k, _)| q.quad_face[*k] as usize == fillet_face) {
            for v in c {
                let d = sub(q.positions[*v as usize], info.axis_origin);
                let along = dot(d, info.axis_dir);
                let r = len(sub(d, [info.axis_dir[0] * along, info.axis_dir[1] * along, info.axis_dir[2] * along]));
                assert!((r - 4.0).abs() < 1e-6, "{d:?} at {r}");
            }
        }
        std::fs::write(std::env::temp_dir().join("forme-quads-one.obj"), q.obj(|f| format!("face{f}"))).unwrap();

        // the four upright edges filleted: top and bottom rounded all around
        let ups: Vec<i32> = [(-10.0, -10.0), (10.0, -10.0), (10.0, 10.0), (-10.0, 10.0)].iter().map(|(x, z)| edge_between(&cube, [*x, -10.0, *z], [*x, 10.0, *z])).collect();
        let four = cube.fillet(&ups, 4.0, 99).unwrap();
        let q = convert(&four, 2.5).unwrap();
        assert!(q.skipped.is_empty(), "{:?}", q.skipped);
        assert!(q.is_closed());
        std::fs::write(std::env::temp_dir().join("forme-quads-four.obj"), q.obj(|f| format!("face{f}"))).unwrap();

        // a face with a hole is left out, said so
        let holed = cube.cut(&Shape::cylinder(4.0, 30.0).unwrap()).unwrap();
        let q = convert(&holed, 2.5).unwrap();
        assert!(q.skipped.iter().any(|(_, w)| w.contains("hole")), "{:?}", q.skipped);
    }
}
