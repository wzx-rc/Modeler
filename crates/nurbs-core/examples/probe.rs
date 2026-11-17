use forme_core::scene::{CombineMode, NodeSpec, PrimKind, Scene, Shape};
use glam::Vec3;
fn case(label: &str, a: (PrimKind, Vec3, Vec3), b: (PrimKind, Vec3, Vec3), mode: CombineMode) {
    let mut s = Scene::new();
    let root = s.root;
    let mut sa = Shape::new(a.0);
    sa.size = a.1;
    s.add(root, NodeSpec::shape("A", sa).at(a.2));
    let mut sb = Shape::new(b.0);
    sb.size = b.1;
    s.add(root, NodeSpec::shape("B", sb).at(b.2).mode(mode));
    let r = nurbs_core::build(&s);
    println!("{label}: {}", match r { Ok(m) => format!("ok {} tris {} ms", m.mesh.tris.len(), m.millis), Err(e) => e });
}
fn main() {
    for k in [PrimKind::Box, PrimKind::Cylinder, PrimKind::Sphere, PrimKind::Capsule, PrimKind::Torus] {
        let mut sh = Shape::new(k);
        sh.size = match k { PrimKind::Torus => Vec3::new(5.0, 2.0, 2.0), PrimKind::Sphere => Vec3::splat(5.0), _ => Vec3::new(3.0, 4.0, 3.0) };
        println!("vol {:?} {:.2}", k, nurbs_core::volume(&nurbs_core::primitive(&sh).unwrap()));
    }
    use PrimKind::*;
    let bx = (Box, Vec3::splat(8.0), Vec3::ZERO);
    for p in [Vec3::new(0.0, 9.1, 0.0), Vec3::new(1.3, 9.1, 0.7), Vec3::new(6.1, 6.3, 5.7), Vec3::new(8.6, 0.4, 0.3), Vec3::new(0.0, 6.0, 0.0)] {
        case(&format!("box-sphere {p:?}"), bx, (Sphere, Vec3::splat(5.0), p), CombineMode::Carve);
        case(&format!("box+sphere {p:?}"), bx, (Sphere, Vec3::splat(5.0), p), CombineMode::Add);
        case(&format!("box-capsule {p:?}"), bx, (Capsule, Vec3::new(5.0, 0.5, 0.0), p), CombineMode::Carve);
        case(&format!("box-torus {p:?}"), bx, (Torus, Vec3::new(5.0, 2.0, 2.0), p), CombineMode::Carve);
    }
}
