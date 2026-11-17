//! Forme NURBS (Romain, 17 nov. 2026): the same scene of shapes as Forme,
//! built as exact B-rep solids with the truck kernel instead of SDFs.
//! Each visible shape becomes a NURBS solid (box, cylinder, sphere,
//! capsule, torus) placed by its world matrix, then combined in tree order
//! with booleans: Add = union, Carve = difference. The result is
//! tessellated for the viewport.
//!
//! Not yet: rounding / chamfer / fillets (truck has none), blends, shells,
//! repetitions. They are ignored, and `NurbsModel::ignored` lists them.

use forme_core::scene::{CombineMode, NodeId, PrimKind, Scene, Shape};
use glam::{Mat4, Vec3};
use std::f64::consts::PI;
use truck_meshalgo::prelude::*;
use truck_modeling::*;

/// Triangles of the built part (world space, f32), one material per triangle.
#[derive(Clone, Debug, Default)]
pub struct NurbsMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Three corners per triangle (indices into positions / normals).
    pub tris: Vec<[u32; 3]>,
    pub tri_material: Vec<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct NurbsModel {
    pub mesh: NurbsMesh,
    /// Shapes combined, booleans done.
    pub shapes: usize,
    pub booleans: usize,
    /// What the NURBS build left out (English, for the UI).
    pub ignored: Vec<String>,
    pub millis: u128,
}

/// Boolean tolerance (mm).
fn tol() -> f64 {
    std::env::var("NURBS_TOL").ok().and_then(|v| v.parse().ok()).unwrap_or(0.05)
}
/// Tessellation tolerance (mm).
const MESH_TOL: f64 = 0.03;

fn p3(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}

/// A box of half extents `h`, centred on the origin.
fn make_box(h: Vec3) -> Solid {
    let (x, y, z) = (h.x as f64, h.y as f64, h.z as f64);
    let v = builder::vertex(p3(-x, -y, -z));
    let e = builder::tsweep(&v, Vector3::unit_x() * (2.0 * x));
    let f = builder::tsweep(&e, Vector3::unit_y() * (2.0 * y));
    builder::tsweep(&f, Vector3::unit_z() * (2.0 * z))
}

/// A cylinder of radius `r` along Y, from -hh to +hh.
fn make_cylinder(r: f64, hh: f64) -> Solid {
    let v = builder::vertex(p3(r, -hh, 0.0));
    let circle = builder::rsweep(&v, p3(0.0, -hh, 0.0), Vector3::unit_y(), Rad(7.0));
    let disk = builder::try_attach_plane(&[circle]).expect("disk");
    builder::tsweep(&disk, Vector3::unit_y() * (2.0 * hh))
}

/// A solid of revolution around Y from a profile wire whose two ends lie on
/// the axis.
fn revolve(wire: &Wire) -> Solid {
    let shell = builder::cone(wire, Vector3::unit_y(), Rad(7.0));
    Solid::new(vec![shell])
}

fn make_sphere(r: f64) -> Solid {
    // two quarter arcs: truck's booleans fail on a single half-circle profile
    let s = std::f64::consts::FRAC_1_SQRT_2 * r;
    let top = builder::vertex(p3(0.0, r, 0.0));
    let eq = builder::vertex(p3(r, 0.0, 0.0));
    let bot = builder::vertex(p3(0.0, -r, 0.0));
    let a0 = builder::circle_arc(&top, &eq, p3(s, s, 0.0));
    let a1 = builder::circle_arc(&eq, &bot, p3(s, -s, 0.0));
    revolve(&Wire::from(vec![a0, a1]))
}

/// Capsule: radius `r`, straight part from -hh to +hh, hemispheres beyond.
fn make_capsule(r: f64, hh: f64) -> Solid {
    if hh < 1e-6 {
        return make_sphere(r);
    }
    let s = std::f64::consts::FRAC_1_SQRT_2 * r;
    let top = builder::vertex(p3(0.0, hh + r, 0.0));
    let a = builder::vertex(p3(r, hh, 0.0));
    let b = builder::vertex(p3(r, -hh, 0.0));
    let bot = builder::vertex(p3(0.0, -hh - r, 0.0));
    let e0 = builder::circle_arc(&top, &a, p3(s, hh + s, 0.0));
    let e1 = builder::line(&a, &b);
    let e2 = builder::circle_arc(&b, &bot, p3(s, -hh - s, 0.0));
    revolve(&Wire::from(vec![e0, e1, e2]))
}

/// Torus around Y: ring radius `big`, tube radius `small`.
fn make_torus(big: f64, small: f64) -> Solid {
    let v = builder::vertex(p3(big + small, 0.0, 0.0));
    let circle: Wire = builder::rsweep(&v, p3(big, 0.0, 0.0), -Vector3::unit_z(), Rad(7.0));
    let shell: Shell = builder::rsweep(&circle, Point3::origin(), Vector3::unit_y(), Rad(7.0));
    Solid::new(vec![shell])
}

/// Signed volume of the tessellated solid (negative = inside out).
pub fn volume(s: &Solid) -> f64 {
    let mesh = s.triangulation(MESH_TOL).to_polygon();
    let pos = mesh.positions();
    let mut v = 0.0;
    for tri in mesh.faces().triangle_iter() {
        let (a, b, c) = (pos[tri[0].pos], pos[tri[1].pos], pos[tri[2].pos]);
        v += a.to_vec().dot(b.to_vec().cross(c.to_vec())) / 6.0;
    }
    v
}

/// The primitive in its local frame, or why it cannot be built.
pub fn primitive(sh: &Shape) -> std::result::Result<Solid, String> {
    let s = sh.size;
    let solid = match sh.kind {
        PrimKind::Box => make_box(s.max(Vec3::splat(0.01))),
        PrimKind::Cylinder => make_cylinder(s.x.max(0.01) as f64, s.y.max(0.01) as f64),
        PrimKind::Sphere => {
            let r = s.x.max(0.01) as f64;
            let sp = make_sphere(r);
            if (s.y - s.x).abs() > 1e-3 || (s.z - s.x).abs() > 1e-3 {
                // ellipsoid: a scaled sphere
                builder::scaled(&sp, Point3::origin(), Vector3::new(1.0, s.y as f64 / r, s.z as f64 / r))
            } else {
                sp
            }
        }
        PrimKind::Capsule => make_capsule(s.x.max(0.01) as f64, s.y.max(0.0) as f64),
        PrimKind::Torus => make_torus(s.x.max(0.02) as f64, s.z.clamp(0.01, s.x.max(0.02) * 0.99) as f64),
        k => return Err(format!("{}: not in NURBS yet", k.label())),
    };
    Ok(solid)
}

fn to_matrix(m: Mat4) -> Matrix4 {
    let c = m.to_cols_array();
    let d: Vec<f64> = c.iter().map(|v| *v as f64).collect();
    Matrix4::new(d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7], d[8], d[9], d[10], d[11], d[12], d[13], d[14], d[15])
}

/// The world placements of a shape node: its own, plus its mirror copies
/// (mirrored across the planes of the parent frame).
fn placements(scene: &Scene, id: NodeId) -> Vec<(Mat4, bool)> {
    let n = &scene.nodes[&id];
    let parent = scene.parent_matrix(id);
    let local = n.transform.matrix();
    let m = n.mirror;
    let mut out = vec![];
    for sx in [false, true] {
        for sy in [false, true] {
            for sz in [false, true] {
                if (sx && !m.x) || (sy && !m.y) || (sz && !m.z) {
                    continue;
                }
                let r = Mat4::from_scale(Vec3::new(if sx { -1.0 } else { 1.0 }, if sy { -1.0 } else { 1.0 }, if sz { -1.0 } else { 1.0 }));
                let flips = (sx as u8 + sy as u8 + sz as u8) % 2 == 1;
                out.push((parent * r * local, flips));
            }
        }
    }
    out
}

/// Shapes in evaluation order (depth first), visible ones only, with the
/// visibility of their ancestors.
fn shape_order(scene: &Scene) -> Vec<NodeId> {
    fn walk(s: &Scene, id: NodeId, out: &mut Vec<NodeId>) {
        let n = &s.nodes[&id];
        if !n.visible {
            return;
        }
        if n.shape().is_some() {
            out.push(id);
        }
        for c in n.children() {
            walk(s, *c, out);
        }
    }
    let mut out = vec![];
    for c in scene.nodes[&scene.root].children() {
        walk(scene, *c, &mut out);
    }
    out
}

/// Builds the whole scene as one NURBS solid and tessellates it.
pub fn build(scene: &Scene) -> std::result::Result<NurbsModel, String> {
    let t0 = std::time::Instant::now();
    let mut model = NurbsModel::default();
    let mut ignored: Vec<String> = vec![];
    // parts: (solid, material)
    let mut parts: Vec<(Solid, u32)> = vec![];
    for id in shape_order(scene) {
        let n = &scene.nodes[&id];
        let sh = n.shape().unwrap();
        let name = &n.name;
        if sh.round > 0.0 || sh.chamfer > 0.0 || !sh.edges.is_empty() {
            ignored.push(format!("{name}: rounds and chamfers"));
        }
        if n.combine.blend > 0.0 || !n.blend_zones.is_empty() {
            ignored.push(format!("{name}: blend"));
        }
        if sh.shell > 0.0 {
            ignored.push(format!("{name}: shell"));
        }
        if !matches!(n.repeat, forme_core::scene::Repeat::None) {
            ignored.push(format!("{name}: repeat"));
        }
        let base = primitive(sh)?;
        for (m, flips) in placements(scene, id) {
            let mut s = builder::transformed(&base, to_matrix(m));
            if flips {
                s.not();
            }
            match n.combine.mode {
                CombineMode::Add => {
                    model.shapes += 1;
                    // union with every part it touches, else a new part
                    let mut merged = s;
                    let mut keep = vec![];
                    for (p, mat) in parts.drain(..) {
                        if overlaps(&merged, &p) {
                            model.booleans += 1;
                            match truck_shapeops::or(&merged, &p, tol()) {
                                Some(u) => merged = u,
                                None => return Err(format!("{name}: the NURBS union failed")),
                            }
                        } else {
                            keep.push((p, mat));
                        }
                    }
                    keep.push((merged, n.material));
                    parts = keep;
                }
                CombineMode::Carve => {
                    model.shapes += 1;
                    let mut cut = s.clone();
                    cut.not();
                    let mut next = vec![];
                    for (p, mat) in parts.drain(..) {
                        if overlaps(&s, &p) {
                            model.booleans += 1;
                            match truck_shapeops::and(&p, &cut, tol()) {
                                Some(d) => next.push((d, mat)),
                                None => return Err(format!("{name}: the NURBS subtraction failed")),
                            }
                        } else {
                            next.push((p, mat));
                        }
                    }
                    parts = next;
                }
                other => ignored.push(format!("{name}: mode {} not in NURBS yet", other.label())),
            }
        }
    }
    for (p, mat) in &parts {
        append_mesh(&mut model.mesh, p, *mat);
    }
    ignored.dedup();
    model.ignored = ignored;
    model.millis = t0.elapsed().as_millis();
    Ok(model)
}

/// Bounding boxes of two solids overlap (skips booleans between far parts).
fn overlaps(a: &Solid, b: &Solid) -> bool {
    let (alo, ahi) = solid_box(a);
    let (blo, bhi) = solid_box(b);
    alo.x <= bhi.x && blo.x <= ahi.x && alo.y <= bhi.y && blo.y <= ahi.y && alo.z <= bhi.z && blo.z <= ahi.z
}

fn solid_box(s: &Solid) -> (Point3, Point3) {
    // a coarse tessellation bounds the solid well enough for this test
    let mesh = s.triangulation(0.5).to_polygon();
    let mut lo = p3(f64::MAX, f64::MAX, f64::MAX);
    let mut hi = p3(f64::MIN, f64::MIN, f64::MIN);
    for p in mesh.positions() {
        lo = p3(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = p3(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    (p3(lo.x - 0.5, lo.y - 0.5, lo.z - 0.5), p3(hi.x + 0.5, hi.y + 0.5, hi.z + 0.5))
}

fn append_mesh(out: &mut NurbsMesh, s: &Solid, mat: u32) {
    let mesh = s.triangulation(MESH_TOL).to_polygon();
    let pos = mesh.positions();
    let nor = mesh.normals();
    for tri in mesh.faces().triangle_iter() {
        let base = out.positions.len() as u32;
        let a = pos[tri[0].pos];
        let b = pos[tri[1].pos];
        let c = pos[tri[2].pos];
        let face_n = (b - a).cross(c - a);
        for v in tri.iter() {
            let p = pos[v.pos];
            out.positions.push([p.x as f32, p.y as f32, p.z as f32]);
            let n = v.nor.map(|i| nor[i]).unwrap_or(face_n);
            let l = n.magnitude().max(1e-12);
            out.normals.push([(n.x / l) as f32, (n.y / l) as f32, (n.z / l) as f32]);
        }
        out.tris.push([base, base + 1, base + 2]);
        out.tri_material.push(mat);
    }
}

#[allow(dead_code)]
fn _unused() -> f64 {
    PI
}

#[cfg(test)]
mod tests {
    use super::*;
    use forme_core::scene::{NodeSpec, Scene, Shape};

    #[test]
    fn block_with_hole() {
        let mut s = Scene::new();
        let root = s.root;
        let mut b = Shape::new(PrimKind::Box);
        b.size = Vec3::new(20.0, 5.0, 15.0);
        s.add(root, NodeSpec::shape("Bloc", b).at(Vec3::new(0.0, 5.0, 0.0)));
        let mut c = Shape::new(PrimKind::Cylinder);
        c.size = Vec3::new(4.0, 10.0, 0.0);
        s.add(root, NodeSpec::shape("Trou", c).at(Vec3::new(0.0, 5.0, 0.0)).mode(CombineMode::Carve));
        let m = build(&s).expect("build");
        assert_eq!(m.booleans, 1);
        assert!(m.mesh.tris.len() > 50, "{}", m.mesh.tris.len());
        // nothing inside the hole: no vertex closer than 3.9 to the axis
        // between the faces
        for p in &m.mesh.positions {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            assert!(r > 3.9, "vertex in the hole at r = {r}");
        }
    }

    #[test]
    fn every_primitive_builds() {
        for k in [PrimKind::Box, PrimKind::Cylinder, PrimKind::Sphere, PrimKind::Capsule, PrimKind::Torus] {
            let mut sh = Shape::new(k);
            sh.size = match k {
                PrimKind::Torus => Vec3::new(10.0, 3.0, 3.0),
                PrimKind::Sphere => Vec3::splat(8.0),
                _ => Vec3::new(6.0, 8.0, 6.0),
            };
            let s = primitive(&sh).expect("primitive");
            let mesh = s.triangulation(0.1).to_polygon();
            assert!(mesh.faces().len() > 10, "{k:?}");
        }
    }
}
