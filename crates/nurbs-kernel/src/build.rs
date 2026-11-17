//! The document built with OpenCascade (crate forme-occt): each feature's
//! solid, its faces tagged (`FaceTag`), then the booleans in order. Used by
//! the kernel process; on iPad it will run in the app itself.
//!
//! Caches, for live editing: the solid of each feature (by content) and the
//! result after each feature (by the content of everything up to it). While
//! one feature moves, only the booleans from it onwards are made again.

use crate::doc::{Doc, Feature, Op, Prim};
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

/// Which face of the primitive (local frame) a face is, from its normal.
fn sub_of(prim: &Prim, kind: OccSurf, n: DVec3) -> u16 {
    match prim {
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
    }
}

/// The feature's solid in the world, its faces tagged.
fn tool(f: &Feature) -> Result<Shape, String> {
    let h = f.prim.height().max(0.01);
    let mut s = match f.prim {
        Prim::Box { x, z, .. } => Shape::cuboid(x.max(0.01), h, z.max(0.01))?,
        Prim::Cylinder { radius, .. } => Shape::cylinder(radius.max(0.01), h)?,
    };
    // the OCC primitives are centred: tag them in their own frame
    let tags: Vec<i64> = (0..s.face_count())
        .map(|i| {
            let info = s.face_info(i);
            let sub = info.map(|fi| sub_of(&f.prim, fi.kind, DVec3::from_array(fi.normal))).unwrap_or(0);
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

fn hash2(a: u64, b: u64) -> u64 {
    crate::doc::fnv(&[a.to_le_bytes(), b.to_le_bytes()].concat())
}

/// Builds the document. Never fails: what cannot be made is listed in
/// `Model::errors` and left out.
pub fn build(doc: &Doc, fine: bool, skip: &[String], cache: &mut Cache) -> Model {
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
        if skipped("tool") || skipped("boolean") {
            key = hash2(key, 1);
        }
        used_stages.push(key);
        if let Some(s) = cache.stages.get(&key) {
            stage = s.clone();
            model.cached += 1;
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
        let m = build(&d, true, &[], &mut cache);
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
        let m2 = build(&d, false, &[], &mut cache);
        assert_eq!((m2.cached, m2.booleans), (1, 1));
        // a step to skip: the hole is left out
        let s = step("boolean", d.feature(hole).unwrap());
        let m3 = build(&d, false, &[s], &mut cache);
        assert_eq!(m3.faces.len(), 6);
        assert_eq!(m3.errors.len(), 1);
        // a carve first: nothing to cut, no error
        let mut d2 = Doc::default();
        d2.add(Op::Carve, PrimKind::Box.default_prim(), DVec3::ZERO, DQuat::IDENTITY);
        let m4 = build(&d2, true, &[], &mut Cache::default());
        assert!(m4.is_empty() && m4.errors.is_empty());
        // round trip through the wire
        assert_eq!(Model::decode(&m.encode()).unwrap(), m);
    }

    #[test]
    fn rotated_and_on_a_face() {
        // a cylinder lying on its side (axis along X) on top of a cube
        let mut d = Doc::default();
        d.add(Op::Add, PrimKind::Box.default_prim(), DVec3::ZERO, DQuat::IDENTITY);
        let c = d.add(Op::Add, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-15.0, 20.0, 0.0), DQuat::from_rotation_z(-std::f64::consts::FRAC_PI_2));
        let f = d.feature(c).unwrap();
        assert!((f.axis() - DVec3::X).length() < 1e-9);
        let m = build(&d, true, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, hi) = m.bounds().unwrap();
        assert!((lo[0] + 15.0).abs() < 0.01 && (hi[0] - 15.0).abs() < 0.01, "{lo:?} {hi:?}");
        assert!((hi[1] - 25.0).abs() < 0.01);
    }
}
