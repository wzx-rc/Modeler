//! The document built with OpenCascade (crate forme-occt): each feature's
//! solid, its faces tagged (`FaceTag`), then the booleans in order. Used by
//! the kernel process; on iPad it will run in the app itself.
//!
//! Caches, for live editing: the solid of each feature (by content) and the
//! result after each feature (by the content of everything up to it). While
//! one feature moves, only the booleans from it onwards are made again.

use crate::doc::{Doc, EdgeRef, Feature, Op, Prim, Round};
use crate::model::{Edge, EdgeKind, Face, FaceTag, Model, SurfKind};
use crate::proto::step;
use forme_occt::{Shape, SurfKind as OccSurf};
use glam::{DMat4, DVec3};
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;
use std::time::Instant;

/// Meshing: chord (mm) and angle (rad), fine when idle, coarse while moving.
const FINE: (f64, f64) = (0.02, 0.2);
const COARSE: (f64, f64) = (0.08, 0.35);

/// Writes the step on stderr before an operation that could crash the process.
fn mark(s: &str) {
    let mut e = std::io::stderr();
    let _ = writeln!(e, "STEP {s}");
    let _ = e.flush();
}

/// The result after a feature: the solid (None while nothing is built) and
/// what went wrong up to there.
#[derive(Clone)]
struct Stage {
    shape: Option<Rc<Shape>>,
    errors: Vec<String>,
}

#[derive(Default)]
pub struct Cache {
    tools: HashMap<u64, Rc<Shape>>,
    stages: HashMap<u64, Stage>,
}

fn rows(m: DMat4) -> [[f64; 4]; 3] {
    let c = m.to_cols_array_2d();
    [[c[0][0], c[1][0], c[2][0], c[3][0]], [c[0][1], c[1][1], c[2][1], c[3][1]], [c[0][2], c[1][2], c[2][2], c[3][2]]]
}

/// Which face of the primitive (local frame) a face is, from its normal and
/// its middle point. A sketch: 1 top, 2 bottom, 3 + the outline piece a side
/// comes from (`doc::Piece::id`).
fn sub_of(f: &Feature, kind: OccSurf, n: DVec3, at: DVec3) -> u16 {
    match f.prim {
        Prim::Box { .. } => {
            let a = n.abs();
            if a.x >= a.y && a.x >= a.z {
                if n.x > 0.0 { 0 } else { 1 }
            } else if a.y >= a.z {
                if n.y > 0.0 { 2 } else { 3 }
            } else if n.z > 0.0 {
                4
            } else {
                5
            }
        }
        Prim::Cylinder { .. } => {
            if kind != OccSurf::Plane {
                0
            } else if n.y > 0.0 {
                1
            } else {
                2
            }
        }
        Prim::Profile { x, z, .. } => {
            if kind == OccSurf::Plane && n.y > 0.5 {
                1
            } else if kind == OccSurf::Plane && n.y < -0.5 {
                2
            } else {
                // the piece nearest the side's middle, seen from above
                let q = [at.x, at.z];
                let pieces = f.profile.as_ref().map(|p| p.pieces(x, z)).unwrap_or_default();
                let near = pieces.iter().map(|pc| (pc.id, pc.pts.windows(2).map(|w| seg_dist(q, w[0], w[1])).fold(f64::MAX, f64::min))).min_by(|a, b| a.1.total_cmp(&b.1));
                3 + near.map(|(id, _)| id).unwrap_or(0).min(60000) as u16
            }
        }
    }
}

/// Distance from `q` to the segment `a`–`b` (plane coordinates).
fn seg_dist(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx * dx + dy * dy;
    let t = if l > 0.0 { (((q[0] - a[0]) * dx + (q[1] - a[1]) * dy) / l).clamp(0.0, 1.0) } else { 0.0 };
    ((q[0] - a[0] - t * dx).powi(2) + (q[1] - a[1] - t * dy).powi(2)).sqrt()
}

/// The feature's solid in the world, its faces tagged.
fn tool(f: &Feature) -> Result<Shape, String> {
    let h = f.prim.height().max(0.01);
    let s = match f.prim {
        Prim::Box { x, z, .. } => Shape::cuboid(x.max(0.01), h, z.max(0.01))?,
        Prim::Cylinder { radius, .. } => Shape::cylinder(radius.max(0.01), h)?,
        Prim::Profile { x, z, .. } => {
            let pr = f.profile.as_ref().ok_or("a sketch without its outline")?;
            // OpenCascade extrudes an outline of its XY plane along Z: local
            // (x, z) is OCC (x, -z), then turned so OCC Z is local Y
            let pts: Vec<[f64; 2]> = pr.scaled(x.max(0.01), z.max(0.01)).iter().map(|p| [p[0], -p[1]]).collect();
            let s = Shape::prism(&pts, &pr.smooth, h * 0.5)?;
            s.transformed(rows(DMat4::from_rotation_x(-std::f64::consts::FRAC_PI_2)))?
        }
    };
    let mut s = s;
    // the shape is centred on its local origin: tag it in its own frame
    let tags: Vec<i64> = (0..s.face_count())
        .map(|i| {
            let info = s.face_info(i);
            let sub = info.map(|fi| sub_of(f, fi.kind, DVec3::from_array(fi.normal), DVec3::from_array(fi.point))).unwrap_or(0);
            FaceTag { feature: f.id, sub }.pack()
        })
        .collect();
    s.set_tags(&tags);
    let m = f.matrix() * DMat4::from_translation(DVec3::Y * h * 0.5);
    s.transformed(rows(m))
}

fn surf(k: OccSurf) -> SurfKind {
    match k {
        OccSurf::Plane => SurfKind::Plane,
        OccSurf::Cylinder => SurfKind::Cylinder,
        OccSurf::Cone => SurfKind::Cone,
        OccSurf::Sphere => SurfKind::Sphere,
        OccSurf::Torus => SurfKind::Torus,
        OccSurf::Other => SurfKind::Other,
    }
}

/// The edges of `shape` the references name (edge-map indices, 1-based),
/// and how many were not found. Among the edges joining the same two faces,
/// the one nearest the reference's point.
fn resolve(shape: &Shape, refs: &[EdgeRef]) -> (Vec<i32>, usize) {
    let adj = shape.edge_faces();
    let tags: Vec<i64> = (0..shape.face_count()).map(|i| shape.face_tag(i)).collect();
    let tag = |f: i32| if f >= 0 { tags.get(f as usize).copied() } else { None };
    let mut out = vec![];
    let mut missing = 0;
    for r in refs {
        let want = r.key();
        let found: Vec<i32> = adj
            .iter()
            .enumerate()
            .filter_map(|(i, f)| {
                let (a, b) = (tag(f[0])?, tag(f[1])?);
                let k = if a <= b { [a, b] } else { [b, a] };
                (k == want).then_some(i as i32 + 1)
            })
            .collect();
        let best = match found.as_slice() {
            [] => None,
            [one] => Some(*one),
            many => many
                .iter()
                .map(|&i| {
                    let d = shape.edge(i, 9).map(|e| e.pts.iter().map(|p| DVec3::from_array(*p).distance(r.at)).fold(f64::MAX, f64::min)).unwrap_or(f64::MAX);
                    (i, d)
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i),
        };
        match best {
            Some(i) if !out.contains(&i) => out.push(i),
            Some(_) => {}
            None => missing += 1,
        }
    }
    (out, missing)
}

/// A fillet or chamfer step on what is built so far: the new solid (None
/// when there is nothing to round), and the edges not found.
fn round(acc: &Shape, f: &Feature, r: &Round) -> (Option<Result<Shape, String>>, usize) {
    let (edges, missing) = resolve(acc, &r.edges);
    if edges.is_empty() || r.size <= 1e-6 {
        return (None, missing);
    }
    let fresh = FaceTag { feature: f.id, sub: 0 }.pack();
    mark(&step("round", f));
    let res = if r.chamfer { acc.chamfer(&edges, r.size, fresh) } else { acc.fillet(&edges, r.size, fresh) };
    (Some(res), missing)
}

fn hash2(a: u64, b: u64) -> u64 {
    crate::doc::fnv(&[a.to_le_bytes(), b.to_le_bytes()].concat())
}

/// Builds the document. Never fails: what cannot be made is listed in
/// `Model::errors` and left out.
pub fn build(doc: &Doc, fine: bool, quads: bool, skip: &[String], cache: &mut Cache) -> Model {
    let t0 = Instant::now();
    let mut model = Model { fine, ..Default::default() };
    let mut stage = Stage { shape: None, errors: vec![] };
    let mut key = 0u64;
    let mut used_tools = vec![];
    let mut used_stages = vec![];
    for f in doc.features.iter().filter(|f| f.visible) {
        let fh = f.hash();
        key = hash2(key, fh);
        let skipped = |what: &str| skip.iter().any(|s| *s == step(what, f));
        if skipped("tool") || skipped("boolean") || skipped("round") {
            key = hash2(key, 1);
        }
        used_stages.push(key);
        if let Some(s) = cache.stages.get(&key) {
            stage = s.clone();
            model.cached += 1;
            continue;
        }
        // a fillet or chamfer: on the solid built so far
        if let Some(r) = &f.round {
            if let Some(acc) = stage.shape.clone() {
                let (res, missing) = if skipped("round") { (Some(Err("skipped, it crashed the kernel".to_string())), 0) } else { round(&acc, f, r) };
                if missing > 0 {
                    stage.errors.push(format!("{}: {missing} edge{} not found", f.name, if missing > 1 { "s" } else { "" }));
                }
                match res {
                    Some(Ok(s)) => {
                        model.booleans += 1;
                        stage.shape = Some(Rc::new(s));
                    }
                    Some(Err(e)) => stage.errors.push(format!("{}: OpenCascade cannot make it at {} mm ({e})", f.name, r.size)),
                    None => {}
                }
            }
            cache.stages.insert(key, stage.clone());
            continue;
        }
        // the feature's solid
        let t = if skipped("tool") {
            Err("skipped, it crashed the kernel".to_string())
        } else if let Some(t) = cache.tools.get(&fh) {
            Ok(t.clone())
        } else {
            mark(&step("tool", f));
            tool(f).map(|t| {
                let t = Rc::new(t);
                cache.tools.insert(fh, t.clone());
                t
            })
        };
        used_tools.push(fh);
        let t = match t {
            Ok(t) => t,
            Err(e) => {
                stage.errors.push(format!("{}: {e}", f.name));
                cache.stages.insert(key, stage.clone());
                continue;
            }
        };
        // the boolean
        let next = match (&stage.shape, f.op) {
            (None, Op::Add) => Some(Ok(t)),
            (None, Op::Carve) => None,
            (Some(_), _) if skipped("boolean") => Some(Err("skipped, it crashed the kernel".to_string())),
            (Some(acc), op) => {
                mark(&step("boolean", f));
                model.booleans += 1;
                Some(if op == Op::Add { acc.fuse(&t) } else { acc.cut(&t) }.map(Rc::new))
            }
        };
        match next {
            Some(Ok(s)) => stage.shape = Some(s),
            Some(Err(e)) => stage.errors.push(format!("{}: {} failed ({e})", f.name, if f.op == Op::Add { "union" } else { "subtraction" })),
            None => {}
        }
        cache.stages.insert(key, stage.clone());
    }
    // keep what this document uses (the next request is most often the same
    // document with one feature changed)
    cache.stages.retain(|k, _| used_stages.contains(k));
    if cache.tools.len() > 256 {
        cache.tools.retain(|k, _| used_tools.contains(k));
    }
    model.errors = stage.errors.clone();
    if let Some(s) = &stage.shape {
        let (defl, angle) = if fine { FINE } else { COARSE };
        mark("mesh:0:0");
        match s.topo(defl, angle) {
            Ok(t) => {
                model.positions = t.positions;
                model.normals = t.normals;
                model.triangles = t.triangles;
                model.tri_face = t.tri_face;
                model.faces = t
                    .faces
                    .iter()
                    .map(|f| Face {
                        tag: FaceTag::unpack(f.tag),
                        kind: surf(f.info.kind),
                        point: f.info.point,
                        normal: f.info.normal,
                        axis_origin: f.info.axis_origin,
                        axis_dir: f.info.axis_dir,
                        radius: f.info.radius,
                    })
                    .collect();
                model.edges = t
                    .edges
                    .into_iter()
                    .map(|e| Edge {
                        index: e.index,
                        faces: e.faces,
                        kind: match e.kind {
                            forme_occt::EdgeKind::Line => EdgeKind::Line,
                            forme_occt::EdgeKind::Circle => EdgeKind::Circle,
                            forme_occt::EdgeKind::Curve => EdgeKind::Curve,
                        },
                        seam: e.seam,
                        pts: e.pts.iter().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect(),
                    })
                    .collect();
            }
            Err(e) => model.errors.push(format!("meshing failed ({e})")),
        }
        // the quads: a segment about a fourteenth of the part's size
        if quads {
            mark("quads:0:0");
            let diag = model.bounds().map(|(lo, hi)| ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt() as f64).unwrap_or(20.0);
            match crate::quads::convert(s, (diag / 14.0).clamp(0.5, 20.0)) {
                Ok(q) => {
                    model.quads = Some(crate::model::Quads {
                        positions: q.positions.iter().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect(),
                        quads: q.quads,
                        quad_face: q.quad_face,
                        skipped: q.skipped.into_iter().map(|(f, s)| (f as u32, s)).collect(),
                    })
                }
                Err(e) => model.errors.push(format!("quads: {e}")),
            }
        }
    }
    model.ms = t0.elapsed().as_secs_f32() * 1000.0;
    model
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::PrimKind;
    use glam::DQuat;

    #[test]
    fn cube_with_a_hole() {
        let mut d = Doc::default();
        let cube = d.add(Op::Add, PrimKind::Box.default_prim(), DVec3::ZERO, DQuat::IDENTITY);
        let hole = d.add(Op::Carve, Prim::Cylinder { radius: 4.0, height: 30.0 }, DVec3::new(0.0, -5.0, 0.0), DQuat::IDENTITY);
        let mut cache = Cache::default();
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.booleans, 1);
        // the cube's 6 faces and the hole's side
        assert_eq!(m.faces.len(), 7);
        let tags: Vec<FaceTag> = m.faces.iter().map(|f| f.tag.unwrap()).collect();
        assert_eq!(tags.iter().filter(|t| t.feature == cube).count(), 6);
        assert_eq!(tags.iter().filter(|t| t.feature == hole).collect::<Vec<_>>(), vec![&FaceTag { feature: hole, sub: 0 }]);
        // the top face is the cube's top (sub 2), a plane facing up at y = 20
        let top = m.faces.iter().find(|f| f.tag == Some(FaceTag { feature: cube, sub: 2 })).unwrap();
        assert_eq!(top.kind, SurfKind::Plane);
        assert!((top.normal[1] - 1.0).abs() < 1e-9 && (top.point[1] - 20.0).abs() < 1e-9);
        // 12 cube edges, 2 rims, the hole's seam
        assert_eq!(m.edges.len(), 15);
        assert_eq!(m.edges.iter().filter(|e| e.seam).count(), 1);
        // moving the hole: the cube comes from the cache
        d.feature_mut(hole).unwrap().origin.x = 2.0;
        let m2 = build(&d, false, false, &[], &mut cache);
        assert_eq!((m2.cached, m2.booleans), (1, 1));
        // a step to skip: the hole is left out
        let s = step("boolean", d.feature(hole).unwrap());
        let m3 = build(&d, false, false, &[s], &mut cache);
        assert_eq!(m3.faces.len(), 6);
        assert_eq!(m3.errors.len(), 1);
        // a carve first: nothing to cut, no error
        let mut d2 = Doc::default();
        d2.add(Op::Carve, PrimKind::Box.default_prim(), DVec3::ZERO, DQuat::IDENTITY);
        let m4 = build(&d2, true, false, &[], &mut Cache::default());
        assert!(m4.is_empty() && m4.errors.is_empty());
        // round trip through the wire
        assert_eq!(Model::decode(&m.encode()).unwrap(), m);
    }

    /// The tag of a face of the built model, packed.
    fn tag(feature: u32, sub: u16) -> i64 {
        FaceTag { feature, sub }.pack()
    }

    #[test]
    fn sketches_are_profiles() {
        // an L of 20 × 20 mm, 10 mm high, its inside corner a curve point
        let mut d = Doc::default();
        let l = d.add(Op::Add, Prim::Profile { x: 20.0, y: 10.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let pr = crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, -0.2], [-0.2, -0.2], [-0.2, 0.5], [-0.5, 0.5]], smooth: vec![false, false, false, true, false, false] };
        let pieces = pr.pieces(20.0, 20.0).len();
        d.feature_mut(l).unwrap().profile = Some(pr);
        let mut cache = Cache::default();
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, hi) = m.bounds().unwrap();
        assert!((lo[1] - 0.0).abs() < 1e-3 && (hi[1] - 10.0).abs() < 1e-3, "{lo:?} {hi:?}");
        assert!((lo[0] + 10.0).abs() < 1e-3 && (hi[2] - 10.0).abs() < 1e-3, "{lo:?} {hi:?}");
        // top 1, bottom 2, one side per piece, each its own tag
        let subs: Vec<u16> = m.faces.iter().filter_map(|f| f.tag).filter(|t| t.feature == l).map(|t| t.sub).collect();
        assert_eq!(subs.len(), pieces + 2, "{subs:?}");
        assert!(subs.contains(&1) && subs.contains(&2));
        let mut sides: Vec<u16> = subs.iter().copied().filter(|s| *s >= 3).collect();
        sides.sort();
        sides.dedup();
        assert_eq!(sides.len(), pieces, "{subs:?}");
        // the side after point 0 (piece 1: the front, z = -10) is sub 4: its
        // top edge fillets
        let front = m.faces.iter().find(|f| f.tag == Some(FaceTag { feature: l, sub: 4 })).unwrap();
        assert!(front.normal[2] < -0.99, "{:?}", front.normal);
        d.add_round(1.0, false, vec![EdgeRef { faces: [tag(l, 1), tag(l, 4)], at: DVec3::new(0.0, 10.0, -10.0) }]);
        let m2 = build(&d, true, false, &[], &mut cache);
        assert!(m2.errors.is_empty(), "{:?}", m2.errors);
        // a sketch carved into a block, turned upside down (it grows down
        // from the block's top)
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 40.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let c = d.add(Op::Carve, Prim::Profile { x: 20.0, y: 5.0, z: 20.0 }, DVec3::new(0.0, 20.0, 0.0), DQuat::from_rotation_x(std::f64::consts::PI));
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.0, 0.5]], smooth: vec![false, false, true] });
        let m3 = build(&d, true, false, &[], &mut cache);
        assert!(m3.errors.is_empty(), "{:?}", m3.errors);
        assert!(m3.faces.iter().any(|f| f.tag.map(|t| t.feature) == Some(c)), "the pocket's faces carry the sketch's tags");
        // a bow tie crosses itself: an error, the block stays
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, 0.5], [0.5, -0.5], [-0.5, 0.5]], smooth: vec![false; 4] });
        let m4 = build(&d, true, false, &[], &mut cache);
        assert_eq!(m4.errors.len(), 1, "{:?}", m4.errors);
    }

    #[test]
    fn fillets_find_their_edges() {
        let mut d = Doc::default();
        let cube = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let boss = d.add(Op::Add, Prim::Cylinder { radius: 6.0, height: 10.0 }, DVec3::new(8.0, 20.0, 0.0), DQuat::IDENTITY);
        // the cube's top front edge (top, +Z) and the boss's foot (side, cube top)
        let edges = vec![
            EdgeRef { faces: [tag(cube, 2), tag(cube, 4)], at: DVec3::new(0.0, 20.0, 15.0) },
            EdgeRef { faces: [tag(boss, 0), tag(cube, 2)], at: DVec3::new(14.0, 20.0, 0.0) },
        ];
        let r = d.add_round(2.0, false, edges);
        let mut cache = Cache::default();
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        // two new fillet faces, tagged with the fillet
        assert_eq!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).count(), 2);
        // the boss moves: its foot is found again, from the cache of the cube
        d.feature_mut(boss).unwrap().origin.x = -8.0;
        let m2 = build(&d, true, false, &[], &mut cache);
        assert!(m2.errors.is_empty(), "{:?}", m2.errors);
        assert_eq!(m2.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).count(), 2);
        // a chamfer instead
        d.feature_mut(r).unwrap().round.as_mut().unwrap().chamfer = true;
        let m3 = build(&d, true, false, &[], &mut cache);
        assert!(m3.errors.is_empty(), "{:?}", m3.errors);
        let cone = m3.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).any(|f| f.kind == SurfKind::Cone);
        assert!(cone, "the boss's foot chamfered is a cone");
        // the boss gone: one edge not found, the other still rounded
        d.feature_mut(boss).unwrap().visible = false;
        let m4 = build(&d, true, false, &[], &mut cache);
        assert_eq!(m4.errors.len(), 1, "{:?}", m4.errors);
        assert!(m4.errors[0].contains("1 edge not found"));
        assert_eq!(m4.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).count(), 1);
        // too big: the fillet fails, the part stays as it was before it
        d.feature_mut(boss).unwrap().visible = true;
        let rr = d.feature_mut(r).unwrap().round.as_mut().unwrap();
        rr.chamfer = false;
        rr.size = 30.0;
        let m5 = build(&d, true, false, &[], &mut cache);
        assert_eq!(m5.errors.len(), 1, "{:?}", m5.errors);
        assert!(m5.faces.iter().all(|f| f.tag.map(|t| t.feature) != Some(r)));
        // of the two edges joining the cube's top and a side split by a slot,
        // the one nearest the point is rounded
        let mut d2 = Doc::default();
        let c2 = d2.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d2.add(Op::Carve, Prim::Box { x: 8.0, y: 10.0, z: 40.0 }, DVec3::new(0.0, 12.0, 0.0), DQuat::IDENTITY);
        let r2 = d2.add_round(1.0, false, vec![EdgeRef { faces: [tag(c2, 2), tag(c2, 4)], at: DVec3::new(15.0, 20.0, 15.0) }]);
        let m6 = build(&d2, true, false, &[], &mut Cache::default());
        assert!(m6.errors.is_empty(), "{:?}", m6.errors);
        let ff: Vec<&Face> = m6.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r2)).collect();
        assert_eq!(ff.len(), 1);
        let on_right = m6.triangles.iter().zip(&m6.tri_face).filter(|(_, f)| m6.faces[**f as usize].tag.map(|t| t.feature) == Some(r2)).all(|(t, _)| t.iter().all(|v| m6.positions[*v as usize][0] > 3.9));
        assert!(on_right, "the right piece of the edge");
    }

    #[test]
    fn rotated_and_on_a_face() {
        // a cylinder lying on its side (axis along X) on top of a cube
        let mut d = Doc::default();
        d.add(Op::Add, PrimKind::Box.default_prim(), DVec3::ZERO, DQuat::IDENTITY);
        let c = d.add(Op::Add, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-15.0, 20.0, 0.0), DQuat::from_rotation_z(-std::f64::consts::FRAC_PI_2));
        let f = d.feature(c).unwrap();
        assert!((f.axis() - DVec3::X).length() < 1e-9);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, hi) = m.bounds().unwrap();
        assert!((lo[0] + 15.0).abs() < 0.01 && (hi[0] - 15.0).abs() < 0.01, "{lo:?} {hi:?}");
        assert!((hi[1] - 25.0).abs() < 0.01);
    }
}
