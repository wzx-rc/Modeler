//! forme-occ: the Forme NURBS kernel process (Romain, 17 nov. 2026: robust
//! booleans and fillets). Reads a scene (JSON) on stdin, builds it with
//! OpenCascade, writes the mesh (`nurbs_core::proto`) on stdout.
//!
//! OpenCascade throws C++ exceptions that the Rust binding cannot catch: the
//! process aborts. That is why it runs apart from the app: before each risky
//! operation it writes `STEP <what>:<node id>` on stderr, and the app reruns
//! it with `--skip <what>:<id>` when a fillet is what failed.
//!
//! Mapping from Forme: Add = union, Carve = cut; the node's Raccord (blend)
//! = fillet of the junction edges of its boolean; local junction fillets
//! (blend zones) = fillet of the junction edges along the zone; Rondeur /
//! Chanfrein / per-edge fillets of a box or a cylinder = fillets / chamfers
//! of the primitive's own edges; repetitions and mirrors = copies.

use dglam::DVec3;
use forme_core::scene::{BlendKind, CombineMode, NodeId, PrimKind, Scene, Shape as FShape};
use glam::{Mat3, Mat4, Quat, Vec3};
use nurbs_core::{NurbsMesh, NurbsModel};
use opencascade::primitives::{BooleanShape, Edge, EdgeType, Shape};
use std::collections::HashSet;
use std::io::{Read, Write};

/// Mesh deflection (mm).
const MESH_TOL: f64 = 0.02;
/// Distance under which an edge lies on a target curve (mm).
const ON_CURVE: f64 = 0.02;

fn dv(v: Vec3) -> DVec3 {
    DVec3::new(v.x as f64, v.y as f64, v.z as f64)
}

fn step(s: &str) {
    eprintln!("STEP {s}");
    let _ = std::io::stderr().flush();
}

/// Points along an edge (its polyline), first to last.
fn samples(e: &Edge) -> Vec<DVec3> {
    let mut pts: Vec<DVec3> = e.approximation_segments().collect();
    if pts.len() < 2 {
        pts = vec![e.start_point(), e.end_point()];
    }
    if e.edge_type() == EdgeType::Line {
        let (a, b) = (e.start_point(), e.end_point());
        pts = (0..=8).map(|i| a.lerp(b, i as f64 / 8.0)).collect();
    }
    pts
}

fn mid(pts: &[DVec3]) -> DVec3 {
    if pts.len() == 2 {
        (pts[0] + pts[1]) * 0.5
    } else {
        pts[pts.len() / 2]
    }
}

fn dist_to_polyline(p: DVec3, pl: &[DVec3]) -> f64 {
    let mut best = f64::MAX;
    for w in pl.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = b - a;
        let t = if ab.length_squared() < 1e-18 { 0.0 } else { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) };
        best = best.min((a + ab * t - p).length());
    }
    if pl.len() == 1 {
        best = (pl[0] - p).length();
    }
    best
}

/// The distinct edges of a shape (the explorer gives each edge once per face).
fn distinct_edges(s: &Shape) -> Vec<(Edge, Vec<DVec3>)> {
    let mut out: Vec<(Edge, Vec<DVec3>)> = vec![];
    for e in s.edges() {
        let pts = samples(&e);
        let m = mid(&pts);
        if out.iter().any(|(_, q)| (mid(q) - m).length() < 1e-6 && (q[0] - pts[0]).length().min((q[0] - pts[pts.len() - 1]).length()) < 1e-6) {
            continue;
        }
        out.push((e, pts));
    }
    out
}

/// Fillets (radius > 0) or chamfers (< 0) the edges of `shape` that lie on
/// the target curves, one operation per radius (same radius = one operation,
/// so corners where they meet are blended).
fn round_edges(shape: Shape, groups: &[(f64, Vec<Vec<DVec3>>)]) -> Shape {
    let mut shape = shape;
    for (r, targets) in groups {
        let edges: Vec<Edge> = distinct_edges(&shape)
            .into_iter()
            .filter(|(_, pts)| targets.iter().any(|t| dist_to_polyline(mid(pts), t) < ON_CURVE && dist_to_polyline(pts[0], t) < ON_CURVE))
            .map(|(e, _)| e)
            .collect();
        if edges.is_empty() {
            continue;
        }
        shape = if *r > 0.0 { shape.fillet_edges(*r, &edges) } else { shape.chamfer_edges(-*r, &edges) };
    }
    shape
}

/// Groups (radius → curves) from a list of (radius, curve).
fn group(items: Vec<(f64, Vec<DVec3>)>) -> Vec<(f64, Vec<Vec<DVec3>>)> {
    let mut out: Vec<(f64, Vec<Vec<DVec3>>)> = vec![];
    for (r, pts) in items {
        if r.abs() < 1e-4 {
            continue;
        }
        match out.iter_mut().find(|(q, _)| (q - r).abs() < 1e-6) {
            Some((_, v)) => v.push(pts),
            None => out.push((r, vec![pts])),
        }
    }
    // fillets before chamfers, big radii first
    out.sort_by(|a, b| (b.0 > 0.0).cmp(&(a.0 > 0.0)).then(b.0.abs().partial_cmp(&a.0.abs()).unwrap()));
    out
}

/// Box edge index (Forme): axis*4 + k, k over the signs of the two other
/// axes (+,+) (-,+) (-,-) (+,-).
fn box_edge_index(a: DVec3, b: DVec3) -> usize {
    let d = (b - a).abs();
    let axis = if d.x >= d.y && d.x >= d.z { 0 } else if d.y >= d.z { 1 } else { 2 };
    let m = (a + b) * 0.5;
    let (o1, o2) = ((axis + 1) % 3, (axis + 2) % 3);
    let k = match (m[o1] > 0.0, m[o2] > 0.0) {
        (true, true) => 0,
        (false, true) => 1,
        (false, false) => 2,
        (true, false) => 3,
    };
    axis * 4 + k
}

/// The primitive in its own frame, with its own fillets and chamfers.
fn primitive(sh: &FShape, own_fillets: bool, ignored: &mut Vec<String>, name: &str) -> Option<Shape> {
    let s = sh.size;
    let base = match sh.kind {
        PrimKind::Box => {
            let h = s.max(Vec3::splat(0.01));
            Shape::box_centered(2.0 * h.x as f64, 2.0 * h.y as f64, 2.0 * h.z as f64)
        }
        PrimKind::Cylinder => Shape::cylinder_centered(DVec3::ZERO, s.x.max(0.01) as f64, DVec3::Y, 2.0 * s.y.max(0.01) as f64),
        PrimKind::Sphere => {
            if (s.y - s.x).abs() > 1e-3 || (s.z - s.x).abs() > 1e-3 {
                ignored.push(format!("{name}: ellipsoid built as a sphere"));
            }
            Shape::sphere(s.x.max(0.01) as f64).build()
        }
        PrimKind::Capsule => {
            let (r, hh) = (s.x.max(0.01) as f64, s.y.max(0.0) as f64);
            let ball = |y: f64| Shape::sphere(r).at(DVec3::new(0.0, y, 0.0)).build();
            if hh < 1e-6 {
                ball(0.0)
            } else {
                let body = Shape::cylinder_centered(DVec3::ZERO, r, DVec3::Y, 2.0 * hh);
                let u: Shape = body.union(&ball(hh)).shape;
                u.union(&ball(-hh)).shape
            }
        }
        PrimKind::Torus => {
            let big = s.x.max(0.02) as f64;
            let small = (s.z as f64).clamp(0.01, big * 0.99);
            Shape::torus().z_axis(DVec3::Y).radius_1(big).radius_2(small).build()
        }
        k => {
            ignored.push(format!("{name}: {} not in NURBS yet", k.label()));
            return None;
        }
    };
    if !own_fillets {
        return Some(base);
    }
    // own edges: box edges / cylinder rims
    let mut items: Vec<(f64, Vec<DVec3>)> = vec![];
    let max_r = match sh.kind {
        PrimKind::Box => s.min_element() as f64 * 0.999,
        PrimKind::Cylinder => s.x.min(s.y) as f64 * 0.999,
        _ => 0.0,
    };
    let uniform = if sh.chamfer > 0.0 { -(sh.chamfer as f64) } else { sh.round as f64 };
    if matches!(sh.kind, PrimKind::Box | PrimKind::Cylinder) {
        for (e, pts) in distinct_edges(&base) {
            let idx = match sh.kind {
                PrimKind::Box => box_edge_index(e.start_point(), e.end_point()),
                _ => {
                    if e.edge_type() != EdgeType::Circle {
                        continue;
                    }
                    if e.start_point().y > 0.0 {
                        0
                    } else {
                        1
                    }
                }
            };
            let r = if sh.edges.is_empty() { uniform } else { sh.edges.get(idx).copied().unwrap_or(0.0) as f64 };
            items.push((r.clamp(-max_r, max_r), pts));
        }
    } else if sh.round > 0.0 || sh.chamfer > 0.0 {
        ignored.push(format!("{name}: rounding of this shape"));
    }
    let groups = group(items);
    if groups.is_empty() {
        return Some(base);
    }
    Some(round_edges(base, &groups))
}

/// Places a shape by a world matrix (rotation, translation, mirrors).
fn place(s: Shape, m: Mat4) -> Shape {
    let l = Mat3::from_mat4(m);
    let mut rot = l;
    let mut s = s;
    if l.determinant() < 0.0 {
        // plane mirror across X = point reflection then half turn around X
        s = s.scaled(DVec3::ZERO, -1.0).mirrored(DVec3::ZERO, DVec3::X);
        rot = l * Mat3::from_diagonal(Vec3::new(-1.0, 1.0, 1.0));
    }
    let (axis, angle) = Quat::from_mat3(&rot).normalize().to_axis_angle();
    if angle.abs() > 1e-7 {
        s = s.rotated(dv(axis), angle as f64);
    }
    let t = m.w_axis.truncate();
    if t.length() > 0.0 {
        s = s.translated(dv(t));
    }
    s
}

struct Ctx {
    skip: HashSet<String>,
    model: NurbsModel,
    material: Option<u32>,
}

/// Fillets the junction edges of a boolean: the node's blend on all of them,
/// a blend zone's radius on those along the zone.
fn junction(ctx: &mut Ctx, b: BooleanShape, scene: &Scene, id: NodeId, m: Mat4, name: &str) -> Shape {
    let n = &scene.nodes[&id];
    let key = format!("fillet:{id}");
    let blend = n.combine.blend as f64;
    if (blend <= 0.0 && n.blend_zones.is_empty()) || b.new_edges.is_empty() {
        return b.shape;
    }
    // a chamfer blend = chamfer of the junction (negative radius)
    let blend = if n.combine.kind == BlendKind::Chamfer { -blend } else { blend };
    if ctx.skip.contains(&key) {
        ctx.model.ignored.push(format!("{name}: junction fillet failed, left sharp"));
        return b.shape;
    }
    let zones: Vec<(f64, Vec<DVec3>)> = n
        .blend_zones
        .iter()
        .map(|z| (z.radius as f64, z.pts.iter().map(|p| dv(m.transform_point3(*p))).collect()))
        .collect();
    let mut items: Vec<(f64, Vec<DVec3>)> = vec![];
    for e in &b.new_edges {
        let pts = samples(e);
        let c = mid(&pts);
        let r = zones.iter().filter(|(_, z)| dist_to_polyline(c, z) < 0.5).map(|(r, _)| *r).fold(None, |a: Option<f64>, r| Some(a.map_or(r, |a| a.max(r)))).unwrap_or(blend);
        items.push((r, pts));
    }
    let groups = group(items);
    if groups.is_empty() {
        return b.shape;
    }
    step(&key);
    round_edges(b.shape, &groups)
}

fn build(scene: &Scene, skip: HashSet<String>) -> NurbsModel {
    let t0 = std::time::Instant::now();
    let mut ctx = Ctx { skip, model: NurbsModel::default(), material: None };
    let mut acc: Option<Shape> = None;
    for id in nurbs_core::shape_order(scene) {
        let n = &scene.nodes[&id];
        let sh = n.shape().unwrap();
        let name = n.name.clone();
        if sh.shell > 0.0 {
            ctx.model.ignored.push(format!("{name}: shell"));
        }
        let own_key = format!("round:{id}");
        let own = !ctx.skip.contains(&own_key);
        if !own && (sh.round > 0.0 || sh.chamfer > 0.0 || !sh.edges.is_empty()) {
            ctx.model.ignored.push(format!("{name}: rounding failed, left sharp"));
        }
        for (m, _) in nurbs_core::placements(scene, id) {
            step(&own_key);
            let Some(base) = primitive(sh, own, &mut ctx.model.ignored, &name) else { break };
            let s = place(base, m);
            match n.combine.mode {
                CombineMode::Add => {
                    let key = format!("union:{id}");
                    if ctx.skip.contains(&key) && acc.is_some() {
                        ctx.model.ignored.push(format!("{name}: the NURBS union failed"));
                        break;
                    }
                    ctx.model.shapes += 1;
                    ctx.material.get_or_insert(n.material);
                    acc = Some(match acc.take() {
                        None => s,
                        Some(a) => {
                            ctx.model.booleans += 1;
                            step(&key);
                            let b = a.union(&s);
                            junction(&mut ctx, b, scene, id, m, &name)
                        }
                    });
                }
                CombineMode::Carve => {
                    let key = format!("cut:{id}");
                    if ctx.skip.contains(&key) {
                        ctx.model.ignored.push(format!("{name}: the NURBS subtraction failed"));
                        break;
                    }
                    ctx.model.shapes += 1;
                    if let Some(a) = acc.take() {
                        ctx.model.booleans += 1;
                        step(&key);
                        let b = a.subtract(&s);
                        acc = Some(junction(&mut ctx, b, scene, id, m, &name));
                    }
                }
                other => {
                    ctx.model.ignored.push(format!("{name}: mode {} not in NURBS yet", other.label()));
                    break;
                }
            }
        }
    }
    if let Some(a) = acc {
        step("mesh:0");
        match a.mesh_with_tolerance(MESH_TOL) {
            Ok(mesh) => {
                let mat = ctx.material.unwrap_or(0);
                let mut out = NurbsMesh::default();
                out.positions = mesh.vertices.iter().map(|p| [p.x as f32, p.y as f32, p.z as f32]).collect();
                out.normals = mesh.normals.iter().map(|p| [p.x as f32, p.y as f32, p.z as f32]).collect();
                out.tris = mesh.indices.chunks(3).map(|t| [t[0] as u32, t[1] as u32, t[2] as u32]).collect();
                out.tri_material = vec![mat; out.tris.len()];
                ctx.model.mesh = out;
            }
            Err(e) => ctx.model.ignored.push(format!("mesh: {e:?}")),
        }
    }
    ctx.model.ignored.dedup();
    ctx.model.millis = t0.elapsed().as_millis();
    ctx.model
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let skip: HashSet<String> = args
        .iter()
        .position(|a| a == "--skip")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.split(',').filter(|x| !x.is_empty()).map(|x| x.to_string()).collect())
        .unwrap_or_default();
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).expect("stdin");
    let res = match serde_json::from_slice::<Scene>(&input) {
        Ok(scene) => Ok(build(&scene, skip)),
        Err(e) => Err(format!("bad scene: {e}")),
    };
    let bytes = nurbs_core::proto::encode(&res);
    let mut out = std::io::stdout().lock();
    out.write_all(&bytes).expect("stdout");
    out.flush().ok();
}
