//! Times the dual contouring of a small hard-surface scene (the SDF tab's preview).
use forme_core::*;
use glam::Vec3;
fn main() {
    let mut s = Scene::new();
    let r = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::new(40.0, 20.0, 30.0);
    b.round = 2.0;
    s.add(r, NodeSpec::shape("a", b));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(12.0, 30.0, 12.0);
    s.add(r, NodeSpec::shape("b", c).at(Vec3::new(10.0, 10.0, 0.0)).mode(CombineMode::Carve).blend(1.5));
    let mut sp = Shape::new(PrimKind::Sphere);
    sp.size = Vec3::splat(16.0);
    s.add(r, NodeSpec::shape("c", sp).at(Vec3::new(-15.0, 12.0, 0.0)).blend(4.0));
    let p = compile(&s);
    for cell in [1.0f32, 0.5, 0.3, 0.2] {
        let t = std::time::Instant::now();
        let m = mesh::dual_contour(&p, Vec3::new(-30.0, -20.0, -25.0), Vec3::new(30.0, 30.0, 25.0), cell);
        println!("cell {cell}: {} tris in {:?}", m.triangle_count(), t.elapsed());
    }
}
