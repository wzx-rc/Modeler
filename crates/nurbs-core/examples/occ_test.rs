//! Runs the OpenCascade kernel (`forme-occ`) on small test parts and writes
//! each result as OBJ: `OUT=/tmp/x cargo run --release -p nurbs-core --example occ_test [case…]`.
//! Build the kernel first: `cargo build --release -p nurbs-occ`.

use forme_core::scene::{CombineMode, Mirror, NodeSpec, PrimKind, Scene, Shape};
use glam::{Quat, Vec3};
use std::io::Write;

fn cube_hole(blend: f32, round: f32) -> Scene {
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::splat(10.0);
    b.round = round;
    s.add(root, NodeSpec::shape("Cube", b).at(Vec3::new(0.0, 10.0, 0.0)));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(4.0, 14.0, 0.0);
    s.add(root, NodeSpec::shape("Hole", c).at(Vec3::new(0.0, 10.0, 0.0)).mode(CombineMode::Carve).blend(blend));
    s
}

fn cases() -> Vec<(&'static str, Scene)> {
    let mut v = vec![
        ("cube-hole", cube_hole(0.0, 0.0)),
        ("cube-hole-fillet", cube_hole(1.5, 0.0)),
        ("cube-hole-fillet-round", cube_hole(1.5, 2.0)),
    ];
    // blind hole, its bottom fillet included, plus a boss on the side
    let mut s = cube_hole(1.0, 1.0);
    let root = s.root;
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(3.0, 3.0, 0.0);
    let side = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    s.add(root, NodeSpec::shape("Boss", c).at(Vec3::new(11.0, 10.0, 0.0)).rot(side).blend(1.0));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(1.5, 4.0, 0.0);
    s.add(root, NodeSpec::shape("Pins", c).at(Vec3::new(6.0, 20.0, 6.0)).mode(CombineMode::Carve).blend(0.5).mirror(Mirror { x: true, y: false, z: true }));
    v.push(("cube-boss-pins", s));
    // the shapes truck could not cut: sphere and torus
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::splat(10.0);
    s.add(root, NodeSpec::shape("Cube", b).at(Vec3::new(0.0, 10.0, 0.0)));
    let mut c = Shape::new(PrimKind::Sphere);
    c.size = Vec3::splat(5.0);
    s.add(root, NodeSpec::shape("Sphere", c).at(Vec3::new(10.0, 20.0, 10.0)).mode(CombineMode::Carve).blend(1.0));
    let mut c = Shape::new(PrimKind::Torus);
    c.size = Vec3::new(6.0, 1.5, 1.5);
    s.add(root, NodeSpec::shape("Torus", c).at(Vec3::new(0.0, 20.0, 0.0)).mode(CombineMode::Carve).blend(0.5));
    v.push(("cube-sphere-torus", s));
    v
}

fn write_obj(path: &std::path::Path, m: &nurbs_core::NurbsMesh) {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    for p in &m.positions {
        writeln!(f, "v {} {} {}", p[0], p[1], p[2]).unwrap();
    }
    for n in &m.normals {
        writeln!(f, "vn {} {} {}", n[0], n[1], n[2]).unwrap();
    }
    for t in &m.tris {
        writeln!(f, "f {0}//{0} {1}//{1} {2}//{2}", t[0] + 1, t[1] + 1, t[2] + 1).unwrap();
    }
}

fn main() {
    let out = std::path::PathBuf::from(std::env::var("OUT").unwrap_or_else(|_| "/tmp/occ".into()));
    std::fs::create_dir_all(&out).unwrap();
    let only: Vec<String> = std::env::args().skip(1).collect();
    let Some(exe) = nurbs_core::worker::occ_exe() else {
        eprintln!("forme-occ not found: cargo build --release -p nurbs-occ, or FORME_OCC=path");
        std::process::exit(1);
    };
    for (name, scene) in cases() {
        if !only.is_empty() && !only.iter().any(|o| name.starts_with(o.as_str())) {
            continue;
        }
        let t = std::time::Instant::now();
        match nurbs_core::worker::build_occ(&exe, &scene) {
            Ok(m) => {
                println!(
                    "{name}: {} shapes, {} booleans, {} ms kernel, {} ms total, {} triangles, ignored {:?}",
                    m.shapes,
                    m.booleans,
                    m.millis,
                    t.elapsed().as_millis(),
                    m.mesh.tris.len(),
                    m.ignored
                );
                write_obj(&out.join(format!("{name}.obj")), &m.mesh);
            }
            Err(e) => println!("{name}: ERROR {e}"),
        }
    }
}
