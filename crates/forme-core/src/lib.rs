//! Forme core: scene model, SDF bytecode, CPU evaluation, meshing and export.
//! Pure Rust, no platform dependencies: runs the same on iPad and desktop.

pub mod bake;
pub mod demo;
pub mod edges;
pub mod export;
pub mod history;
pub mod mesh;
pub mod program;
pub mod scene;

pub use glam;
pub use history::History;
pub use program::{compile, compile_single, Program};
pub use scene::*;

use glam::Vec3;

fn visible_chain(scene: &Scene, id: NodeId) -> bool {
    scene.node(id).map(|n| n.visible).unwrap_or(false)
        && scene.ancestors(id).iter().all(|a| scene.nodes[a].visible)
}

/// Which shape owns the surface at world point `p` (picking after a raycast).
pub fn pick_node(scene: &Scene, p: Vec3, tolerance: f32) -> Option<NodeId> {
    let mut best: Option<(NodeId, f32)> = None;
    for (id, n) in &scene.nodes {
        if n.shape().is_none() || !visible_chain(scene, *id) {
            continue;
        }
        let prog = compile_single(scene, *id);
        let mut d = prog.dist(p).abs();
        // cutters own the walls they carve: favour them on ties
        if n.combine.mode.is_cutter() {
            d *= 0.8;
        }
        if best.map(|(_, bd)| d < bd).unwrap_or(true) {
            best = Some((*id, d));
        }
    }
    best.filter(|(_, d)| *d <= tolerance).map(|(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};

    fn one(shape: Shape, pos: Vec3) -> Scene {
        let mut s = Scene::new();
        let r = s.root;
        s.add(r, NodeSpec::shape("a", shape).at(pos));
        s
    }

    #[test]
    fn sphere_distance() {
        let mut sh = Shape::new(PrimKind::Sphere);
        sh.size = Vec3::splat(10.0);
        let p = compile(&one(sh, Vec3::ZERO));
        assert!((p.dist(Vec3::ZERO) + 10.0).abs() < 1e-3);
        assert!((p.dist(Vec3::new(20.0, 0.0, 0.0)) - 10.0).abs() < 1e-3);
    }

    #[test]
    fn transform_and_scale() {
        let mut s = Scene::new();
        let r = s.root;
        let id = s.add(r, NodeSpec::shape("b", Shape::new(PrimKind::Box)).at(Vec3::new(50.0, 0.0, 0.0)));
        s.node_mut(id).unwrap().transform.scale = 2.0;
        s.node_mut(id).unwrap().transform.rot = Quat::from_rotation_y(0.7);
        let p = compile(&s);
        assert!(p.dist(Vec3::new(50.0, 0.0, 0.0)) < -10.0);
        // box half-size 10 scaled ×2 → top face at y = 20
        assert!((p.dist(Vec3::new(50.0, 25.0, 0.0)) - 5.0).abs() < 1e-3);
    }

    #[test]
    fn carve_makes_a_hole() {
        let mut s = Scene::new();
        let r = s.root;
        s.add(r, NodeSpec::shape("block", Shape::new(PrimKind::Box)));
        let mut c = Shape::new(PrimKind::Cylinder);
        c.size = Vec3::new(3.0, 20.0, 0.0);
        s.add(r, NodeSpec::shape("hole", c).mode(CombineMode::Carve).mat(3));
        let p = compile(&s);
        assert!(p.dist(Vec3::ZERO) > 0.0, "centre is carved");
        assert!(p.dist(Vec3::new(6.0, 0.0, 0.0)) < 0.0, "rest is solid");
        // the carved wall takes the cutter's material
        let (_, m) = p.eval(Vec3::new(3.0, 0.0, 0.0));
        assert_eq!(m, 3);
    }

    #[test]
    fn radial_and_linear_repeat() {
        let mut s = Scene::new();
        let r = s.root;
        let mut sh = Shape::new(PrimKind::Sphere);
        sh.size = Vec3::splat(2.0);
        s.add(
            r,
            NodeSpec::shape("dot", sh.clone())
                .at(Vec3::new(0.0, 0.0, 10.0))
                .repeat(Repeat::Radial { count: 4, axis: Axis::Y }),
        );
        s.add(
            r,
            NodeSpec::shape("row", sh)
                .at(Vec3::new(0.0, 30.0, 0.0))
                .repeat(Repeat::Linear { count: 3, step: Vec3::new(10.0, 0.0, 0.0) }),
        );
        let p = compile(&s);
        for q in [Vec3::new(0.0, 0.0, 10.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(0.0, 0.0, -10.0), Vec3::new(-10.0, 0.0, 0.0)] {
            assert!(p.dist(q) < -1.9, "radial copy at {q}");
        }
        assert!(p.dist(Vec3::new(7.07, 0.0, 7.07)) > 0.0);
        for x in [0.0, 10.0, 20.0] {
            assert!(p.dist(Vec3::new(x, 30.0, 0.0)) < -1.9);
        }
        assert!(p.dist(Vec3::new(30.0, 30.0, 0.0)) > 0.0, "only 3 copies");
    }

    #[test]
    fn linear_repeat_of_a_group_made_from_the_tree() {
        // group at the origin, content at y = 46: copies at 46 and 70
        let mut s = Scene::new();
        let r = s.root;
        let g = s.add(r, NodeSpec::group("g", false).repeat(Repeat::Linear { count: 2, step: Vec3::new(0.0, 24.0, 0.0) }));
        let mut sh = Shape::new(PrimKind::Sphere);
        sh.size = Vec3::splat(3.0);
        s.add(g, NodeSpec::shape("s", sh).at(Vec3::new(0.0, 46.0, 20.0)));
        let p = compile(&s);
        assert!(p.dist(Vec3::new(0.0, 46.0, 20.0)) < -2.9);
        assert!(p.dist(Vec3::new(0.0, 70.0, 20.0)) < -2.9);
        assert!(p.dist(Vec3::new(0.0, 22.0, 20.0)) > 0.0);
    }

    #[test]
    fn mirror_and_groups() {
        let mut s = Scene::new();
        let r = s.root;
        let g = s.add(r, NodeSpec::group("g", true).mirror(Mirror { x: true, y: false, z: false }));
        let mut sh = Shape::new(PrimKind::Sphere);
        sh.size = Vec3::splat(3.0);
        s.add(g, NodeSpec::shape("s", sh).at(Vec3::new(10.0, 0.0, 0.0)));
        let p = compile(&s);
        assert!(p.dist(Vec3::new(-10.0, 0.0, 0.0)) < -2.9);
    }

    #[test]
    fn groove_cuts_a_line() {
        let mut s = Scene::new();
        let r = s.root;
        let mut b = Shape::new(PrimKind::Box);
        b.size = Vec3::new(20.0, 10.0, 10.0);
        s.add(r, NodeSpec::shape("slab", b));
        let mut plane = Shape::new(PrimKind::Box);
        plane.size = Vec3::new(0.05, 30.0, 30.0);
        s.add(r, NodeSpec::shape("g", plane).mode(CombineMode::Groove).groove(2.0, 1.0));
        let p = compile(&s);
        // on the top face, at the plane: groove 2 mm deep
        assert!(p.dist(Vec3::new(0.0, 9.0, 0.0)) > 0.0);
        assert!(p.dist(Vec3::new(5.0, 9.0, 0.0)) < 0.0);
    }

    #[test]
    fn profile_extrusion() {
        let mut sh = Shape::new(PrimKind::Profile);
        sh.size = Vec3::new(0.0, 0.0, 3.0);
        sh.profile = Some(Profile {
            points: [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)]
                .iter()
                .map(|(x, y)| ProfilePoint { pos: glam::Vec2::new(*x, *y), smooth: false })
                .collect(),
            mode: ProfileMode::Extrude,
        });
        let p = compile(&one(sh, Vec3::ZERO));
        assert!(p.dist(Vec3::new(10.0, 5.0, 0.0)) < -2.9);
        assert!((p.dist(Vec3::new(10.0, 5.0, 5.0)) - 2.0).abs() < 1e-3);
        assert!(p.dist(Vec3::new(25.0, 5.0, 0.0)) > 4.9);
    }

    #[test]
    fn grenade_compiles_and_is_hit_from_the_front() {
        let s = demo::grenade();
        let p = compile(&s);
        assert!(p.prim_count > 20, "prims: {}", p.prim_count);
        let (lo, hi) = s.bounds();
        assert!(lo.y < 10.0 && hi.y > 140.0, "bounds {lo} {hi}");
        assert!(lo.y > -2.0, "nothing below the ground: {lo}");
        let t = p.raycast(Vec3::new(0.0, 46.0, 300.0), -Vec3::Z, 600.0).expect("hit");
        // front boss surface ~ z = 33
        let z = 300.0 - t;
        assert!(z > 28.0 && z < 36.0, "front hit at z={z}");
        // picking at the screw finds a screw-related node
        let hit = Vec3::new(0.0, 46.0, z);
        let id = pick_node(&s, hit, 2.0).expect("picked");
        let name = &s.node(id).unwrap().name;
        assert!(["Vis", "Fente", "Logement vis", "Bossage"].contains(&name.as_str()), "picked {name}");
    }

    #[test]
    fn json_round_trip_and_history() {
        let mut s = demo::grenade();
        let json = s.to_json();
        let back = Scene::from_json(&json).unwrap();
        assert_eq!(back, s);
        let mut h = History::new();
        h.record("suppr", &s);
        let first = s.rows()[0].0;
        s.remove(first);
        assert_ne!(s, back);
        h.undo(&mut s);
        assert_eq!(s, back);
        h.redo(&mut s);
        assert_ne!(s, back);
    }

    #[test]
    fn duplicate_and_reparent() {
        let mut s = demo::grenade();
        let n0 = s.nodes.len();
        let corps = s.rows()[0].0;
        let copy = s.duplicate(corps).unwrap();
        assert!(s.nodes.len() > n0 + 5);
        let head = s.rows().iter().find(|(id, _)| s.node(*id).unwrap().name == "Tête").unwrap().0;
        s.reparent(copy, head, 0);
        assert_eq!(s.parent_of(copy), Some(head));
        s.reparent(head, copy, 0); // would create a cycle: refused
        assert_eq!(s.parent_of(head), Some(s.root));
    }

    #[test]
    fn mesh_sphere_is_closed_and_accurate() {
        let mut sh = Shape::new(PrimKind::Sphere);
        sh.size = Vec3::splat(10.0);
        let p = compile(&one(sh, Vec3::ZERO));
        let m = mesh::dual_contour(&p, Vec3::splat(-10.0), Vec3::splat(10.0), 1.0);
        assert!(m.triangle_count() > 500);
        for v in &m.positions {
            assert!((v.length() - 10.0).abs() < 0.15, "vertex off surface: {}", v.length());
        }
        // closed: every edge used exactly twice
        let mut edges = std::collections::HashMap::new();
        for t in m.indices.chunks(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        assert!(edges.values().all(|c| *c == 2), "mesh not closed");
        // outward winding
        let t = &m.indices[0..3];
        let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
        assert!((b - a).cross(c - a).dot(a) > 0.0, "inward winding");
    }

    #[test]
    fn mesh_box_keeps_sharp_corners() {
        let p = compile(&one(Shape::new(PrimKind::Box), Vec3::new(0.3, 0.2, 0.1)));
        let m = mesh::dual_contour(&p, Vec3::splat(-11.0), Vec3::splat(11.0), 1.3);
        let corner = Vec3::new(10.3, 10.2, 10.1);
        let best = m.positions.iter().map(|v| v.distance(corner)).fold(f32::MAX, f32::min);
        assert!(best < 0.05, "corner rounded off by {best}");
    }

    #[test]
    fn glb_and_obj_export() {
        let s = demo::starter();
        let p = compile(&s);
        let (lo, hi) = s.bounds();
        let m = mesh::dual_contour(&p, lo, hi, 2.0);
        let glb = export::to_glb(&m, &s.materials, "bloc");
        assert_eq!(&glb[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize, glb.len());
        let obj = export::to_obj(&m, &s.materials, "bloc");
        assert!(obj.lines().filter(|l| l.starts_with("f ")).count() == m.triangle_count());
    }

    #[test]
    fn overlapping_radial_copies_have_no_seam() {
        // spheres larger than their sector: the neighbour copy must be seen
        let mut s = Scene::new();
        let r = s.root;
        let mut sp = Shape::new(PrimKind::Sphere);
        sp.size = Vec3::splat(9.0);
        s.add(r, NodeSpec::shape("s", sp).at(Vec3::new(0.0, 0.0, 10.0)).repeat(Repeat::Radial { count: 4, axis: Axis::Y }));
        let p = compile(&s);
        let a = std::f32::consts::FRAC_PI_4;
        for rad in [6.0f32, 8.0, 10.0] {
            let d1 = p.dist(Vec3::new(rad * (a + 1e-3).sin(), 0.0, rad * (a + 1e-3).cos()));
            let d2 = p.dist(Vec3::new(rad * (a - 1e-3).sin(), 0.0, rad * (a - 1e-3).cos()));
            assert!((d1 - d2).abs() < 0.05, "seam at r={rad}: {d1} vs {d2}");
        }
    }

    fn bake_ok(s: &Scene) -> bake::Baked {
        let b = bake::bake_blocking(s, bake::Quality::Standard);
        assert!(!b.faces.is_empty());
        assert_eq!(b.stats.open_edges + b.stats.nonmanifold_edges, 0, "seams left open");
        assert!(b.stats.mean_dev < 0.02, "mean deviation {}", b.stats.mean_dev);
        b
    }

    #[test]
    fn bake_box_with_blended_hole_is_closed_and_mostly_quads() {
        let mut s = Scene::new();
        let r = s.root;
        let mut b = Shape::new(PrimKind::Box);
        b.round = 1.0;
        s.add(r, NodeSpec::shape("b", b));
        let mut c = Shape::new(PrimKind::Cylinder);
        c.size = Vec3::new(4.0, 14.0, 0.0);
        s.add(r, NodeSpec::shape("c", c).mode(CombineMode::Carve).blend(1.0));
        let out = bake_ok(&s);
        assert!(out.stats.quads * 2 > out.faces.len());
        // OBJ keeps the polygons, GLB is triangulated
        let obj = out.to_obj(&s.materials, "t");
        assert_eq!(obj.lines().filter(|l| l.starts_with("f ")).count(), out.faces.len());
        let mesh = out.to_mesh();
        assert_eq!(mesh.triangle_count(), out.triangle_count());
    }

    #[test]
    fn flat_loops_pass_is_optional_and_keeps_the_mesh_closed() {
        let mut s = Scene::new();
        let r = s.root;
        let mut b = Shape::new(PrimKind::Box);
        b.round = 1.0;
        s.add(r, NodeSpec::shape("b", b));
        let mut c = Shape::new(PrimKind::Cylinder);
        c.size = Vec3::new(4.0, 14.0, 0.0);
        s.add(r, NodeSpec::shape("c", c).mode(CombineMode::Carve).blend(1.0));
        let out = bake_ok(&s);
        // off by default: the bake itself never runs it
        assert_eq!(out.stats.flat_loops, 0);
        let d = bake::decimate::dissolve_flat_loops(&s, &out, bake::decimate::FLAT_LOOPS_DEG, 2);
        assert!(d.stats.flat_loops > 0 && d.faces.len() < out.faces.len());
        assert_eq!(d.stats.faces_before_flat, out.faces.len());
        assert_eq!(d.stats.open_edges + d.stats.nonmanifold_edges, 0, "pass opened the mesh");
        assert_eq!(d.stats.quads, d.faces.len());
        assert!(d.stats.max_dev <= out.stats.max_dev + 1e-6);
    }

    #[test]
    fn bake_threads_on_a_core_and_a_groove() {
        let mut s = Scene::new();
        let r = s.root;
        let mut c = Shape::new(PrimKind::Cylinder);
        c.size = Vec3::new(14.0, 9.0, 0.0);
        s.add(r, NodeSpec::shape("core", c).at(Vec3::new(0.0, 100.0, 0.0)));
        let mut t = Shape::new(PrimKind::Torus);
        t.size = Vec3::new(14.4, 0.0, 1.5);
        s.add(r, NodeSpec::shape("thread", t).at(Vec3::new(0.0, 93.5, 0.0)).repeat(Repeat::Linear { count: 5, step: Vec3::new(0.0, 3.3, 0.0) }));
        bake_ok(&s);
        let mut s = Scene::new();
        let r = s.root;
        let mut b = Shape::new(PrimKind::Box);
        b.size = Vec3::new(10.0, 6.0, 10.0);
        s.add(r, NodeSpec::shape("b", b));
        let mut g = Shape::new(PrimKind::Box);
        g.size = Vec3::new(0.1, 10.0, 14.0);
        s.add(r, NodeSpec::shape("g", g).at(Vec3::new(0.0, 6.0, 0.0)).mode(CombineMode::Groove).groove(1.2, 0.9));
        bake_ok(&s);
    }
}
