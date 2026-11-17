//! forme-occ: the Forme NURBS kernel process (Romain, 17 nov. 2026: robust
//! booleans and fillets). Reads a scene (JSON) on stdin, builds it with
//! OpenCascade (crate forme-occt), writes the mesh (`nurbs_core::proto`) on
//! stdout.
//!
//! OpenCascade's errors come back as `Err` (a fillet it cannot make leaves
//! the edges sharp and is listed). A hard crash inside OpenCascade would
//! still kill the process: before each operation it writes
//! `STEP <what>:<node id>` on stderr, and the app reruns it with
//! `--skip <what>:<id>`.
//!
//! Mapping from Forme: Add = union, Carve = cut; the node's Blend = fillet of
//! the junction edges of its boolean (Chamfer kind = chamfer); local
//! junction fillets (blend zones) = fillet of the junction edges along the
//! zone; Round / Chamfer / per-edge fillets of a box or a cylinder = fillets
//! / chamfers of the primitive's own edges; repetitions and mirrors = copies.

use forme_core::scene::{BlendKind, CombineMode, NodeId, PrimKind, ProfileMode, Scene, Shape as FShape};
use forme_occt::{Edge, EdgeKind, Shape};
use glam::{DVec3, Mat4, Vec3};
use nurbs_core::{NurbsMesh, NurbsModel};
use std::collections::HashSet;
use std::io::{Read, Write};

/// Mesh chord deflection (mm) and angular deflection (rad).
const MESH_TOL: f64 = 0.02;
const MESH_ANGLE: f64 = 0.2;
/// Distance under which an edge lies on a target curve (mm).
const ON_CURVE: f64 = 0.02;
/// Points sampled along each edge.
const SAMPLES: usize = 9;

fn dv(v: Vec3) -> DVec3 {
    DVec3::new(v.x as f64, v.y as f64, v.z as f64)
}

fn pts(e: &Edge) -> Vec<DVec3> {
    e.pts.iter().map(|p| DVec3::from_array(*p)).collect()
}

fn step(s: &str) {
    eprintln!("STEP {s}");
    let _ = std::io::stderr().flush();
}

fn dist_to_polyline(p: DVec3, pl: &[DVec3]) -> f64 {
    if pl.len() == 1 {
        return (pl[0] - p).length();
    }
    let mut best = f64::MAX;
    for w in pl.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = b - a;
        let t = if ab.length_squared() < 1e-18 { 0.0 } else { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) };
        best = best.min((a + ab * t - p).length());
    }
    best
}

/// Groups (radius → curves) from a list of (radius, curve): fillets before
/// chamfers (negative), big radii first, zero radii dropped.
fn group(items: Vec<(f64, Vec<DVec3>)>) -> Vec<(f64, Vec<Vec<DVec3>>)> {
    let mut out: Vec<(f64, Vec<Vec<DVec3>>)> = vec![];
    for (r, p) in items {
        if r.abs() < 1e-4 {
            continue;
        }
        match out.iter_mut().find(|(q, _)| (q - r).abs() < 1e-6) {
            Some((_, v)) => v.push(p),
            None => out.push((r, vec![p])),
        }
    }
    out.sort_by(|a, b| (b.0 > 0.0).cmp(&(a.0 > 0.0)).then(b.0.abs().partial_cmp(&a.0.abs()).unwrap()));
    out
}

/// Fillets (radius > 0) or chamfers (< 0) the edges of `shape` lying on the
/// target curves, one operation per radius. A group OpenCascade cannot make
/// is left sharp and reported in `failed`.
fn round_edges(mut shape: Shape, groups: &[(f64, Vec<Vec<DVec3>>)], failed: &mut Vec<String>) -> Shape {
    for (r, targets) in groups {
        let idx: Vec<i32> = shape
            .edges(SAMPLES)
            .iter()
            .filter(|e| {
                let p = pts(e);
                targets.iter().any(|t| dist_to_polyline(p[p.len() / 2], t) < ON_CURVE && dist_to_polyline(p[0], t) < ON_CURVE)
            })
            .map(|e| e.index)
            .collect();
        if idx.is_empty() {
            continue;
        }
        let res = if *r > 0.0 { shape.fillet(&idx, *r) } else { shape.chamfer(&idx, -*r) };
        match res {
            Ok(s) => shape = s,
            Err(e) => failed.push(format!("{} {:.2} mm ({e})", if *r > 0.0 { "fillet" } else { "chamfer" }, r.abs())),
        }
    }
    shape
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
fn primitive(sh: &FShape, own_fillets: bool, ignored: &mut Vec<String>, name: &str) -> Result<Option<Shape>, String> {
    let s = sh.size;
    let base = match sh.kind {
        PrimKind::Box => {
            let h = s.max(Vec3::splat(0.01));
            Shape::cuboid(2.0 * h.x as f64, 2.0 * h.y as f64, 2.0 * h.z as f64)?
        }
        PrimKind::Cylinder => Shape::cylinder(s.x.max(0.01) as f64, 2.0 * s.y.max(0.01) as f64)?,
        PrimKind::Sphere => {
            if (s.y - s.x).abs() > 1e-3 || (s.z - s.x).abs() > 1e-3 {
                ignored.push(format!("{name}: ellipsoid built as a sphere"));
            }
            Shape::sphere(s.x.max(0.01) as f64)?
        }
        PrimKind::Capsule => {
            let (r, hh) = (s.x.max(0.01) as f64, s.y.max(0.0) as f64);
            let ball = |y: f64| Shape::sphere(r).and_then(|b| b.transformed([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, y], [0.0, 0.0, 1.0, 0.0]]));
            if hh < 1e-6 {
                ball(0.0)?
            } else {
                let body = Shape::cylinder(r, 2.0 * hh)?;
                body.fuse(&ball(hh)?)?.fuse(&ball(-hh)?)?
            }
        }
        PrimKind::Torus => {
            let big = s.x.max(0.02) as f64;
            let small = (s.z as f64).clamp(0.01, big * 0.99);
            Shape::torus(big, small)?
        }
        // a closed outline extruded on both sides of its plane (the curve
        // points are true curves here, a polyline in the SDF)
        PrimKind::Profile if sh.profile.as_ref().is_some_and(|p| p.mode == ProfileMode::Extrude) => {
            let p = sh.profile.as_ref().unwrap();
            let pts: Vec<[f64; 2]> = p.points.iter().map(|q| [q.pos.x as f64, q.pos.y as f64]).collect();
            let smooth: Vec<bool> = p.points.iter().map(|q| q.smooth).collect();
            Shape::prism(&pts, &smooth, s.z.max(0.01) as f64)?
        }
        k => {
            ignored.push(format!("{name}: {} not in NURBS yet", k.label()));
            return Ok(None);
        }
    };
    if !own_fillets {
        return Ok(Some(base));
    }
    // own edges: box edges, cylinder rims
    let max_r = match sh.kind {
        PrimKind::Box => s.min_element() as f64 * 0.999,
        PrimKind::Cylinder => s.x.min(s.y) as f64 * 0.999,
        _ => 0.0,
    };
    let uniform = if sh.chamfer > 0.0 { -(sh.chamfer as f64) } else { sh.round as f64 };
    let mut items: Vec<(f64, Vec<DVec3>)> = vec![];
    if matches!(sh.kind, PrimKind::Box | PrimKind::Cylinder) {
        for e in base.edges(SAMPLES) {
            let p = pts(&e);
            let idx = match sh.kind {
                PrimKind::Box => box_edge_index(p[0], p[p.len() - 1]),
                _ => {
                    if e.kind != EdgeKind::Circle {
                        continue;
                    }
                    if p[0].y > 0.0 {
                        0
                    } else {
                        1
                    }
                }
            };
            let r = if sh.edges.is_empty() { uniform } else { sh.edges.get(idx).copied().unwrap_or(0.0) as f64 };
            items.push((r.clamp(-max_r, max_r), p));
        }
    } else if sh.kind == PrimKind::Profile && uniform != 0.0 {
        // like the SDF: every edge, the outline's corners and both caps
        let r = uniform.clamp(-(s.z as f64) * 0.999, s.z as f64 * 0.999);
        for e in base.edges(SAMPLES) {
            items.push((r, pts(&e)));
        }
    } else if sh.round > 0.0 || sh.chamfer > 0.0 {
        ignored.push(format!("{name}: rounding of this shape"));
    }
    let groups = group(items);
    if groups.is_empty() {
        return Ok(Some(base));
    }
    let mut failed = vec![];
    let out = round_edges(base, &groups, &mut failed);
    for f in failed {
        ignored.push(format!("{name}: {f}, left sharp"));
    }
    Ok(Some(out))
}

/// Places a shape by a world matrix (rotation, uniform scale, mirrors,
/// translation).
fn place(s: &Shape, m: Mat4) -> Result<Shape, String> {
    let row = |r: usize| [m.x_axis[r] as f64, m.y_axis[r] as f64, m.z_axis[r] as f64, m.w_axis[r] as f64];
    s.transformed([row(0), row(1), row(2)])
}

struct Ctx {
    skip: HashSet<String>,
    model: NurbsModel,
    material: Option<u32>,
}

/// Fillets the junction edges of a boolean result: the node's blend on all
/// of them, a blend zone's radius on those along the zone.
fn junction(ctx: &mut Ctx, shape: Shape, scene: &Scene, id: NodeId, m: Mat4, name: &str) -> Shape {
    let n = &scene.nodes[&id];
    let key = format!("fillet:{id}");
    let blend = n.combine.blend as f64;
    let sec = shape.section_edges();
    // the junction's B-rep edges, filleted or not: the Edges tool lights
    // exactly the ones a fillet will round
    let edges: Vec<Vec<DVec3>> = sec.iter().filter_map(|&i| shape.edge(i, SAMPLES).map(|e| pts(&e))).collect();
    for p in &edges {
        ctx.model.junctions.push((id, p.iter().map(|q| [q.x as f32, q.y as f32, q.z as f32]).collect()));
    }
    if (blend <= 0.0 && n.blend_zones.is_empty()) || sec.is_empty() {
        return shape;
    }
    if ctx.skip.contains(&key) {
        ctx.model.ignored.push(format!("{name}: junction fillet crashed the kernel, left sharp"));
        return shape;
    }
    // a chamfer blend = chamfer of the junction (negative radius)
    let blend = if n.combine.kind == BlendKind::Chamfer { -blend } else { blend };
    let zones: Vec<(f64, Vec<DVec3>)> = n.blend_zones.iter().map(|z| (z.radius as f64, z.pts.iter().map(|p| dv(m.transform_point3(*p))).collect())).collect();
    let mut items: Vec<(f64, Vec<DVec3>)> = vec![];
    for p in edges {
        let c = p[p.len() / 2];
        // a zone rounds the edges along it, and the edge it was picked on
        // (its middle): a closed rim is one B-rep edge whose own middle can
        // be anywhere on it, and OCC fillets a whole edge
        let on = |z: &Vec<DVec3>| !z.is_empty() && (dist_to_polyline(c, z) < 0.5 || dist_to_polyline(z[z.len() / 2], &p) < 0.5);
        let r = zones.iter().filter(|(_, z)| on(z)).map(|(r, _)| *r).fold(None, |a: Option<f64>, r| Some(a.map_or(r, |a| a.max(r)))).unwrap_or(blend);
        items.push((r, p));
    }
    let groups = group(items);
    if groups.is_empty() {
        return shape;
    }
    step(&key);
    let mut failed = vec![];
    let out = round_edges(shape, &groups, &mut failed);
    for f in failed {
        ctx.model.ignored.push(format!("{name}: junction {f}, left sharp"));
    }
    out
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
            ctx.model.ignored.push(format!("{name}: rounding crashed the kernel, left sharp"));
        }
        step(&own_key);
        let base = match primitive(sh, own, &mut ctx.model.ignored, &name) {
            Ok(Some(b)) => b,
            Ok(None) => continue,
            Err(e) => {
                ctx.model.ignored.push(format!("{name}: {e}"));
                continue;
            }
        };
        for (m, _) in nurbs_core::placements(scene, id) {
            let s = match place(&base, m) {
                Ok(s) => s,
                Err(e) => {
                    ctx.model.ignored.push(format!("{name}: placement ({e})"));
                    continue;
                }
            };
            match n.combine.mode {
                CombineMode::Add => {
                    let key = format!("union:{id}");
                    ctx.model.shapes += 1;
                    ctx.material.get_or_insert(n.material);
                    acc = Some(match acc.take() {
                        None => s,
                        Some(a) if ctx.skip.contains(&key) => {
                            ctx.model.ignored.push(format!("{name}: the union crashed the kernel"));
                            a
                        }
                        Some(a) => {
                            ctx.model.booleans += 1;
                            step(&key);
                            match a.fuse(&s) {
                                Ok(r) => junction(&mut ctx, r, scene, id, m, &name),
                                Err(e) => {
                                    ctx.model.ignored.push(format!("{name}: the union failed ({e})"));
                                    a
                                }
                            }
                        }
                    });
                }
                CombineMode::Carve => {
                    let key = format!("cut:{id}");
                    ctx.model.shapes += 1;
                    let Some(a) = acc.take() else { continue };
                    if ctx.skip.contains(&key) {
                        ctx.model.ignored.push(format!("{name}: the subtraction crashed the kernel"));
                        acc = Some(a);
                        continue;
                    }
                    ctx.model.booleans += 1;
                    step(&key);
                    acc = Some(match a.cut(&s) {
                        Ok(r) => junction(&mut ctx, r, scene, id, m, &name),
                        Err(e) => {
                            ctx.model.ignored.push(format!("{name}: the subtraction failed ({e})"));
                            a
                        }
                    });
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
        match a.mesh(MESH_TOL, MESH_ANGLE) {
            Ok(mesh) => {
                let mat = ctx.material.unwrap_or(0);
                let mut out = NurbsMesh::default();
                out.positions = mesh.positions;
                out.normals = mesh.normals;
                out.tris = mesh.triangles;
                out.tri_material = vec![mat; out.tris.len()];
                ctx.model.mesh = out;
            }
            Err(e) => ctx.model.ignored.push(format!("mesh: {e}")),
        }
        // the edges, drawn as thin lines (where each fillet starts and ends)
        if !ctx.skip.contains("lines:0") {
            step("lines:0");
            match a.edge_lines(MESH_TOL, MESH_ANGLE) {
                Ok(lines) => ctx.model.edges = lines.into_iter().map(|l| l.into_iter().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect()).collect(),
                Err(e) => ctx.model.ignored.push(format!("edges: {e}")),
            }
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
