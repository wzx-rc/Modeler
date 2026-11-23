//! Vertices moved as Maya moves them (`Shape::deform`, 23 nov.): every edge
//! and face touching them follows.

use forme_occt::{Shape, SurfKind};

fn translate(x: f64, y: f64, z: f64) -> [[f64; 4]; 3] {
    [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, y], [0.0, 0.0, 1.0, z]]
}

/// The vertices (1-based) where `keep` holds, each moved by `d`.
fn moves(s: &Shape, keep: impl Fn([f64; 3]) -> bool, d: [f64; 3]) -> Vec<(i32, [f64; 3])> {
    s.vertex_points().iter().enumerate().filter(|(_, p)| keep(**p)).map(|(i, p)| (i as i32 + 1, [p[0] + d[0], p[1] + d[1], p[2] + d[2]])).collect()
}

fn kinds(s: &Shape) -> Vec<SurfKind> {
    (0..s.face_count()).filter_map(|i| s.face_info(i).map(|f| f.kind)).collect()
}

fn block() -> Shape {
    let b = Shape::cuboid(40.0, 20.0, 30.0).unwrap();
    let pts = b.vertex_points();
    let lo = pts.iter().fold([f64::MAX; 3], |a, p| [a[0].min(p[0]), a[1].min(p[1]), a[2].min(p[2])]);
    b.transformed(translate(-lo[0] - 20.0, -lo[1], -lo[2] - 15.0)).unwrap()
}

#[test]
fn a_corner_pulled_up_bends_its_faces() {
    let b = block();
    let v0 = b.volume();
    let m = moves(&b, |p| p[0] > 19.9 && p[1] > 19.9 && p[2] > 14.9, [0.0, 6.0, 0.0]);
    assert_eq!(m.len(), 1);
    let r = b.deform(&m).expect("deform");
    assert!(r.is_valid());
    assert_eq!(r.face_count(), 6);
    assert!(r.volume() > v0 + 100.0, "{} {v0}", r.volume());
    // the top bent; the sides round the corner stay flat (the corner
    // moves in their planes)
    assert_eq!(kinds(&r).iter().filter(|k| **k == SurfKind::Plane).count(), 5, "{:?}", kinds(&r));
}

#[test]
fn a_top_with_a_hole_tilted_keeps_its_hole_round() {
    let b = block();
    let h = Shape::cylinder(4.0, 40.0).unwrap().transformed(translate(0.0, 10.0, 0.0)).unwrap();
    let b = b.cut(&h).unwrap();
    let v0 = b.volume();
    let m = moves(&b, |p| p[0] > 19.9 && p[1] > 19.9, [0.0, 8.0, 0.0]);
    assert_eq!(m.len(), 2);
    let r = b.deform(&m).expect("deform");
    assert!(r.is_valid());
    assert!(r.volume() > v0 + 1000.0, "{} {v0}", r.volume());
    // the hole's wall is still a cylinder (its rim slid along its axis)
    assert!(kinds(&r).contains(&SurfKind::Cylinder), "{:?}", kinds(&r));
    // the whole top raised: the hole goes with it
    let m = moves(&b, |p| p[1] > 19.9, [0.0, 5.0, 0.0]);
    let r = b.deform(&m).expect("deform");
    eprintln!("valid {} kinds {:?} faces {}", r.is_valid(), kinds(&r), r.face_count());
    assert!((r.volume() - (v0 + 5.0 * (1200.0 - std::f64::consts::PI * 16.0))).abs() < 1.0, "{} {v0}", r.volume());
}

#[test]
fn a_face_next_to_a_fillet_moves() {
    let b = block();
    // the top front edge rounded (the edge between the top and +z faces)
    let t = b.topo(0.05, 0.3).unwrap();
    let e = t.edges.iter().find(|e| e.pts.iter().all(|p| p[1] > 19.9 && p[2] > 14.9)).expect("top front edge").index;
    let f = b.fillet(&[e], 4.0, 99).unwrap();
    let v0 = f.volume();
    // the back face (z = −15) pushed back 5: the fillet's ends slide on it
    let m = moves(&f, |p| p[2] < -14.9, [0.0, 0.0, -5.0]);
    let r = f.deform(&m).expect("deform");
    assert!(r.is_valid());
    assert!(r.volume() > v0 + 1000.0, "{} {v0}", r.volume());
    // the +x face sheared: its points at the back go out along x
    let m = moves(&f, |p| p[0] > 19.9 && p[2] < -14.9, [6.0, 0.0, 0.0]);
    let r = f.deform(&m).expect("deform");
    assert!(r.is_valid(), "sheared");
}
