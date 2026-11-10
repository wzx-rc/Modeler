//! Bake diagnostics: base meshes per primitive, then full bakes.
use forme_core::bake::{self, field::Field, gen, Quality};
use forme_core::glam::{Quat, Vec2, Vec3};
use forme_core::*;

fn single(shape: Shape, combine: Combine) -> Scene {
    let mut s = Scene::new();
    let root = s.root;
    let mut spec = NodeSpec::shape("p", shape);
    spec.combine = combine;
    s.add(root, spec);
    s
}

fn defects(b: &bake::Baked) {
    let mut und: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
    for f in &b.faces {
        for i in 0..f.verts.len() {
            let (a, c) = (f.verts[i], f.verts[(i + 1) % f.verts.len()]);
            *und.entry((a.min(c), a.max(c))).or_default() += 1;
        }
    }
    let mut clusters: std::collections::BTreeMap<(i64, i64, i64), (u32, u32)> = std::collections::BTreeMap::new();
    for (&(a, _), &c) in &und {
        if c != 2 {
            let p = b.positions[a as usize];
            let k = ((p.x / 3.0).round() as i64 * 3, (p.y / 3.0).round() as i64 * 3, (p.z / 3.0).round() as i64 * 3);
            let e = clusters.entry(k).or_default();
            if c == 1 { e.0 += 1 } else { e.1 += 1 }
        }
    }
    for (k, (o, n)) in clusters.iter().take(30) {
        println!("   defects near {:?}: open {o} nonmanifold {n}", k);
    }
}

/// Test piece like the reference: a pot with a lip and an inset panel.
fn pot() -> Scene {
    let mut s = Scene::new();
    let root = s.root;
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(12.0, 12.0, 0.0);
    c.round = 1.0;
    s.add(root, NodeSpec::shape("pot", c));
    let mut lip = Shape::new(PrimKind::Cylinder);
    lip.size = Vec3::new(10.6, 2.0, 0.0);
    s.add(root, NodeSpec::shape("lip", lip).at(Vec3::new(0.0, 12.0, 0.0)).mode(CombineMode::Carve).blend(0.6));
    let g = s.add(root, NodeSpec::group("panel", true).mode(CombineMode::Carve).blend(0.5));
    let mut tube = Shape::new(PrimKind::Tube);
    tube.size = Vec3::new(13.0, 20.0, 11.3);
    s.add(g, NodeSpec::shape("shell", tube));
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::new(5.5, 6.0, 8.0);
    b.round = 2.0;
    s.add(g, NodeSpec::shape("window", b).at(Vec3::new(0.0, -1.0, 10.0)).mode(CombineMode::Intersect));
    s
}

fn report(name: &str, scene: &Scene, q: Quality) {
    let b = bake::bake_blocking(scene, q);
    if std::env::var("DEFECTS").is_ok() {
        defects(&b);
    }
    let s = &b.stats;
    println!(
        "{name:<22} {:>7} faces ({:>6} q {:>6} t {:>5} n) open {:>4} nonman {:>3} holes {:>3} dev max {:.4} mean {:.5}  {:.2}s [{:.2} {:.2} {:.2} {:.2} {:.2}]",
        b.faces.len(),
        s.quads,
        s.tris,
        s.ngons,
        s.open_edges,
        s.nonmanifold_edges,
        s.holes_filled,
        s.max_dev,
        s.mean_dev,
        s.seconds,
        s.stage_seconds[0],
        s.stage_seconds[1],
        s.stage_seconds[2],
        s.stage_seconds[3],
        s.stage_seconds[4]
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let what = args.get(1).map(|s| s.as_str()).unwrap_or("all");
    let gs = gen::GenSettings { tol: 0.01, max_edge: 4.0, segs: 0 };
    if what == "all" || what == "gen" {
        let mut kinds: Vec<(&str, Shape)> = vec![];
        let mut b = Shape::new(PrimKind::Box);
        b.round = 2.0;
        kinds.push(("box round", b.clone()));
        b.round = 0.0;
        kinds.push(("box sharp", b.clone()));
        b.chamfer = 1.5;
        kinds.push(("box chamfer", b));
        let mut c = Shape::new(PrimKind::Cylinder);
        c.round = 1.5;
        kinds.push(("cylinder", c.clone()));
        c.taper = 0.3;
        kinds.push(("cylinder taper", c));
        kinds.push(("sphere", Shape::new(PrimKind::Sphere)));
        let mut e = Shape::new(PrimKind::Sphere);
        e.size = Vec3::new(14.0, 15.5, 12.0);
        kinds.push(("ellipsoid", e));
        kinds.push(("capsule", Shape::new(PrimKind::Capsule)));
        kinds.push(("torus", Shape::new(PrimKind::Torus)));
        let mut co = Shape::new(PrimKind::Cone);
        co.round = 1.0;
        kinds.push(("cone", co));
        let mut pr = Shape::new(PrimKind::Prism);
        pr.round = 1.0;
        kinds.push(("prism", pr));
        let mut tu = Shape::new(PrimKind::Tube);
        tu.round = 0.8;
        kinds.push(("tube", tu));
        let mut pf = Shape::new(PrimKind::Profile);
        pf.profile = Some(demo::lever_profile());
        pf.round = 1.2;
        pf.size.z = 4.5;
        kinds.push(("profile lever", pf.clone()));
        let mut rv = pf.clone();
        rv.profile = Some(Profile {
            points: vec![
                ProfilePoint { pos: Vec2::new(0.0, -5.0), smooth: false },
                ProfilePoint { pos: Vec2::new(8.0, -5.0), smooth: false },
                ProfilePoint { pos: Vec2::new(6.0, 5.0), smooth: true },
                ProfilePoint { pos: Vec2::new(0.0, 7.0), smooth: false },
            ],
            mode: ProfileMode::Revolve,
        });
        rv.round = 0.5;
        kinds.push(("profile revolve", rv));
        for (name, sh) in kinds {
            let scene = single(sh, Combine::default());
            let f = Field::new(&compile(&scene));
            let pr = &f.prims[0];
            let t = std::time::Instant::now();
            let m = gen::generate(pr, 0.0, &gs);
            let el = t.elapsed().as_secs_f64();
            let mut dev = 0.0f64;
            for p in &m.pos {
                dev = dev.max(bake::field::prim_raw(pr, *p).abs());
            }
            let mut fdev = 0.0f64;
            let mut flipped = 0;
            for face in &m.faces {
                let c = face.iter().fold(glam::DVec3::ZERO, |s, v| s + m.pos[*v as usize]) / face.len() as f64;
                fdev = fdev.max(bake::field::prim_raw(pr, c).abs());
                let n = m.normal_of(face);
                if n.dot(bake::field::prim_grad(pr, c)) < 0.0 {
                    flipped += 1;
                    if std::env::var("DBG").is_ok() {
                        let pts: Vec<_> = face.iter().map(|v| m.pos[*v as usize]).collect();
                        println!("  flipped {:?} n {:?} g {:?}", pts, n, bake::field::prim_grad(pr, c));
                    }
                }
            }
            let quads = m.faces.iter().filter(|f| f.len() == 4).count();
            println!(
                "{name:<16} {:>6} v {:>6} f ({quads} quads) open {:>3} flipped {flipped} vdev {:.1e} fdev {:.4}  {:.3}s",
                m.pos.len(),
                m.faces.len(),
                m.open_edges(),
                dev,
                fdev,
                el
            );
        }
    }
    if what == "dump" {
        let mut s = Scene::new();
        let root = s.root;
        let mut b = Shape::new(PrimKind::Box);
        b.size = Vec3::new(10.0, 6.0, 10.0);
        s.add(root, NodeSpec::shape("b", b));
        let mut g = Shape::new(PrimKind::Box);
        g.size = Vec3::new(0.1, 10.0, 14.0);
        s.add(root, NodeSpec::shape("g", g).at(Vec3::new(0.0, 6.0, 0.0)).mode(CombineMode::Groove).groove(1.2, 0.9));
        dump_near(&s, 0, 0.0, glam::DVec3::new(0.38, -4.855, 10.0), 0.05);
    }
    if what == "neck" {
        let mut s = Scene::new();
        let root = s.root;
        let mut c = Shape::new(PrimKind::Cylinder);
        c.size = Vec3::new(14.0, 9.0, 0.0);
        s.add(root, NodeSpec::shape("core", c).at(Vec3::new(0.0, 100.0, 0.0)));
        let mut t = Shape::new(PrimKind::Torus);
        t.size = Vec3::new(14.4, 0.0, 1.5);
        s.add(root, NodeSpec::shape("thread", t).at(Vec3::new(0.0, 93.5, 0.0)).repeat(Repeat::Linear { count: 5, step: Vec3::new(0.0, 3.3, 0.0) }));
        if std::env::var("FARBLEND").is_ok() {
            let sp = Shape::new(PrimKind::Sphere);
            s.add(root, NodeSpec::shape("far", sp).at(Vec3::new(0.0, 0.0, 0.0)).blend(5.0));
        }
        report("neck", &s, Quality::Standard);
        check_cuts(&s, Quality::Standard);
        let b = bake::bake_blocking(&s, Quality::Standard);
        render(&b, "/tmp/neck.png", glam::DVec3::new(0.6, 0.3, 1.0), glam::DVec3::new(0.0, 100.0, 0.0), 40.0, 900, 800);
    }
    if what == "sub" {
        // the grenade with only the named shapes visible
        let keep: Vec<String> = args[2..].to_vec();
        let mut s = demo::grenade();
        let ids: Vec<NodeId> = s.nodes.keys().copied().collect();
        for id in ids {
            let n = s.nodes.get_mut(&id).unwrap();
            if n.shape().is_some() && !keep.iter().any(|k| n.name == *k) {
                n.visible = false;
            }
        }
        report("subset", &s, Quality::Standard);
        if let Ok(v) = std::env::var("NEAR") {
            let w: Vec<f64> = v.split(',').map(|x| x.parse().unwrap()).collect();
            let at = glam::DVec3::new(w[0], w[1], w[2]);
            let b = bake::bake_blocking(&s, Quality::Standard);
            let mut und: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
            for f in &b.faces {
                for i in 0..f.verts.len() {
                    let (a, c) = (f.verts[i], f.verts[(i + 1) % f.verts.len()]);
                    *und.entry((a.min(c), a.max(c))).or_default() += 1;
                }
            }
            for (fi, f) in b.faces.iter().enumerate() {
                let bad = (0..f.verts.len()).any(|i| {
                    let (a, c) = (f.verts[i], f.verts[(i + 1) % f.verts.len()]);
                    und[&(a.min(c), a.max(c))] != 2
                });
                if bad && f.verts.iter().any(|v| b.positions[*v as usize].distance(at) < w[3]) {
                    println!("face {fi} patch {} :", f.patch);
                    for i in 0..f.verts.len() {
                        let (a, c) = (f.verts[i], f.verts[(i + 1) % f.verts.len()]);
                        println!("   v{a} {:?} edge->v{c} uses {}", b.positions[a as usize], und[&(a.min(c), a.max(c))]);
                    }
                }
            }
        }
        if std::env::var("CUTS").is_ok() {
            check_cuts(&s, Quality::Standard);
        }
        let b = bake::bake_blocking(&s, Quality::Standard);
        let (lo, hi) = s.bounds();
        let c = (lo + hi) * 0.5;
        let (cx, ext) = match std::env::var("VIEW").ok().map(|v| v.split(',').map(|x| x.parse::<f64>().unwrap()).collect::<Vec<_>>()) {
            Some(v) => (glam::DVec3::new(v[0], v[1], v[2]), v[3]),
            None => (glam::DVec3::new(c.x as f64, c.y as f64, c.z as f64), (hi - lo).max_element() as f64 * 1.2),
        };
        let dir = std::env::var("DIR").ok().map(|v| { let w: Vec<f64> = v.split(',').map(|x| x.parse().unwrap()).collect(); glam::DVec3::new(w[0], w[1], w[2]) }).unwrap_or(glam::DVec3::new(0.6, 0.35, 1.0));
        render(&b, "/tmp/sub.png", dir, cx, ext, 1000, 900);
    }
    if what == "quad" {
        let s = if std::env::var("POT").is_ok() { pot() } else { cube_hole() };
        let size: f64 = std::env::var("SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.8);
        let qs = bake::quad::QuadSettings { size, crease_deg: 35.0, threads: 2 };
        let b = bake::quad::quad_bake(&s, &qs, &bake::Progress::default()).unwrap();
        let st = &b.stats;
        println!(
            "pot quad  {} faces ({} q {} t {} n) open {} nonman {} holes {} dev max {:.4} mean {:.5} {:.2}s {:?}",
            b.faces.len(), st.quads, st.tris, st.ngons, st.open_edges, st.nonmanifold_edges, st.holes_filled, st.max_dev, st.mean_dev, st.seconds, st.stage_seconds
        );
        if std::env::var("POT").is_ok() {
            render(&b, "/tmp/pot.png", glam::DVec3::new(0.5, 0.45, 1.0), glam::DVec3::new(0.0, 0.0, 0.0), 34.0, 1000, 1000);
            render(&b, "/tmp/pot_zoom.png", glam::DVec3::new(0.25, 0.15, 1.0), glam::DVec3::new(3.0, 2.0, 12.0), 14.0, 1000, 1000);
        } else {
            render(&b, "/tmp/pot.png", glam::DVec3::new(0.75, 0.9, 1.0), glam::DVec3::new(0.0, 1.0, 0.0), 34.0, 1100, 1100);
            render(&b, "/tmp/pot_zoom.png", glam::DVec3::new(0.35, 1.0, 0.55), glam::DVec3::new(1.5, 8.0, 2.0), 15.0, 1100, 1100);
        }

    }
    if what == "build" {
        let s = cube_hole();
        let size: f64 = std::env::var("SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(2.25);
        let bs = bake::build::BuildSettings { size, tol: 0.03, fillet_rows: 0, threads: 2 };
        let b = bake::build::build(&s, &bs, &bake::Progress::default()).expect("build unsupported");
        let st = &b.stats;
        println!(
            "build {} faces ({} q {} t {} n) verts {} open {} nonman {} dev max {:.5} mean {:.6} {:.3}s",
            b.faces.len(), st.quads, st.tris, st.ngons, st.verts, st.open_edges, st.nonmanifold_edges, st.max_dev, st.mean_dev, st.seconds
        );
        render(&b, "/tmp/build_a.png", glam::DVec3::new(0.75, 0.9, 1.0), glam::DVec3::new(0.0, 1.0, 0.0), 34.0, 1100, 1100);
        render(&b, "/tmp/build_b.png", glam::DVec3::new(0.35, 1.0, 0.55), glam::DVec3::new(1.5, 8.0, 2.0), 15.0, 1100, 1100);
    }
    if what == "support" {
        // the PC test part, junction by junction: why the bake by operation
        // refuses it, and close-ups of what the bake gives at each junction
        use glam::DVec3;
        let out = std::env::var("OUT").unwrap_or_else(|_| "/tmp".into());
        let stages: Vec<(&str, Scene)> = vec![
            ("1-socle-palier", support(2)),
            ("2-alesage", support(3)),
            ("3-trous", support(4)),
            ("0-bossage-debout", support_boss()),
        ];
        for (name, s) in &stages {
            let mut bs = bake::BakeSettings::preset(Quality::Standard);
            bs.threads = 2;
            match bake::build::build_auto(s, &bs, &bake::Progress::default()) {
                Ok(b) => {
                    let st = &b.stats;
                    println!("{name:18} par opération : {} faces ({} q {} t {} n) open {} nonman {} dev max {:.4}", b.faces.len(), st.quads, st.tris, st.ngons, st.open_edges, st.nonmanifold_edges, st.max_dev);
                }
                Err(e) => println!("{name:18} repli : {e}"),
            }
            let b = bake::bake_blocking(s, Quality::Standard);
            let st = &b.stats;
            println!("{name:18} bake : {} faces ({} q {} t {} n) open {} nonman {} constructif {}", b.faces.len(), st.quads, st.tris, st.ngons, st.open_edges, st.nonmanifold_edges, st.constructive);
            render(&b, &format!("{out}/{name}_vue.png"), DVec3::new(0.75, 0.6, 1.0), DVec3::new(0.0, 12.0, 0.0), 66.0, 1200, 1000);
            if name.starts_with('0') {
                render(&b, &format!("{out}/{name}_zoom.png"), DVec3::new(0.7, 0.45, 1.0), DVec3::new(6.0, 11.0, 4.0), 16.0, 1100, 1100);
            } else {
                // along the long junction line, then where the end cap meets the top
                render(&b, &format!("{out}/{name}_zoom.png"), DVec3::new(1.0, 0.55, 0.35), DVec3::new(7.0, 11.0, 4.0), 18.0, 1100, 1100);
                render(&b, &format!("{out}/{name}_coin.png"), DVec3::new(0.55, 0.5, 1.0), DVec3::new(6.0, 11.0, 13.0), 12.0, 1100, 1100);
            }
        }
    }
    if what == "lignes" {
        // the bearing / base junction of the test part as lines, then its
        // fillet alone as a strip of quads
        use glam::DVec3;
        let out = std::env::var("OUT").unwrap_or_else(|_| "/tmp".into());
        let s = support(4);
        let bs = bake::build::BuildSettings { size: 1.5, tol: 0.02, fillet_rows: 0, threads: 2 };
        let j = bake::build::junction(&support(2), 1, &bs).expect("junction");
        let f = &j.fillet;
        let st = &f.stats;
        let len = |c: &Vec<DVec3>| (0..c.len()).map(|i| c[i].distance(c[(i + 1) % c.len()])).sum::<f64>();
        println!("coin {:?} mm", j.corner.iter().map(len).collect::<Vec<_>>());
        println!("rail socle {:?} mm", j.rail_a.iter().map(len).collect::<Vec<_>>());
        println!("rail palier {:?} mm", j.rail_b.iter().map(len).collect::<Vec<_>>());
        println!("fillet {} faces ({} q) open {} dev max {:.3} mean {:.4}", f.faces.len(), st.quads, st.open_edges, st.max_dev, st.mean_dev);
        let part = bake::bake_blocking(&s, Quality::Max);
        // the corner sits under the fillet: drawn dashed through the surface
        let lines: Vec<(&Vec<DVec3>, [u8; 3], bool, bool)> = j
            .corner
            .iter()
            .map(|c| (c, [255, 214, 10], true, true))
            .chain(j.rail_a.iter().map(|c| (c, [64, 156, 255], false, true)))
            .chain(j.rail_b.iter().map(|c| (c, [255, 120, 40], false, true)))
            .collect();
        let views = [
            ("a-vue", DVec3::new(0.75, 0.6, 1.0), DVec3::new(0.0, 12.0, 0.0), 62.0),
            ("b-cote", DVec3::new(1.0, 0.45, 0.25), DVec3::new(8.0, 11.0, 0.0), 30.0),
            ("c-bout", DVec3::new(0.6, 0.5, 1.0), DVec3::new(7.0, 11.0, 14.0), 13.0),
        ];
        for (name, dir, target, ext) in views {
            render_lines(Some(&part), None, &lines, &format!("{out}/lignes-{name}.png"), dir, target, ext, 1400, 1050);
            render_lines(Some(&part), Some(f), &lines, &format!("{out}/fillet-{name}.png"), dir, target, ext, 1400, 1050);
        }
        render_lines(None, Some(f), &lines, &format!("{out}/fillet-seul.png"), DVec3::new(0.75, 0.6, 1.0), DVec3::new(0.0, 11.0, 0.0), 36.0, 1400, 1050);
        render_lines(None, Some(f), &lines, &format!("{out}/fillet-seul-coin.png"), DVec3::new(0.35, 1.0, 0.45), DVec3::new(7.0, 11.0, 14.0), 9.0, 1100, 1100);
    }
    if what == "anneau" {
        // front ring of the bearing: its inner edge (junction with the bore,
        // 0.6 mm fillet) and its outer edge (the cylinder's own 1 mm round)
        use bake::field::Field;
        use glam::DVec3;
        let out = std::env::var("OUT").unwrap_or_else(|_| "/tmp".into());
        let s = support(4);
        let full = Field::new(&compile(&s));
        let on_part = |p: DVec3| full.dist(p).abs() < 0.02;
        let front = |p: &DVec3| p.z > 0.0;
        // inner edge
        let bs = bake::build::BuildSettings { size: 1.0, tol: 0.01, fillet_rows: 0, threads: 2 };
        let j = bake::build::junction(&support(3), 2, &bs).expect("junction");
        let keep = |l: &Vec<Vec<DVec3>>| -> Vec<Vec<DVec3>> { l.iter().filter(|c| c.iter().all(front)).cloned().collect() };
        let (corner, rail_a, rail_b) = (keep(&j.corner), keep(&j.rail_a), keep(&j.rail_b));
        let mut inner = j.fillet.clone();
        inner.faces.retain(|f| f.verts.iter().all(|v| front(&inner.positions[*v as usize])));
        // outer edge: the native rows of the round, where they are on the part
        let steps = full.steps().unwrap();
        let pi = steps[1].prim;
        let pr = &full.prims[pi as usize];
        let inst = full.instances(pi)[0];
        let (r, hh, rd) = (pr.size.x, pr.size.y, pr.round);
        // rows of the round like the inner strip: columns at the same target
        // length, rows across the quarter circle, kept where they are on the part
        let n = ((std::f64::consts::TAU * (r - rd * 0.5) / bs.size).round() as usize).max(8);
        let rows = ((rd * std::f64::consts::FRAC_PI_2 / bs.size).round() as usize).max(2);
        let mut outer = bake::Baked::default();
        for i in 0..n {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            for t in 0..=rows {
                let phi = t as f64 / rows as f64 * std::f64::consts::FRAC_PI_2;
                let rad = r - rd + rd * phi.sin();
                let y = hh - rd + rd * phi.cos();
                outer.positions.push(inst.to_world.transform_point3(DVec3::new(rad * a.cos(), y, rad * a.sin())));
            }
        }
        let id = |i: usize, t: usize| ((i % n) * (rows + 1) + t) as u32;
        for i in 0..n {
            for t in 0..rows {
                let verts = vec![id(i, t), id(i + 1, t), id(i + 1, t + 1), id(i, t + 1)];
                let w: Vec<DVec3> = verts.iter().map(|v| outer.positions[*v as usize]).collect();
                if !w.iter().all(|p| on_part(*p)) {
                    continue;
                }
                let normals = w.iter().map(|p| full.grad(*p).normalize_or_zero()).collect();
                outer.faces.push(bake::BFace { verts, normals, mat: 0, patch: 2 });
            }
        }
        // tangent lines of the round, cut where the part swallows them
        let circle = |rad: f64, y: f64| -> Vec<Vec<DVec3>> {
            let mut segs: Vec<Vec<DVec3>> = vec![vec![]];
            for i in 0..=512 {
                let a = i as f64 / 512.0 * std::f64::consts::TAU;
                let p = inst.to_world.transform_point3(DVec3::new(rad * a.cos(), y, rad * a.sin()));
                if on_part(p) {
                    segs.last_mut().unwrap().push(p);
                } else if !segs.last().unwrap().is_empty() {
                    segs.push(vec![]);
                }
            }
            segs.into_iter().filter(|s| s.len() > 1).collect()
        };
        let (cap_line, side_line) = (circle(r - rd, hh), circle(r, hh - rd));
        println!("bord intérieur : coin {} boucle(s), rails {} / {}, bande {} quads", corner.len(), rail_a.len(), rail_b.len(), inner.faces.len());
        println!("bord extérieur : arrondi {} quads, lignes {} + {} morceaux", outer.faces.len(), cap_line.len(), side_line.len());
        let mut strip = inner.clone();
        let base = strip.positions.len() as u32;
        strip.positions.extend(outer.positions.iter().copied());
        for f in &outer.faces {
            strip.faces.push(bake::BFace { verts: f.verts.iter().map(|v| v + base).collect(), normals: f.normals.clone(), mat: 0, patch: 2 });
        }
        let lines: Vec<(&Vec<DVec3>, [u8; 3], bool, bool)> = corner
            .iter()
            .map(|c| (c, [255, 214, 10], true, true))
            .chain(rail_a.iter().map(|c| (c, [64, 156, 255], false, true)))
            .chain(rail_b.iter().map(|c| (c, [255, 120, 40], false, true)))
            .chain(cap_line.iter().map(|c| (c, [64, 156, 255], false, false)))
            .chain(side_line.iter().map(|c| (c, [255, 120, 40], false, false)))
            .collect();
        let part = bake::bake_blocking(&s, Quality::Max);
        let views = [("a-vue", DVec3::new(0.45, 0.35, 1.0), DVec3::new(0.0, 18.0, 14.0), 30.0), ("b-zoom", DVec3::new(0.55, 0.5, 1.0), DVec3::new(5.0, 25.0, 14.0), 11.0)];
        for (name, dir, target, ext) in views {
            render_lines(Some(&part), None, &lines, &format!("{out}/anneau-lignes-{name}.png"), dir, target, ext, 1400, 1050);
            render_lines(Some(&part), Some(&strip), &lines, &format!("{out}/anneau-polys-{name}.png"), dir, target, ext, 1400, 1050);
        }
        render_lines(None, Some(&strip), &lines, &format!("{out}/anneau-polys-seul.png"), DVec3::new(0.45, 0.35, 1.0), DVec3::new(0.0, 18.0, 14.0), 30.0, 1400, 1050);
    }
    if what == "raccord" {
        raccord();
    }
    if what == "liaison" {
        liaison();
    }
    if what == "piece" {
        piece();
    }
    if what == "construction" {
        construction();
        return;
    }
    if what == "decime" {
        decime();
    }
    if what == "screws" {
        let s = screws_scene();
        let size: f64 = std::env::var("SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let mut bs = bake::BakeSettings::preset(bake::Quality::Standard);
        bs.threads = 2;
        if size > 0.0 {
            bs.divisions = 20.0 / size;
        }
        let t = std::time::Instant::now();
        match bake::build::build_auto(&s, &bs, &bake::Progress::default()).map_err(|e| println!("repli : {e}")).ok() {
            Some(b) => {
                let st = &b.stats;
                println!("screws {} faces ({} q {} t {} n) open {} nonman {} dev max {:.4} mean {:.5} {:.2}s", b.faces.len(), st.quads, st.tris, st.ngons, st.open_edges, st.nonmanifold_edges, st.max_dev, st.mean_dev, t.elapsed().as_secs_f64());
                render(&b, "/tmp/screws_a.png", glam::DVec3::new(0.6, 0.5, 1.0), glam::DVec3::new(0.0, 0.0, 0.0), 36.0, 1100, 1100);
                render(&b, "/tmp/screws_b.png", glam::DVec3::new(0.25, 0.3, 1.0), glam::DVec3::new(-4.0, 3.0, 10.0), 10.0, 1100, 1100);
                render(&b, "/tmp/screws_c.png", glam::DVec3::new(-0.5, 0.9, 1.0), glam::DVec3::new(-4.0, 3.0, 10.5), 4.0, 1100, 1100);
                std::env::set_var("BY_STEP", "1");
                render(&b, "/tmp/screws_d.png", glam::DVec3::new(-0.5, 0.9, 1.0), glam::DVec3::new(-4.0, 3.0, 10.5), 4.0, 1100, 1100);
                std::env::remove_var("BY_STEP");
            }
            None => println!("screws: not handled ({:.2}s)", t.elapsed().as_secs_f64()),
        }
    }
    if what == "variants" {
        use glam::DVec3;
        let cases: Vec<(&str, Scene)> = vec![
            ("decale", variant(Vec3::new(3.0, 0.0, 2.0), Quat::IDENTITY, Vec3::new(3.0, 14.0, 0.0), CombineMode::Carve, 0.8)),
            ("cote", variant(Vec3::ZERO, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), Vec3::new(3.0, 14.0, 0.0), CombineMode::Carve, 0.6)),
            ("borgne", variant(Vec3::new(0.0, 9.0, 0.0), Quat::IDENTITY, Vec3::new(4.0, 5.0, 0.0), CombineMode::Carve, 0.8)),
            ("bossage", variant(Vec3::new(0.0, 11.0, 0.0), Quat::IDENTITY, Vec3::new(4.0, 3.0, 0.0), CombineMode::Add, 1.0)),
            ("vif", variant(Vec3::ZERO, Quat::IDENTITY, Vec3::new(4.0, 14.0, 0.0), CombineMode::Carve, 0.0)),
            ("pres-bord", variant(Vec3::new(4.5, 0.0, 0.0), Quat::IDENTITY, Vec3::new(3.0, 14.0, 0.0), CombineMode::Carve, 0.6)),
        ];
        let mut cases = cases;
        {
            // per-edge fillets: three edges at 3 mm, one 1.5 mm chamfer, the rest 1 mm
            let mut s = cube_hole();
            let id = s.nodes.values().find(|n| n.name == "cube").unwrap().id;
            let sh = s.node_mut(id).unwrap().shape_mut().unwrap();
            for e in [8usize, 0, 9] {
                sh.set_edge(e, 3.0);
            }
            sh.set_edge(10, -1.5);
            cases.push(("par-arete", s));
        }
        for (i, (name, s)) in cases.iter().enumerate() {
            let bs = bake::build::BuildSettings { size: 2.25, tol: 0.03, fillet_rows: 0, threads: 2 };
            match bake::build::build(s, &bs, &bake::Progress::default()).ok() {
                Some(b) => {
                    let st = &b.stats;
                    println!("{name:10} {} faces ({} q {} t {} n) open {} nonman {} dev max {:.4}", b.faces.len(), st.quads, st.tris, st.ngons, st.open_edges, st.nonmanifold_edges, st.max_dev);
                    render(&b, &format!("/tmp/var_{i}.png"), DVec3::new(0.75, 0.9, 1.0), DVec3::new(0.0, 1.0, 0.0), 34.0, 700, 700);
                    if *name == "pres-bord" {
                        render(&b, "/tmp/var_zoom.png", DVec3::new(0.9, 0.8, 0.5), DVec3::new(8.0, 9.0, 0.0), 10.0, 700, 700);
                    }
                    if *name == "par-arete" {
                        render(&b, "/tmp/var_edges_b.png", DVec3::new(-0.8, 0.6, -0.9), DVec3::new(0.0, 1.0, 0.0), 34.0, 700, 700);
                    }
                }
                None => println!("{name:10} not handled"),
            }
        }
    }
    if what == "target" {
        let b = target_mesh();
        let mut und: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
        for f in &b.faces {
            for i in 0..f.verts.len() {
                let (a, c) = (f.verts[i], f.verts[(i + 1) % f.verts.len()]);
                *und.entry((a.min(c), a.max(c))).or_default() += 1;
            }
        }
        let quads = b.faces.iter().filter(|f| f.verts.len() == 4).count();
        println!("target: {} faces, {} quads, open {} nonmanifold {}", b.faces.len(), quads, und.values().filter(|&&c| c == 1).count(), und.values().filter(|&&c| c > 2).count());
        render(&b, "/tmp/target_a.png", glam::DVec3::new(0.75, 0.9, 1.0), glam::DVec3::new(0.0, 1.0, 0.0), 34.0, 1100, 1100);
        render(&b, "/tmp/target_b.png", glam::DVec3::new(0.35, 1.0, 0.55), glam::DVec3::new(1.5, 8.0, 2.0), 15.0, 1100, 1100);
    }
    if what == "terms" {
        per_term(&demo::grenade(), Quality::Standard);
    }
    if what == "all" || what == "bake" {
        // box with a hole
        let mut s = Scene::new();
        let root = s.root;
        let mut b = Shape::new(PrimKind::Box);
        b.round = 1.0;
        s.add(root, NodeSpec::shape("b", b));
        let mut c = Shape::new(PrimKind::Cylinder);
        c.size = Vec3::new(4.0, 14.0, 0.0);
        s.add(root, NodeSpec::shape("c", c).mode(CombineMode::Carve).blend(1.0));
        report("box - cylinder", &s, Quality::Standard);
        let b = bake::bake_blocking(&s, Quality::Standard);
        render(&b, "/tmp/boxhole.png", glam::DVec3::new(0.6, 0.7, 1.0), glam::DVec3::ZERO, 30.0, 900, 700);
        // union with blend
        let mut s = Scene::new();
        let root = s.root;
        let mut c = Shape::new(PrimKind::Cylinder);
        c.size = Vec3::new(10.0, 15.0, 0.0);
        c.round = 2.0;
        s.add(root, NodeSpec::shape("c", c));
        let mut sp = Shape::new(PrimKind::Sphere);
        sp.size = Vec3::splat(6.0);
        s.add(root, NodeSpec::shape("s", sp).at(Vec3::new(0.0, 0.0, 10.0)).blend(3.0).repeat(Repeat::Radial { count: 4, axis: Axis::Y }));
        report("cyl + spheres x4", &s, Quality::Standard);
        let b = bake::bake_blocking(&s, Quality::Standard);
        render(&b, "/tmp/spheres.png", glam::DVec3::new(0.6, 0.5, 1.0), glam::DVec3::ZERO, 34.0, 900, 700);
        let mut s = Scene::new();
        let root = s.root;
        let mut b = Shape::new(PrimKind::Box);
        b.size = Vec3::new(10.0, 6.0, 10.0);
        s.add(root, NodeSpec::shape("b", b));
        let mut g = Shape::new(PrimKind::Box);
        g.size = Vec3::new(0.1, 10.0, 14.0);
        s.add(root, NodeSpec::shape("g", g).at(Vec3::new(0.0, 6.0, 0.0)).rot(Quat::IDENTITY).mode(CombineMode::Groove).groove(1.2, 0.9));
        report("box groove", &s, Quality::Standard);
        check_cuts(&s, Quality::Standard);
        let b = bake::bake_blocking(&s, Quality::Standard);
        render(&b, "/tmp/groove.png", glam::DVec3::new(0.6, 0.7, 1.0), glam::DVec3::new(0.0, 4.0, 0.0), 26.0, 900, 700);
        render(&b, "/tmp/groove_end.png", glam::DVec3::new(0.3, 0.2, 1.0), glam::DVec3::new(0.0, -4.0, 10.0), 4.0, 900, 700);
        if args.get(2).is_some() {
            report("starter", &demo::starter(), Quality::Standard);
            let q = match std::env::var("Q").as_deref() { Ok("max") => Quality::Max, Ok("draft") => Quality::Draft, _ => Quality::Standard };
            report("grenade", &demo::grenade(), q);
            let b = bake::bake_blocking(&demo::grenade(), Quality::Standard);
            render(&b, "/tmp/grenade.png", glam::DVec3::new(0.7, 0.35, 1.0), glam::DVec3::new(15.0, 80.0, 0.0), 175.0, 1100, 1300);
            render(&b, "/tmp/grenade_zoom.png", glam::DVec3::new(0.3, 0.2, 1.0), glam::DVec3::new(0.0, 50.0, 25.0), 40.0, 1100, 900);
        }
    }
}

#[allow(dead_code)]
pub fn per_term(scene: &Scene, q: Quality) {
    use bake::patch::{self, PatchSettings, Term};
    let st = bake::BakeSettings::preset(q);
    let field = Field::new(&compile(scene));
    let names: Vec<String> = {
        // prim order = compile order = depth-first visible shapes
        let mut v = vec![];
        fn walk(s: &Scene, id: NodeId, v: &mut Vec<String>) {
            let n = &s.nodes[&id];
            if !n.visible {
                return;
            }
            match &n.kind {
                NodeKind::Shape(sh) => {
                    if !(sh.kind == PrimKind::Profile && sh.profile.as_ref().map(|p| p.points.len() < 3).unwrap_or(true)) {
                        v.push(n.name.clone())
                    }
                }
                NodeKind::Group { children, .. } => {
                    for c in children {
                        walk(s, *c, v);
                    }
                }
            }
        }
        walk(scene, scene.root, &mut v);
        v
    };
    let gs = gen::GenSettings { tol: st.tol, max_edge: st.max_edge, segs: 0 };
    let ps = PatchSettings { trim_len: st.trim_len, min_size: st.trim_len / 8.0, max_level: 9 };
    for (prim, level) in field.term_levels() {
        let pr = &field.prims[prim as usize];
        let base = gen::generate(pr, level / pr.sc, &gs);
        let mut tot = (0, 0, 0, 0);
        let insts = field.instances(prim);
        let t = std::time::Instant::now();
        for inst in &insts {
            let key = bake::field::Owner { prim, copy: inst.copy, level }.key();
            let term = Term { key, inst: *inst, level_local: level / pr.sc };
            let o = patch::process(&field, &term, &base, &[], &[], &ps);
            tot.0 += o.faces.len();
            tot.1 += o.refined;
            tot.2 += o.cuts;
            tot.3 += 1;
        }
        println!(
            "{:>2} {:<16} lvl {:>6.2} x{:<2} base {:>6}  out {:>7} refined {:>7} cuts {:>6}  {:.2}s",
            prim,
            names.get(prim as usize).cloned().unwrap_or_default(),
            level,
            tot.3,
            base.faces.len(),
            tot.0,
            tot.1,
            tot.2,
            t.elapsed().as_secs_f64()
        );
    }
}

/// CPU render of a baked mesh: shaded polygons, polygon edges, open edges in red.
#[allow(dead_code)]
pub fn render(b: &bake::Baked, path: &str, eye_dir: glam::DVec3, target: glam::DVec3, extent: f64, w: u32, h: u32) {
    use glam::DVec3;
    let fwd = (-eye_dir).normalize();
    let right = fwd.cross(DVec3::Y).normalize();
    let up = right.cross(fwd);
    let scale = w.min(h) as f64 / extent;
    let proj = |p: DVec3| -> (f64, f64, f64) {
        let d = p - target;
        (w as f64 * 0.5 + d.dot(right) * scale, h as f64 * 0.5 - d.dot(up) * scale, d.dot(fwd))
    };
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([24, 26, 30]));
    let mut zb = vec![f64::MAX; (w * h) as usize];
    let light = DVec3::new(-0.4, 0.8, 0.5).normalize();
    let palette = [[0.62, 0.62, 0.6], [0.45, 0.55, 0.32], [0.92, 0.5, 0.2], [0.3, 0.32, 0.36], [0.7, 0.72, 0.76], [0.2, 0.2, 0.22]];
    let mut edges: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
    for f in &b.faces {
        for i in 0..f.verts.len() {
            let (a, c) = (f.verts[i], f.verts[(i + 1) % f.verts.len()]);
            *edges.entry((a.min(c), a.max(c))).or_default() += 1;
        }
    }
    for f in &b.faces {
        let pts: Vec<DVec3> = f.verts.iter().map(|v| b.positions[*v as usize]).collect();
        let n = f.normals.iter().fold(DVec3::ZERO, |s, n| s + *n).normalize_or_zero();
        if n.dot(fwd) > 0.2 {
            // back face: still draw (helps to see holes) but darker
        }
        let shade = (n.dot(light).max(0.0) * 0.75 + 0.25).min(1.0);
        let col = if std::env::var("BY_STEP").is_ok() {
            // constructive bake groups: native, rings, fillet, tool rings, tool native
            [[0.66, 0.68, 0.64], [0.55, 0.68, 0.86], [0.95, 0.6, 0.3], [0.68, 0.55, 0.85], [0.42, 0.72, 0.66]][(f.patch as usize).min(4)]
        } else if std::env::var("BY_PATCH").is_ok() {
            if f.patch == u32::MAX {
                [1.0, 0.2, 0.2]
            } else {
                let h = (f.patch.wrapping_mul(2654435761) >> 8) as f64 / 16777216.0;
                [0.4 + 0.5 * (h * 6.28).sin().abs(), 0.4 + 0.5 * (h * 11.0).cos().abs(), 0.4 + 0.5 * (h * 3.7).sin().abs()]
            }
        } else {
            palette[(f.mat as usize).min(5)]
        };
        let rgb = image::Rgb([(col[0] * shade * 255.0) as u8, (col[1] * shade * 255.0) as u8, (col[2] * shade * 255.0) as u8]);
        for k in 1..pts.len() - 1 {
            let t = [proj(pts[0]), proj(pts[k]), proj(pts[k + 1])];
            let minx = t.iter().map(|p| p.0).fold(f64::MAX, f64::min).floor().max(0.0) as i64;
            let maxx = t.iter().map(|p| p.0).fold(f64::MIN, f64::max).ceil().min(w as f64 - 1.0) as i64;
            let miny = t.iter().map(|p| p.1).fold(f64::MAX, f64::min).floor().max(0.0) as i64;
            let maxy = t.iter().map(|p| p.1).fold(f64::MIN, f64::max).ceil().min(h as f64 - 1.0) as i64;
            let area = (t[1].0 - t[0].0) * (t[2].1 - t[0].1) - (t[2].0 - t[0].0) * (t[1].1 - t[0].1);
            if area.abs() < 1e-12 {
                continue;
            }
            for y in miny..=maxy {
                for x in minx..=maxx {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    let w0 = ((t[1].0 - px) * (t[2].1 - py) - (t[2].0 - px) * (t[1].1 - py)) / area;
                    let w1 = ((t[2].0 - px) * (t[0].1 - py) - (t[0].0 - px) * (t[2].1 - py)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * t[0].2 + w1 * t[1].2 + w2 * t[2].2;
                    let i = (y as u32 * w + x as u32) as usize;
                    if z < zb[i] {
                        zb[i] = z;
                        img.put_pixel(x as u32, y as u32, rgb);
                    }
                }
            }
        }
    }
    // edges
    let line = |a: DVec3, c: DVec3, col: image::Rgb<u8>, thick: i64, img: &mut image::RgbImage| {
        let (pa, pc) = (proj(a), proj(c));
        let steps = ((pc.0 - pa.0).abs().max((pc.1 - pa.1).abs()).ceil() as usize).max(1);
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let x = pa.0 + (pc.0 - pa.0) * t;
            let y = pa.1 + (pc.1 - pa.1) * t;
            let z = pa.2 + (pc.2 - pa.2) * t;
            for dx in -thick..=thick {
                for dy in -thick..=thick {
                    let (xi, yi) = (x as i64 + dx, y as i64 + dy);
                    if xi < 0 || yi < 0 || xi >= w as i64 || yi >= h as i64 {
                        continue;
                    }
                    let i = (yi as u32 * w + xi as u32) as usize;
                    if thick > 0 || z <= zb[i] + 0.05 * extent / 100.0 {
                        img.put_pixel(xi as u32, yi as u32, col);
                    }
                }
            }
        }
    };
    for (&(a, c), &cnt) in &edges {
        let (pa, pc) = (b.positions[a as usize], b.positions[c as usize]);
        if cnt == 2 {
            line(pa, pc, image::Rgb([10, 10, 12]), 0, &mut img);
        }
    }
    for (&(a, c), &cnt) in &edges {
        let (pa, pc) = (b.positions[a as usize], b.positions[c as usize]);
        if cnt == 1 {
            line(pa, pc, image::Rgb([255, 40, 40]), 1, &mut img);
        } else if cnt > 2 {
            line(pa, pc, image::Rgb([255, 0, 255]), 1, &mut img);
        }
    }
    img.save(path).unwrap();
}

/// Welded quad mesh under construction.
#[derive(Default)]
struct Weld {
    pos: Vec<glam::DVec3>,
    faces: Vec<(Vec<u32>, u32)>,
    map: std::collections::HashMap<(i64, i64, i64), u32>,
}

impl Weld {
    fn v(&mut self, p: glam::DVec3) -> u32 {
        let k = ((p.x * 1e6).round() as i64, (p.y * 1e6).round() as i64, (p.z * 1e6).round() as i64);
        let n = self.pos.len() as u32;
        *self.map.entry(k).or_insert_with(|| {
            self.pos.push(p);
            n
        })
    }
    fn quad(&mut self, q: [glam::DVec3; 4], patch: u32) {
        let f: Vec<u32> = q.iter().map(|p| self.v(*p)).collect();
        self.faces.push((f, patch));
    }
}

/// Front face of the bearing between its two rings (bore fillet inside,
/// 1 mm round outside), down to the line where the base fillet starts.
/// `inner` and `outer`: segments along the arc part; `chord`: along the
/// bottom; outer - inner must be even (3-to-1 reductions in the middle row).
fn front_face(full: &Field, inner: usize, outer: usize, chord: usize) -> (bake::Baked, String) {
    let mut m = Weld::default();
    let f = front_face_into(&mut m, full, inner, outer, chord);
    let (b, open, nonman, dev) = weld_baked(m, full);
    let face_n = b.faces.iter().filter(|f| f.patch != 2).count();
    let info = format!(
        "{} quads (face {face_n}, réductions {}), colonnes dedans {} / dehors {}, bords libres {open}, non-manifold {nonman}, écart max {dev:.3} mm, face la plus étroite {:.2} mm",
        b.faces.len(),
        f.reductions,
        f.n_in,
        f.n_out,
        f.pinch
    );
    (b, info)
}

struct Front {
    /// where the face meets the base fillet, from +x to -x: up the round's
    /// end column, the chord, down the other end column
    bottom: Vec<glam::DVec3>,
    /// the outer round's edge on the bearing's body, from -x over the top to +x
    rim: Vec<glam::DVec3>,
    /// the bore fillet's edge on the bore wall, all around
    bore: Vec<glam::DVec3>,
    reductions: usize,
    n_in: usize,
    n_out: usize,
    pinch: f64,
}

fn front_face_into(m: &mut Weld, full: &Field, inner: usize, outer: usize, chord: usize) -> Front {
    use glam::DVec3;
    use std::f64::consts::{FRAC_PI_2, PI, TAU};
    let steps = full.steps().unwrap();
    let pal = &full.prims[steps[1].prim as usize];
    let inst = full.instances(steps[1].prim)[0];
    let (r, hh, rd) = (pal.size.x, pal.size.y, pal.round);
    let (rb, kb) = (full.prims[steps[2].prim as usize].size.x, steps[2].k);
    let base = &full.prims[steps[0].prim as usize];
    let base_top = full.instances(steps[0].prim)[0].to_world.translation.y + base.size.y;
    // the base fillet reaches the bearing up to this local z (world height)
    let zc = inst.to_local.transform_point3(DVec3::new(0.0, base_top + steps[1].k, 0.0)).z;
    let w = |rho: f64, y: f64, a: f64| inst.to_world.transform_point3(DVec3::new(rho * a.cos(), y, rho * a.sin()));
    let ri = rb + kb;
    let ro = r - rd;
    let a1 = (zc / ro).asin();
    let a2 = PI - a1;
    let span = TAU - (a2 - a1);
    // inner circle samples: the bottom (chord) part, then the arc part
    let mut ang_in: Vec<f64> = (0..chord).map(|i| a1 + (a2 - a1) * i as f64 / chord as f64).collect();
    ang_in.extend((0..inner).map(|i| a2 + span * i as f64 / inner as f64));
    // outer boundary: chord points, then the arc at the round's edge
    let (xa, xb) = (ro * a1.cos(), ro * a2.cos());
    let mut bound: Vec<DVec3> = (0..chord).map(|i| inst.to_world.transform_point3(DVec3::new(xa + (xb - xa) * i as f64 / chord as f64, hh, zc))).collect();
    let arc: Vec<DVec3> = (0..=outer).map(|i| w(ro, hh, a2 + span * i as f64 / outer as f64)).collect();
    bound.extend(arc[..outer].iter().copied());
    let n_in = ang_in.len();
    let n_out = bound.len();
    let ring0: Vec<DVec3> = ang_in.iter().map(|a| w(ri, hh, *a)).collect();
    // outer segments per inner segment: 1, or 3 for a reduction, spread evenly
    let k = (outer - inner) / 2;
    let mut per = vec![1usize; n_in];
    for j in 0..k {
        per[chord + ((j as f64 + 0.5) * inner as f64 / k as f64) as usize] = 3;
    }
    // where each outer vertex sits on the inner side (to place the rows)
    let mut foot = vec![DVec3::ZERO; n_out];
    let mut first = vec![0usize; n_in + 1];
    let mut o = 0;
    for j in 0..n_in {
        first[j] = o;
        for m in 0..per[j] {
            let t = m as f64 / per[j] as f64;
            let a = ang_in[j] + (if j + 1 < n_in { ang_in[j + 1] } else { ang_in[0] + TAU } - ang_in[j]) * t;
            foot[o] = w(ri, hh, a);
            o += 1;
        }
    }
    first[n_in] = n_out;
    assert_eq!(o, n_out);
    // the boundary point straight out of each inner sample, for row 1
    let out_of = |j: usize| -> DVec3 { bound[first[j]] };
    let ring1: Vec<DVec3> = (0..n_in).map(|j| ring0[j].lerp(out_of(j), 1.0 / 3.0)).collect();
    let ring2: Vec<DVec3> = (0..n_out).map(|o| foot[o].lerp(bound[o], 2.0 / 3.0)).collect();
    for j in 0..n_in {
        let j2 = (j + 1) % n_in;
        m.quad([ring0[j], ring0[j2], ring1[j2], ring1[j]], 1);
    }
    for j in 0..n_in {
        let j2 = (j + 1) % n_in;
        let o0 = first[j];
        let r2 = |o: usize| ring2[o % n_out];
        if per[j] == 1 {
            m.quad([ring1[j], ring1[j2], r2(o0 + 1), r2(o0)], 1);
        } else {
            let a = ((ring1[j] * 2.0 + ring1[j2]) / 3.0).lerp(r2(o0 + 1), 0.5);
            let b = ((ring1[j] + ring1[j2] * 2.0) / 3.0).lerp(r2(o0 + 2), 0.5);
            m.quad([ring1[j], a, r2(o0 + 1), r2(o0)], 3);
            m.quad([a, b, r2(o0 + 2), r2(o0 + 1)], 3);
            m.quad([b, ring1[j2], r2(o0 + 3), r2(o0 + 2)], 3);
            m.quad([ring1[j], ring1[j2], b, a], 3);
        }
    }
    for o in 0..n_out {
        let o2 = (o + 1) % n_out;
        m.quad([ring2[o], ring2[o2], bound[o2], bound[o]], 1);
    }
    // bore fillet: rows from the face down into the bore, same columns
    let rows_b = 2;
    let bez = |a: f64, t: f64| -> DVec3 {
        let (p, c, q) = (w(ri, hh, a), w(rb, hh, a), w(rb, hh - kb, a));
        let x = p * (1.0 - t) * (1.0 - t) + c * 2.0 * t * (1.0 - t) + q * t * t;
        if t > 0.0 && t < 1.0 {
            full.project(x).unwrap_or(x)
        } else {
            x
        }
    };
    for j in 0..n_in {
        let (a, b) = (ang_in[j], ang_in[(j + 1) % n_in]);
        for t in 0..rows_b {
            let (t0, t1) = (t as f64 / rows_b as f64, (t + 1) as f64 / rows_b as f64);
            m.quad([bez(a, t0), bez(b, t0), bez(b, t1), bez(a, t1)], 2);
        }
    }
    // outer round over the arc part; its end columns follow the line where
    // the base fillet starts
    let rows_r = 2;
    let round_pt = |i: usize, t: usize| -> DVec3 {
        if t == 0 {
            return arc[i];
        }
        let phi = t as f64 / rows_r as f64 * FRAC_PI_2;
        let rho = ro + rd * phi.sin();
        let y = hh - rd + rd * phi.cos();
        let c2 = PI - (zc / rho).asin();
        let c1 = (zc / rho).asin() + TAU;
        w(rho, y, c2 + (c1 - c2) * i as f64 / outer as f64)
    };
    for i in 0..outer {
        for t in 0..rows_r {
            m.quad([round_pt(i, t), round_pt(i + 1, t), round_pt(i + 1, t + 1), round_pt(i, t + 1)], 2);
        }
    }
    let mut bottom: Vec<DVec3> = (1..=rows_r).rev().map(|t| round_pt(outer, t)).collect();
    bottom.extend(bound[..chord].iter().copied());
    bottom.extend((0..=rows_r).map(|t| round_pt(0, t)));
    let pinch = (0..chord).map(|i| ring0[i].distance(bound[i])).fold(f64::MAX, f64::min);
    let rim = (0..=outer).map(|i| round_pt(i, rows_r)).collect();
    let bore = ang_in.iter().map(|a| bez(*a, rows_b as f64 / rows_b as f64)).collect();
    Front { bottom, rim, bore, reductions: k, n_in, n_out, pinch }
}

/// The welded quads as a mesh facing out of `full`, with its open and
/// non-manifold edge counts and its largest distance to the surface.
fn weld_baked(m: Weld, full: &Field) -> (bake::Baked, usize, usize, f64) {
    use glam::DVec3;
    let mut b = bake::Baked::default();
    b.positions = m.pos.clone();
    for (mut f, patch) in m.faces {
        let c = f.iter().fold(DVec3::ZERO, |s, v| s + m.pos[*v as usize]) / f.len() as f64;
        let mut nrm = DVec3::ZERO;
        for i in 0..f.len() {
            nrm += m.pos[f[i] as usize].cross(m.pos[f[(i + 1) % f.len()] as usize]);
        }
        if nrm.dot(full.grad(c)) < 0.0 {
            f.reverse();
        }
        let normals = f.iter().map(|v| full.grad(m.pos[*v as usize]).normalize_or_zero()).collect();
        b.faces.push(bake::BFace { verts: f, normals, mat: 0, patch });
    }
    let edges = edge_counts(&b);
    let open = edges.values().filter(|&&c| c == 1).count();
    let nonman = edges.values().filter(|&&c| c > 2).count();
    let dev = b.positions.iter().map(|p| full.dist(*p).abs()).fold(0.0, f64::max);
    (b, open, nonman, dev)
}

fn edge_counts(b: &bake::Baked) -> std::collections::HashMap<(u32, u32), u32> {
    let mut edges = std::collections::HashMap::new();
    for f in &b.faces {
        for i in 0..f.verts.len() {
            let (a, c) = (f.verts[i], f.verts[(i + 1) % f.verts.len()]);
            *edges.entry((a.min(c), a.max(c))).or_default() += 1;
        }
    }
    edges
}

fn raccord() {
    use glam::DVec3;
    let out = std::env::var("OUT").unwrap_or_else(|_| "/tmp".into());
    let s = support(4);
    let full = Field::new(&compile(&s));
    let part = bake::bake_blocking(&s, Quality::Max);
    let bs = bake::build::BuildSettings { size: 1.5, tol: 0.02, fillet_rows: 0, threads: 2 };
    let j = bake::build::junction(&support(2), 1, &bs).expect("junction");
    let rail: Vec<(&Vec<DVec3>, [u8; 3], bool, bool)> = j.rail_b.iter().map(|c| (c, [255, 120, 40], false, true)).collect();
    // A: same columns everywhere; B: ~1 mm everywhere with reductions
    let props = [("A", 36, 36, 12), ("B", 30, 46, 12)];
    for (name, inner, outer, chord) in props {
        let (b, info) = front_face(&full, inner, outer, chord);
        println!("{name} : {info}");
        let views = [
            ("vue", DVec3::new(0.3, 0.25, 1.0), DVec3::new(0.0, 19.0, 14.0), 26.0),
            ("bas", DVec3::new(0.3, 0.45, 1.0), DVec3::new(0.0, 12.5, 14.0), 9.0),
            ("haut", DVec3::new(0.25, 0.2, 1.0), DVec3::new(5.0, 26.0, 14.0), 9.0),
        ];
        for (v, dir, target, ext) in views {
            render_lines(Some(&part), Some(&b), &rail, &format!("{out}/raccord-{name}-{v}.png"), dir, target, ext, 1400, 1050);
        }
        render_lines(None, Some(&b), &rail, &format!("{out}/raccord-{name}-seul.png"), DVec3::new(0.3, 0.25, 1.0), DVec3::new(0.0, 19.0, 14.0), 26.0, 1400, 1050);
    }
}

/// Where the closed polyline `l` crosses the plane through `c` square to
/// `t`, the crossing nearest to `c` (same as in the constructive bake).
fn cut(l: &[glam::DVec3], c: glam::DVec3, t: glam::DVec3) -> Option<glam::DVec3> {
    let n = l.len();
    let mut best: Option<(f64, glam::DVec3)> = None;
    for i in 0..n {
        let (a, b) = (l[i], l[(i + 1) % n]);
        let (da, db) = ((a - c).dot(t), (b - c).dot(t));
        if (da <= 0.0) != (db <= 0.0) {
            let x = a.lerp(b, da / (da - db));
            let d = x.distance_squared(c);
            if best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, x));
            }
        }
    }
    best.map(|(_, x)| x)
}

/// The bearing / base fillet as one strip around the bearing, through the
/// given points of its top edge (on the bearing). Each column takes its
/// section from the bevel-like sections square to the corner curve, picked by
/// where it sits on the top edge: so the front columns land exactly on the
/// front face's bottom vertices and the strip shares them.
fn base_fillet(m: &mut Weld, full: &Field, j: &bake::build::Junction, top: &[glam::DVec3], rows: usize) -> Vec<Vec<glam::DVec3>> {
    let cols: Vec<Vec<glam::DVec3>> = base_sections(full, j, top).iter().map(|&(p, c, q)| fillet_column(full, p, c, q, rows)).collect();
    strip_quads(m, &cols, true, 2);
    cols
}

/// (top point, corner, base point) of each column of the bearing / base fillet.
fn base_sections(full: &Field, j: &bake::build::Junction, top: &[glam::DVec3]) -> Vec<(glam::DVec3, glam::DVec3, glam::DVec3)> {
    use glam::DVec3;
    let longest = |ls: &[Vec<DVec3>]| ls.iter().max_by_key(|l| l.len()).unwrap().clone();
    let (corner, rail_top, rail_base) = (longest(&j.corner), longest(&j.rail_b), longest(&j.rail_a));
    // corner curve every 0.1 mm or less, one section per point
    let mut spine = vec![];
    for i in 0..corner.len() {
        let (a, b) = (corner[i], corner[(i + 1) % corner.len()]);
        let n = (a.distance(b) / 0.1).ceil().max(1.0) as usize;
        spine.extend((0..n).map(|k| a.lerp(b, k as f64 / n as f64)));
    }
    let ns = spine.len();
    let sec: Vec<(DVec3, DVec3, DVec3)> = (0..ns)
        .filter_map(|i| {
            let c = spine[i];
            let t = (spine[(i + 1) % ns] - spine[(i + ns - 1) % ns]).normalize_or_zero();
            Some((cut(&rail_top, c, t)?, c, cut(&rail_base, c, t)?))
        })
        .collect();
    let n = sec.len();
    // the section through a top point: between the two nearest sections
    let section = |p: DVec3| -> (DVec3, DVec3) {
        let i = (0..n).min_by(|&a, &b| sec[a].0.distance_squared(p).partial_cmp(&sec[b].0.distance_squared(p)).unwrap()).unwrap();
        let mut best = (f64::MAX, sec[i].1, sec[i].2);
        for (a, b) in [((i + n - 1) % n, i), (i, (i + 1) % n)] {
            let (pa, pb) = (sec[a].0, sec[b].0);
            let d = pb - pa;
            let f = if d.length_squared() > 1e-12 { ((p - pa).dot(d) / d.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
            let e = p.distance(pa + d * f);
            if e < best.0 {
                best = (e, sec[a].1.lerp(sec[b].1, f), sec[a].2.lerp(sec[b].2, f));
            }
        }
        (best.1, best.2)
    };
    top.iter()
        .map(|&p| {
            let (c, q) = section(p);
            (p, c, full.project(q).unwrap_or(q))
        })
        .collect()
}

/// One column of a fillet strip: rows along the quadratic from `p` (first
/// rail) through the corner `c` to `q` (second rail), projected on the surface.
fn fillet_column(full: &Field, p: glam::DVec3, c: glam::DVec3, q: glam::DVec3, rows: usize) -> Vec<glam::DVec3> {
    (0..=rows)
        .map(|r| {
            let t = r as f64 / rows as f64;
            let x = p * (1.0 - t) * (1.0 - t) + c * 2.0 * t * (1.0 - t) + q * t * t;
            if r == 0 || r == rows {
                x
            } else {
                full.project(x).unwrap_or(x)
            }
        })
        .collect()
}

/// Quads between consecutive columns of equal length.
fn strip_quads(m: &mut Weld, cols: &[Vec<glam::DVec3>], closed: bool, patch: u32) {
    let n = cols.len();
    let segs = if closed { n } else { n - 1 };
    for i in 0..segs {
        let (a, b) = (&cols[i], &cols[(i + 1) % n]);
        for r in 0..a.len() - 1 {
            m.quad([a[r], b[r], b[r + 1], a[r + 1]], patch);
        }
    }
}

/// Front face (proposal A) welded to the bearing / base fillet.
fn liaison() {
    use glam::DVec3;
    let out = std::env::var("OUT").unwrap_or_else(|_| "/tmp".into());
    let s = support(4);
    let full = Field::new(&compile(&s));
    let part = bake::bake_blocking(&s, Quality::Max);
    let bs = bake::build::BuildSettings { size: 1.5, tol: 0.02, fillet_rows: 0, threads: 2 };
    let j = bake::build::junction(&support(2), 1, &bs).expect("junction");
    let mut m = Weld::default();
    let front = front_face_into(&mut m, &full, 36, 36, 12);
    // top edge of the fillet all around: the face's bottom at the front, the
    // bearing's side lines, and the same at the back (mirrored, for when the
    // back face is built the same way)
    let fb = &front.bottom;
    let (first, last) = (fb[0], fb[fb.len() - 1]);
    let side = |from: DVec3, to: DVec3| -> Vec<DVec3> {
        let n = ((from.distance(to) / bs.size).round() as usize).max(1);
        (1..n).map(|i| from.lerp(to, i as f64 / n as f64)).collect()
    };
    let mirror = |p: DVec3| DVec3::new(p.x, p.y, -p.z);
    let mut top: Vec<DVec3> = fb.clone();
    top.extend(side(last, mirror(last)));
    top.extend(fb.iter().rev().map(|p| mirror(*p)));
    top.extend(side(mirror(first), first));
    let rows = 3;
    let cols = base_fillet(&mut m, &full, &j, &top, rows);
    let n_fillet = cols.len() * rows;
    let base_edges: Vec<f64> = (0..cols.len()).map(|i| cols[i][rows].distance(cols[(i + 1) % cols.len()][rows])).collect();
    let (base_min, base_max) = (base_edges.iter().cloned().fold(f64::MAX, f64::min), base_edges.iter().cloned().fold(0.0, f64::max));
    if std::env::var("DBG").is_ok() {
        for (i, c) in cols.iter().enumerate() {
            if c[0].x > -1.0 && c[0].z > 10.0 {
                println!("col {i:2} top {:6.3} {:6.3} {:6.3}  base {:6.3} {:6.3} {:6.3}", c[0].x, c[0].y, c[0].z, c[rows].x, c[rows].y, c[rows].z);
            }
        }
    }
    let (b, open, nonman, dev) = weld_baked(m, &full);
    // the join itself: every edge of the face's bottom must have two faces
    let edges = edge_counts(&b);
    let id = |p: DVec3| b.positions.iter().position(|q| q.distance(p) < 1e-6).unwrap() as u32;
    let joined = (0..fb.len() - 1).filter(|&i| {
        let (a, c) = (id(fb[i]), id(fb[i + 1]));
        edges.get(&(a.min(c), a.max(c))) == Some(&2)
    });
    println!(
        "{} quads (fillet socle {n_fillet}, {} colonnes x {rows} rangées), jonction soudée {}/{} arêtes, bords libres {open}, non-manifold {nonman}, écart max {dev:.3} mm, arêtes côté socle {base_min:.2} à {base_max:.2} mm",
        b.faces.len(),
        top.len(),
        joined.count(),
        fb.len() - 1
    );
    let rail: Vec<(&Vec<DVec3>, [u8; 3], bool, bool)> = vec![];
    let views = [
        ("vue", DVec3::new(0.3, 0.25, 1.0), DVec3::new(0.0, 17.0, 14.0), 28.0),
        ("bas", DVec3::new(0.3, 0.45, 1.0), DVec3::new(0.0, 11.5, 14.5), 10.0),
        ("coin", DVec3::new(0.9, 0.7, 1.0), DVec3::new(7.5, 11.0, 13.5), 8.0),
        ("tour", DVec3::new(0.75, 0.6, 1.0), DVec3::new(0.0, 15.0, 0.0), 50.0),
    ];
    for (v, dir, target, ext) in views {
        render_lines(Some(&part), Some(&b), &rail, &format!("{out}/liaison-{v}.png"), dir, target, ext, 1400, 1050);
    }
    render_lines(None, Some(&b), &rail, &format!("{out}/liaison-seul.png"), DVec3::new(0.45, 0.35, 1.0), DVec3::new(0.0, 15.0, 8.0), 34.0, 1400, 1050);
    render_lines(None, Some(&b), &rail, &format!("{out}/liaison-seul-coin.png"), DVec3::new(0.9, 0.7, 1.0), DVec3::new(7.5, 11.0, 13.5), 8.0, 1400, 1050);
}

/// `n` steps from the last value of `v` to `to`, `to` itself exact.
fn run(v: &mut Vec<f64>, to: f64, n: usize) {
    let a = *v.last().unwrap();
    for i in 1..n {
        v.push(a + (to - a) * i as f64 / n as f64);
    }
    v.push(to);
}

/// Fill grid: column 0 is `inner` (the edge of a fillet strip), the next
/// columns sit on the x lines; rows leave the inner edge at its own z and
/// straighten onto `zn` after `blend` columns. Cells inside the hole blocks
/// (i0, i1, j0, j1) are left for the holes.
fn region(m: &mut Weld, inner: &[glam::DVec3], xs: &[f64], zn: &[f64], y: f64, blend: usize, holes: &[(usize, usize, usize, usize)]) -> Vec<Vec<glam::DVec3>> {
    use glam::DVec3;
    assert_eq!(inner.len(), zn.len());
    let mut g = vec![inner.to_vec()];
    for (i, &x) in xs.iter().enumerate() {
        let s = (i + 1) as f64 / blend as f64;
        g.push(inner.iter().zip(zn).map(|(p, &z)| DVec3::new(x, y, if s >= 1.0 { z } else { p.z + (z - p.z) * s })).collect());
    }
    grid_quads(m, &g, holes);
    g
}

fn grid_quads(m: &mut Weld, g: &[Vec<glam::DVec3>], holes: &[(usize, usize, usize, usize)]) {
    for i in 0..g.len() - 1 {
        for j in 0..g[i].len() - 1 {
            if holes.iter().any(|&(i0, i1, j0, j1)| i >= i0 && i < i1 && j >= j0 && j < j1) {
                continue;
            }
            m.quad([g[i][j], g[i + 1][j], g[i + 1][j + 1], g[i][j + 1]], 1);
        }
    }
}

/// The edge of a hole block of the grid, all around.
fn block_edge(g: &[Vec<glam::DVec3>], (i0, i1, j0, j1): (usize, usize, usize, usize)) -> Vec<glam::DVec3> {
    let mut v = vec![];
    v.extend((i0..i1).map(|i| g[i][j0]));
    v.extend((j0..j1).map(|j| g[i1][j]));
    v.extend((i0 + 1..=i1).rev().map(|i| g[i][j1]));
    v.extend((j0 + 1..=j1).rev().map(|j| g[i0][j]));
    v
}

/// The whole test part with the fillets-first rule: every fillet and round
/// as a strip of quads (orange), then the fill paths between the strips
/// (cream), all welded on shared vertices. Written for this part (bearing
/// along z on the base, a hole on each corner of the base) to see the
/// topology; the general version belongs in the constructive bake.
fn piece_mesh() -> (Field, bake::Baked) {
    use glam::DVec3;
    use std::f64::consts::FRAC_PI_2;
    let s = support(4);
    let full = Field::new(&compile(&s));
    let steps = full.steps().unwrap();
    let bs = bake::build::BuildSettings { size: 1.5, tol: 0.02, fillet_rows: 0, threads: 2 };
    let size = bs.size;
    let j = bake::build::junction(&support(2), 1, &bs).expect("junction");
    let mut m = Weld::default();
    let mirror = |p: DVec3| DVec3::new(p.x, p.y, -p.z);
    let lin = |a: f64, b: f64, n: usize| -> Vec<f64> {
        let mut v = vec![a];
        run(&mut v, b, n);
        v
    };

    // ---- 1. fillets and rounds (orange) --------------------------------
    // the bearing's front face with its two rings, the back as its mirror
    let mut fw = Weld::default();
    let front = front_face_into(&mut fw, &full, 36, 36, 12);
    for (f, patch) in &fw.faces {
        let q: Vec<DVec3> = f.iter().map(|v| fw.pos[*v as usize]).collect();
        m.quad([q[0], q[1], q[2], q[3]], *patch);
        m.quad([mirror(q[0]), mirror(q[1]), mirror(q[2]), mirror(q[3])], *patch);
    }
    // the bearing / base fillet all around: the faces' bottom edges at both
    // ends, straight lines along the body
    let fb = &front.bottom;
    let nf = fb.len();
    let (first, last) = (fb[0], fb[nf - 1]);
    let side = |from: DVec3, to: DVec3| -> Vec<DVec3> {
        let n = ((from.distance(to) / size).round() as usize).max(1);
        (1..n).map(|i| from.lerp(to, i as f64 / n as f64)).collect()
    };
    let left = side(last, mirror(last));
    let right = side(mirror(first), first);
    let ns = left.len();
    let mut top: Vec<DVec3> = fb.clone();
    top.extend(left.iter().copied());
    top.extend(fb.iter().rev().map(|p| mirror(*p)));
    top.extend(right.iter().copied());
    let back0 = nf + ns;
    let right0 = back0 + nf;
    let mut secs = base_sections(&full, &j, &top);
    // the base: tangent lines of its 1.5 mm round
    let base = &full.prims[steps[0].prim as usize];
    let bc = full.instances(steps[0].prim)[0].to_world.translation;
    let rr = base.round;
    let (xt, zt) = (base.size.x - rr, base.size.z - rr);
    let (ytop, ybot) = (bc.y + base.size.y, bc.y - base.size.y);
    let (yhi, ylo) = (ytop - rr, ybot + rr);
    // at both ends the fillet's base edge leaves the base's top face for its
    // round: the column nearest that point lands exactly on it
    let rail_base = j.rail_a.iter().max_by_key(|l| l.len()).unwrap().clone();
    let cross = [2, nf - 3, back0 + 2, back0 + nf - 3];
    for &i in &cross {
        let q = secs[i].2;
        let z = zt * q.z.signum();
        assert!((q.z.abs() - zt).abs() < 0.6, "colonne {i} loin du bord du dessus ({:.2})", q.z);
        let x = cut(&rail_base, DVec3::new(q.x, ytop, z), DVec3::Z).expect("rail du socle").x;
        secs[i].2 = DVec3::new(x, ytop, z);
    }
    let rows = 3;
    let cols: Vec<Vec<DVec3>> = secs.iter().map(|&(p, c, q)| fillet_column(&full, p, c, q, rows)).collect();
    strip_quads(&mut m, &cols, true, 2);
    let qb = |i: usize| cols[i][rows];

    // fixing holes: 0.4 mm fillets top and bottom
    let hole = &full.prims[steps[3].prim as usize];
    let (hr, hk) = (hole.size.x, steps[3].k);
    let centers: Vec<DVec3> = full.instances(steps[3].prim).iter().map(|i| i.to_world.translation).collect();
    let (hxc, hzc) = (centers[0].x.abs(), centers[0].z.abs());
    let hs = hr + hk + 0.6; // half side of the square around each hole
    let kq = 5; // segments per side of that square
    let nwall = (((ytop - ybot - 2.0 * hk) / size).round() as usize).max(1);
    let hole_at = |m: &mut Weld, ctr: DVec3, sq: &[DVec3]| {
        let at = |a: f64, r: f64, y: f64| DVec3::new(ctr.x + r * a.cos(), y, ctr.z + r * a.sin());
        let ang: Vec<f64> = sq.iter().map(|p| (p.z - ctr.z).atan2(p.x - ctr.x)).collect();
        let ring_t: Vec<Vec<DVec3>> = sq.iter().zip(&ang).map(|(p, &a)| vec![*p, at(a, hr + hk, ytop)]).collect();
        let fil_t: Vec<Vec<DVec3>> = ang.iter().map(|&a| fillet_column(&full, at(a, hr + hk, ytop), at(a, hr, ytop), at(a, hr, ytop - hk), 2)).collect();
        let wall: Vec<Vec<DVec3>> = ang
            .iter()
            .map(|&a| (0..=nwall).map(|k| at(a, hr, if k == 0 { ytop - hk } else if k == nwall { ybot + hk } else { ytop - hk + (ybot - ytop + 2.0 * hk) * k as f64 / nwall as f64 })).collect())
            .collect();
        let fil_b: Vec<Vec<DVec3>> = ang.iter().map(|&a| fillet_column(&full, at(a, hr, ybot + hk), at(a, hr, ybot), at(a, hr + hk, ybot), 2)).collect();
        let ring_b: Vec<Vec<DVec3>> = sq.iter().zip(&ang).map(|(p, &a)| vec![at(a, hr + hk, ybot), DVec3::new(p.x, ybot, p.z)]).collect();
        strip_quads(m, &ring_t, true, 1);
        strip_quads(m, &fil_t, true, 2);
        strip_quads(m, &wall, true, 1);
        strip_quads(m, &fil_b, true, 2);
        strip_quads(m, &ring_b, true, 1);
    };

    // ---- 2. fill paths (cream) -----------------------------------------
    // bearing body: between the two rims and the fillet's top lines
    let rim = &front.rim;
    let no = rim.len() - 1;
    let kz = ns + 1;
    let body: Vec<Vec<DVec3>> = (0..=no)
        .map(|i| {
            (0..=kz)
                .map(|k| {
                    if k == 0 {
                        rim[i]
                    } else if k == kz {
                        mirror(rim[i])
                    } else if i == 0 {
                        left[k - 1]
                    } else if i == no {
                        right[ns - k]
                    } else {
                        DVec3::new(rim[i].x, rim[i].y, left[k - 1].z)
                    }
                })
                .collect()
        })
        .collect();
    strip_quads(&mut m, &body, false, 1);
    // bore wall
    let bore = &front.bore;
    let zb = bore[0].z;
    let nbz = ((2.0 * zb / size).round() as usize).max(1);
    let wall: Vec<Vec<DVec3>> = bore
        .iter()
        .map(|&p| (0..=nbz).map(|k| if k == 0 { p } else if k == nbz { mirror(p) } else { DVec3::new(p.x, p.y, zb - 2.0 * zb * k as f64 / nbz as f64) }).collect())
        .collect();
    strip_quads(&mut m, &wall, true, 1);

    // base top, right and left of the bearing: rows from the fillet's base
    // edge, columns straight, a square of kq x kq cells around each hole
    let inner_r: Vec<DVec3> = [back0 + nf - 3, back0 + nf - 2, back0 + nf - 1].into_iter().chain(right0..right0 + ns).chain([0, 1, 2]).map(qb).collect();
    let inner_l: Vec<DVec3> = [back0 + 2, back0 + 1, back0].into_iter().chain((nf..nf + ns).rev()).chain([nf - 1, nf - 2, nf - 3]).map(qb).collect();
    let jseg = inner_r.len() - 1;
    let mid = jseg - 2 * (kq + 1);
    let mut zn = vec![-zt];
    run(&mut zn, -hzc - hs, 1);
    run(&mut zn, -hzc + hs, kq);
    run(&mut zn, hzc - hs, mid);
    run(&mut zn, hzc + hs, kq);
    run(&mut zn, zt, 1);
    let x_in = qb(right0 + ns / 2).x;
    let blend = 4;
    let mut xs = vec![x_in];
    run(&mut xs, hxc - hs, blend);
    run(&mut xs, hxc + hs, kq);
    run(&mut xs, xt, 1);
    xs.remove(0);
    let xs_l: Vec<f64> = xs.iter().map(|x| -x).collect();
    let nx = xs.len();
    let blocks = [(blend, blend + kq, 1, 1 + kq), (blend, blend + kq, jseg - 1 - kq, jseg - 1)];
    let g_r = region(&mut m, &inner_r, &xs, &zn, ytop, blend, &blocks);
    let g_l = region(&mut m, &inner_l, &xs_l, &zn, ytop, blend, &blocks);
    let center = |sx: f64, sz: f64| *centers.iter().find(|c| c.x.signum() == sx && c.z.signum() == sz).unwrap();
    for (g, sx) in [(&g_r, 1.0), (&g_l, -1.0)] {
        hole_at(&mut m, center(sx, -1.0), &block_edge(g, blocks[0]));
        hole_at(&mut m, center(sx, 1.0), &block_edge(g, blocks[1]));
    }

    // base rounds: a column = top vertex, middle, bottom vertex around the
    // round's axis; where the bearing fillet sits on the round, the column
    // starts at the fillet's base edge
    let round_col = |p: DVec3, c: DVec3, u: DVec3, v: DVec3| -> Vec<DVec3> {
        let d = p - c;
        let a0 = d.dot(v).atan2(d.dot(u));
        let am = 0.5 * (a0 + FRAC_PI_2);
        vec![p, c + (u * am.cos() + v * am.sin()) * rr, c + v * rr]
    };
    let top_edge = |sz: f64| -> Vec<DVec3> {
        let jj = if sz > 0.0 { jseg } else { 0 };
        let (zone, cl, cr): (Vec<usize>, usize, usize) =
            if sz > 0.0 { ((3..=nf - 4).rev().collect(), nf - 3, 2) } else { ((back0 + 3..=back0 + nf - 4).collect(), back0 + 2, back0 + nf - 3) };
        let mut v: Vec<DVec3> = (1..=nx).rev().map(|i| g_l[i][jj]).collect();
        v.push(qb(cl));
        v.extend(zone.into_iter().map(qb));
        v.push(qb(cr));
        v.extend((1..=nx).map(|i| g_r[i][jj]));
        v
    };
    let ny = (((yhi - ylo) / size).round() as usize).max(1);
    let ys = lin(yhi, ylo, ny);
    let mut xf_all = vec![];
    for sz in [1.0, -1.0] {
        let edge = top_edge(sz);
        let tz = DVec3::Z * sz;
        let strip: Vec<Vec<DVec3>> = edge.iter().map(|&p| round_col(p, DVec3::new(p.x, yhi, zt * sz), DVec3::Y, tz)).collect();
        strip_quads(&mut m, &strip, false, 2);
        let xf: Vec<f64> = strip.iter().map(|c| c[2].x).collect();
        // front / back face of the base
        let face: Vec<Vec<DVec3>> = xf.iter().map(|&x| ys.iter().map(|&y| DVec3::new(x, y, (zt + rr) * sz)).collect()).collect();
        strip_quads(&mut m, &face, false, 1);
        // bottom round along x
        let low: Vec<Vec<DVec3>> = xf.iter().map(|&x| round_col(DVec3::new(x, ylo, (zt + rr) * sz), DVec3::new(x, ylo, zt * sz), tz, -DVec3::Y)).collect();
        strip_quads(&mut m, &low, false, 2);
        xf_all.push(xf);
    }
    for sx in [1.0, -1.0] {
        let tx = DVec3::X * sx;
        let g = if sx > 0.0 { &g_r } else { &g_l };
        let strip: Vec<Vec<DVec3>> = (0..=jseg).map(|jj| round_col(g[nx][jj], DVec3::new(xt * sx, yhi, zn[jj]), DVec3::Y, tx)).collect();
        strip_quads(&mut m, &strip, false, 2);
        let face: Vec<Vec<DVec3>> = zn.iter().map(|&z| ys.iter().map(|&y| DVec3::new((xt + rr) * sx, y, z)).collect()).collect();
        strip_quads(&mut m, &face, false, 1);
        let low: Vec<Vec<DVec3>> = zn.iter().map(|&z| round_col(DVec3::new((xt + rr) * sx, ylo, z), DVec3::new(xt * sx, ylo, z), tx, -DVec3::Y)).collect();
        strip_quads(&mut m, &low, false, 2);
        for sz in [1.0, -1.0] {
            let tz = DVec3::Z * sz;
            // vertical round
            let vert: Vec<Vec<DVec3>> = ys.iter().map(|&y| round_col(DVec3::new(xt * sx, y, (zt + rr) * sz), DVec3::new(xt * sx, y, zt * sz), tz, tx)).collect();
            strip_quads(&mut m, &vert, false, 2);
            // corners: three quads on the ball
            for (yc, ty) in [(yhi, DVec3::Y), (ylo, -DVec3::Y)] {
                let c = DVec3::new(xt * sx, yc, zt * sz);
                let h = std::f64::consts::FRAC_1_SQRT_2;
                let (py, pz, px) = (c + ty * rr, c + tz * rr, c + tx * rr);
                let (myz, myx, mzx) = (c + (ty + tz) * h * rr, c + (ty + tx) * h * rr, c + (tz + tx) * h * rr);
                let ctr = c + (tx + ty + tz).normalize() * rr;
                m.quad([py, myz, ctr, myx], 2);
                m.quad([pz, mzx, ctr, myz], 2);
                m.quad([px, myx, ctr, mzx], 2);
            }
        }
    }
    // base bottom: same columns as the front and back faces, same rows as
    // the sides, the same squares around the holes
    let (xf, xb) = (&xf_all[0], &xf_all[1]);
    let gb: Vec<Vec<DVec3>> = (0..xf.len())
        .map(|i| {
            zn.iter()
                .map(|&z| {
                    let t = (zt - z) / (2.0 * zt);
                    let x = if xf[i] == xb[i] { xf[i] } else { xf[i] + (xb[i] - xf[i]) * t };
                    DVec3::new(x, ybot, z)
                })
                .collect()
        })
        .collect();
    let col_of = |x: f64| xf.iter().position(|v| (v - x).abs() < 1e-9).expect("colonne du carré");
    let mut bblocks = vec![];
    for sx in [1.0, -1.0] {
        let (a, b) = (col_of(sx * (hxc - hs)), col_of(sx * (hxc + hs)));
        let (i0, i1) = (a.min(b), a.max(b));
        bblocks.push((i0, i1, 1, 1 + kq));
        bblocks.push((i0, i1, jseg - 1 - kq, jseg - 1));
    }
    grid_quads(&mut m, &gb, &bblocks);

    let (b, _, _, _) = weld_baked(m, &full);
    (full, b)
}

/// Open and non-manifold edge counts, largest vertex distance to the surface.
fn mesh_check(b: &bake::Baked, full: &Field) -> (usize, usize, f64) {
    let edges = edge_counts(b);
    let open = edges.values().filter(|&&c| c == 1).count();
    let nonman = edges.values().filter(|&&c| c > 2).count();
    let dev = b.positions.iter().map(|p| full.dist(*p).abs()).fold(0.0, f64::max);
    (open, nonman, dev)
}

fn piece() {
    use glam::DVec3;
    let out = std::env::var("OUT").unwrap_or_else(|_| "/tmp".into());
    let (full, b) = piece_mesh();
    let (open, nonman, dev) = mesh_check(&b, &full);
    let fil = b.faces.iter().filter(|f| f.patch == 2).count();
    println!(
        "pièce : {} quads (fillets {fil}, remplissage {}), {} sommets, bords libres {open}, non-manifold {nonman}, écart max {dev:.3} mm",
        b.faces.len(),
        b.faces.len() - fil,
        b.positions.len()
    );
    let none: Vec<(&Vec<DVec3>, [u8; 3], bool, bool)> = vec![];
    let views = [
        ("dessus", DVec3::new(0.75, 0.6, 1.0), DVec3::new(0.0, 8.0, 0.0), 62.0),
        ("dessous", DVec3::new(0.6, -0.7, 0.8), DVec3::new(0.0, 3.0, 0.0), 62.0),
        ("arriere", DVec3::new(-0.7, 0.5, -1.0), DVec3::new(0.0, 9.0, 0.0), 62.0),
        ("avant", DVec3::new(0.35, 0.35, 1.0), DVec3::new(0.0, 12.0, 15.0), 26.0),
        ("trou", DVec3::new(0.45, 1.0, 0.6), DVec3::new(18.0, 10.0, 10.0), 14.0),
        ("coin", DVec3::new(0.9, 0.6, 1.0), DVec3::new(10.0, 10.0, 14.5), 10.0),
    ];
    for (v, dir, target, ext) in views {
        render_lines(None, Some(&b), &none, &format!("{out}/piece-{v}.png"), dir, target, ext, 1400, 1050);
    }
}

/// Largest and mean distance to the surface at the faces' centres and edge
/// middles: what the flat quads lose.
fn face_dev(b: &bake::Baked, full: &Field) -> (f64, f64) {
    let mut v = vec![];
    for f in &b.faces {
        let p: Vec<glam::DVec3> = f.verts.iter().map(|i| b.positions[*i as usize]).collect();
        v.push(full.dist((p[0] + p[1] + p[2] + p[3]) * 0.25).abs());
        for k in 0..4 {
            v.push(full.dist((p[k] + p[(k + 1) % 4]) * 0.5).abs());
        }
    }
    (v.iter().cloned().fold(0.0, f64::max), v.iter().sum::<f64>() / v.len() as f64)
}

/// Romain's case: a cube with a cylinder carved at one corner, no fillet.
pub fn coin(blend: f32) -> Scene {
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::new(20.0, 20.0, 20.0);
    s.add(root, NodeSpec::shape("Cube", b).at(Vec3::new(0.0, 20.0, 0.0)));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(9.0, 22.0, 0.0);
    s.add(root, NodeSpec::shape("Cylindre", c).at(Vec3::new(20.0, 20.0, 20.0)).mode(CombineMode::Carve).blend(blend));
    s
}

/// Construction lines read from the history (bake::lines), over the part.
fn construction() {
    use glam::DVec3;
    let out = std::env::var("OUT").unwrap_or_else(|_| "/tmp".into());
    let romain = Scene::from_json(include_str!("scenes/romain-coin.forme")).expect("scène de Romain");
    let front = DVec3::new(0.8, 0.7, 1.0);
    let cases = [
        ("romain", romain, DVec3::new(39.5, 19.4, 51.5), 80.0, DVec3::new(-0.8, 0.7, -1.0)),
        ("coin", coin(0.0), DVec3::new(0.0, 20.0, 0.0), 70.0, front),
        ("coin-fillet", coin(1.5), DVec3::new(0.0, 20.0, 0.0), 70.0, front),
        ("support", support(4), DVec3::new(0.0, 10.0, 0.0), 64.0, front),
    ];
    for (name, scene, target, ext, eye) in cases {
        let l = bake::lines::construction_lines(&scene);
        println!(
            "{name} : {} lignes ({} d'arêtes, {} de jonctions, {} rails) en {:.0} ms",
            l.lines.len(),
            l.count(false),
            l.count(true),
            l.lines.iter().filter(|x| x.kind.is_rail()).count(),
            l.seconds * 1000.0
        );
        for x in &l.lines {
            println!("  {:?} {} · {} points · r {:.2}", x.kind, x.label, x.pts.len(), x.radius);
        }
        let part = bake::bake_blocking(&scene, bake::Quality::Standard);
        let lines: Vec<(&Vec<DVec3>, [u8; 3], bool, bool)> =
            l.lines.iter().map(|x| (&x.pts, if x.kind.is_rail() { [255, 140, 40] } else { [250, 215, 70] }, false, false)).collect();
        render_lines(Some(&part), None, &lines, &format!("{out}/construction-{name}.png"), eye, target, ext, 1400, 1050);
    }
}

fn decime() {
    use glam::DVec3;
    let out = std::env::var("OUT").unwrap_or_else(|_| "/tmp".into());
    let deg: f64 = std::env::var("DEG").ok().and_then(|v| v.parse().ok()).unwrap_or(5.0);
    let (full, b) = piece_mesh();
    let t0 = std::time::Instant::now();
    let (d, loops) = bake::decimate::dissolve(&full, &b, deg);
    let (open, nonman, _) = mesh_check(&d, &full);
    let quads = d.faces.iter().filter(|f| f.verts.len() == 4).count();
    let (m0, a0) = face_dev(&b, &full);
    let (m1, a1) = face_dev(&d, &full);
    println!(
        "seuil {deg}° : {} loops retirées en {:.1} s, {} → {} faces ({} quads), {} → {} sommets, bords libres {open}, non-manifold {nonman}, écart au milieu des faces {m0:.3} / {a0:.4} → {m1:.3} / {a1:.4} mm (max / moyen)",
        loops.len(),
        t0.elapsed().as_secs_f64(),
        b.faces.len(),
        d.faces.len(),
        quads,
        b.positions.len(),
        d.positions.len()
    );
    let lines: Vec<(&Vec<DVec3>, [u8; 3], bool, bool)> = loops.iter().map(|l| (l, [220, 40, 60], false, true)).collect();
    let none: Vec<(&Vec<DVec3>, [u8; 3], bool, bool)> = vec![];
    let views = [
        ("dessus", DVec3::new(0.75, 0.6, 1.0), DVec3::new(0.0, 8.0, 0.0), 62.0),
        ("dessous", DVec3::new(0.6, -0.7, 0.8), DVec3::new(0.0, 3.0, 0.0), 62.0),
        ("avant", DVec3::new(0.35, 0.35, 1.0), DVec3::new(0.0, 12.0, 15.0), 26.0),
    ];
    for (v, dir, target, ext) in views {
        render_lines(None, Some(&b), &lines, &format!("{out}/decime-loops-{v}.png"), dir, target, ext, 1400, 1050);
        render_lines(None, Some(&d), &none, &format!("{out}/decime-{v}.png"), dir, target, ext, 1400, 1050);
    }
}

/// Shaded part without wire, the fillet strip with its wire, and curves on top.
#[allow(clippy::too_many_arguments)]
pub fn render_lines(part: Option<&bake::Baked>, strip: Option<&bake::Baked>, lines: &[(&Vec<glam::DVec3>, [u8; 3], bool, bool)], path: &str, eye_dir: glam::DVec3, target: glam::DVec3, extent: f64, w: u32, h: u32) {
    use glam::DVec3;
    let fwd = (-eye_dir).normalize();
    let right = fwd.cross(DVec3::Y).normalize();
    let up = right.cross(fwd);
    let scale = w.min(h) as f64 / extent;
    let proj = |p: DVec3| -> (f64, f64, f64) {
        let d = p - target;
        (w as f64 * 0.5 + d.dot(right) * scale, h as f64 * 0.5 - d.dot(up) * scale, d.dot(fwd))
    };
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([24, 26, 30]));
    let mut zb = vec![f64::MAX; (w * h) as usize];
    let light = DVec3::new(-0.4, 0.8, 0.5).normalize();
    let tri = |pts: &[DVec3], n: DVec3, col: [f64; 3], bias: f64, img: &mut image::RgbImage, zb: &mut Vec<f64>| {
        let shade = (n.dot(light).abs() * 0.7 + 0.3).min(1.0);
        let rgb = image::Rgb([(col[0] * shade * 255.0) as u8, (col[1] * shade * 255.0) as u8, (col[2] * shade * 255.0) as u8]);
        for k in 1..pts.len() - 1 {
            let t = [proj(pts[0]), proj(pts[k]), proj(pts[k + 1])];
            let area = (t[1].0 - t[0].0) * (t[2].1 - t[0].1) - (t[2].0 - t[0].0) * (t[1].1 - t[0].1);
            if area.abs() < 1e-12 {
                continue;
            }
            let minx = t.iter().map(|p| p.0).fold(f64::MAX, f64::min).floor().max(0.0) as i64;
            let maxx = t.iter().map(|p| p.0).fold(f64::MIN, f64::max).ceil().min(w as f64 - 1.0) as i64;
            let miny = t.iter().map(|p| p.1).fold(f64::MAX, f64::min).floor().max(0.0) as i64;
            let maxy = t.iter().map(|p| p.1).fold(f64::MIN, f64::max).ceil().min(h as f64 - 1.0) as i64;
            for y in miny..=maxy {
                for x in minx..=maxx {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    let w0 = ((t[1].0 - px) * (t[2].1 - py) - (t[2].0 - px) * (t[1].1 - py)) / area;
                    let w1 = ((t[2].0 - px) * (t[0].1 - py) - (t[0].0 - px) * (t[2].1 - py)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * t[0].2 + w1 * t[1].2 + w2 * t[2].2 + bias;
                    let i = (y as u32 * w + x as u32) as usize;
                    if z < zb[i] {
                        zb[i] = z;
                        img.put_pixel(x as u32, y as u32, rgb);
                    }
                }
            }
        }
    };
    let normal = |_b: &bake::Baked, f: &bake::BFace| -> DVec3 { f.normals.iter().fold(DVec3::ZERO, |s, n| s + *n).normalize_or_zero() };
    if let Some(b) = part {
        for f in &b.faces {
            let pts: Vec<DVec3> = f.verts.iter().map(|v| b.positions[*v as usize]).collect();
            // pushed back a little: the shaded part is only an approximation of the
            // surface and must not poke through the exact strips
            tri(&pts, normal(b, f), [0.6, 0.61, 0.6], 0.35, &mut img, &mut zb);
        }
    }
    if let Some(b) = strip {
        // slightly lifted so it wins over the part where they coincide
        let lift = 0.15;
        for f in &b.faces {
            let n = normal(b, f);
            let pts: Vec<DVec3> = f.verts.iter().enumerate().map(|(k, v)| b.positions[*v as usize] + f.normals.get(k).copied().unwrap_or(n) * lift).collect();
            let col = match f.patch {
                1 => [0.88, 0.82, 0.66],
                3 => [0.58, 0.82, 0.52],
                _ => [0.95, 0.62, 0.32],
            };
            tri(&pts, n, col, 0.0, &mut img, &mut zb);
        }
    }
    let mut dash = 0.0f64;
    let mut line = |a: DVec3, c: DVec3, col: image::Rgb<u8>, thick: i64, tol: f64, img: &mut image::RgbImage, zb: &[f64]| {
        let (pa, pc) = (proj(a), proj(c));
        let steps = ((pc.0 - pa.0).abs().max((pc.1 - pa.1).abs()).ceil() as usize).max(1);
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let (x, y, z) = (pa.0 + (pc.0 - pa.0) * t, pa.1 + (pc.1 - pa.1) * t, pa.2 + (pc.2 - pa.2) * t);
            if tol.is_infinite() {
                // dashed: 10 px on, 7 px off
                dash += 1.0;
                if dash % 17.0 >= 10.0 {
                    continue;
                }
            }
            for dx in -thick..=thick {
                for dy in -thick..=thick {
                    let (xi, yi) = (x as i64 + dx, y as i64 + dy);
                    if xi < 0 || yi < 0 || xi >= w as i64 || yi >= h as i64 {
                        continue;
                    }
                    let i = (yi as u32 * w + xi as u32) as usize;
                    if z <= zb[i] + tol {
                        img.put_pixel(xi as u32, yi as u32, col);
                    }
                }
            }
        }
    };
    if let Some(b) = strip {
        let mut seen = std::collections::HashSet::new();
        for f in &b.faces {
            let n = normal(b, f);
            let at = |k: usize| b.positions[f.verts[k] as usize] + f.normals.get(k).copied().unwrap_or(n) * 0.16;
            for i in 0..f.verts.len() {
                let (a, c) = (f.verts[i], f.verts[(i + 1) % f.verts.len()]);
                if seen.insert((a.min(c), a.max(c))) {
                    line(at(i), at((i + 1) % f.verts.len()), image::Rgb([40, 22, 10]), 0, 0.05, &mut img, &zb);
                }
            }
        }
    }
    // curves: drawn through the surface when they are just under it (the
    // corner sits under the fillet)
    let px = extent / w.min(h) as f64;
    for (c, col, xray, closed) in lines {
        let tol = if *xray { f64::INFINITY } else { 1.2 + 3.0 * px };
        let n = if *closed { c.len() } else { c.len().saturating_sub(1) };
        for i in 0..n {
            line(c[i], c[(i + 1) % c.len()], image::Rgb(*col), 1, tol, &mut img, &zb);
        }
    }
    img.save(path).unwrap();
}

#[allow(dead_code)]
pub fn check_cuts(scene: &Scene, q: Quality) {
    use bake::patch::{self, PatchSettings, Term};
    let st = bake::BakeSettings::preset(q);
    let field = Field::new(&compile(scene));
    let gs = gen::GenSettings { tol: st.tol, max_edge: st.max_edge, segs: 0 };
    let ps = PatchSettings { trim_len: st.trim_len, min_size: st.trim_len / 8.0, max_level: 9 };
    let term_val = |k: &bake::field::TermKey, x: glam::DVec3| -> f64 {
        let pr = &field.prims[k.prim as usize];
        let inst = field.instances(k.prim).into_iter().find(|i| i.copy == k.copy).unwrap();
        bake::field::prim_raw(pr, inst.to_local.transform_point3(x)) * pr.sc - k.level as f64 * 1e-5
    };
    for (prim, level) in field.term_levels() {
        let pr = &field.prims[prim as usize];
        let base = gen::generate(pr, level / pr.sc, &gs);
        for inst in field.instances(prim) {
            let key = bake::field::Owner { prim, copy: inst.copy, level }.key();
            let term = Term { key, inst, level_local: level / pr.sc };
            let o = patch::process(&field, &term, &base, &[], &[], &ps);
            let mut bad = 0;
            let mut none = 0;
            let mut worst = (0.0f64, glam::DVec3::ZERO, None);
            for (x, other) in o.pos.iter().zip(&o.other) {
                if other.map(|k| k.prim == u32::MAX).unwrap_or(false) {
                    none += 1;
                    if none < 4 {
                        println!("   none-label cut at {:?} f {:.2e}", x, field.dist(*x));
                    }
                    continue;
                }
                if let Some(k) = other {
                    let own = term_val(&key, *x).abs();
                    let oth = term_val(k, *x).abs();
                    let e = own.max(oth);
                    if e > 1e-3 {
                        bad += 1;
                    }
                    if e > worst.0 {
                        worst = (e, *x, Some((k.prim, k.level)));
                    }
                }
            }
            println!("term {prim} lvl {level:.2} copy {}: faces {} cuts {} none {none} bad {bad} worst {:.4} at {:?} other {:?}", inst.copy, o.faces.len(), o.cuts, worst.0, worst.1, worst.2);
        }
    }
}

#[allow(dead_code)]
pub fn dump_near(scene: &Scene, prim: u32, level: f64, at: glam::DVec3, r: f64) {
    use bake::patch::{self, PatchSettings, Term};
    let st = bake::BakeSettings::preset(Quality::Standard);
    let field = Field::new(&compile(scene));
    let gs = gen::GenSettings { tol: st.tol, max_edge: st.max_edge, segs: 0 };
    let ps = PatchSettings { trim_len: st.trim_len, min_size: st.trim_len / 8.0, max_level: 9 };
    let pr = &field.prims[prim as usize];
    let base = gen::generate(pr, level / pr.sc, &gs);
    let inst = field.instances(prim)[0];
    let key = bake::field::Owner { prim, copy: inst.copy, level }.key();
    let term = Term { key, inst, level_local: level / pr.sc };
    let o = patch::process(&field, &term, &base, &[], &[], &ps);
    for (fi, f) in o.faces.iter().enumerate() {
        if f.iter().any(|v| o.pos[*v as usize].distance(at) < r) {
            println!("face {fi}:");
            for v in f {
                println!("   v{v} {:?} {}", o.pos[*v as usize], if o.other[*v as usize].is_some() { "cut" } else { "" });
            }
        }
    }
}

/// Cube bevel radius (env ROUND, 1 mm by default: "plus sharp").
pub fn round() -> f32 {
    std::env::var("ROUND").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0)
}

/// Test cube: 20 mm, rounded edges (bevel) ROUND mm, through hole Ø8 with a 0.8 mm rim fillet.
pub fn cube_hole() -> Scene {
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::splat(10.0);
    b.round = round();
    s.add(root, NodeSpec::shape("cube", b));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(4.0, 14.0, 0.0);
    s.add(root, NodeSpec::shape("trou", c).mode(CombineMode::Carve).blend(0.8));
    s
}

/// Like the user's test: a box with four slotted screws from the kit on its
/// front face (hole carved, head added, slot carved; transparent groups).
pub fn screws_scene() -> Scene {
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::splat(10.0);
    b.round = 0.0;
    s.add(root, NodeSpec::shape("Boîte 1", b));
    let cyl = |r: f32, hh: f32, round: f32| {
        let mut c = Shape::new(PrimKind::Cylinder);
        c.size = Vec3::new(r, hh, 0.0);
        c.round = round;
        c
    };
    for (i, at) in [Vec3::new(-4.0, 3.0, 10.0), Vec3::new(4.0, 4.0, 10.0), Vec3::new(3.0, -3.5, 10.0), Vec3::new(-3.5, -4.5, 10.0)].iter().enumerate() {
        let sc = if i == 0 { 1.0 } else { 0.55 };
        let mut spec = NodeSpec::group("Vis", false).at(*at).rot(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
        spec.transform.scale = sc;
        let g = s.add(root, spec);
        s.add(g, NodeSpec::shape("Logement", cyl(3.4, 1.2, 0.0)).mode(CombineMode::Carve).blend(0.3));
        s.add(g, NodeSpec::shape("Tête", cyl(2.9, 0.9, 0.7)).at(Vec3::new(0.0, -0.3, 0.0)));
        let mut slot = Shape::new(PrimKind::Box);
        slot.size = Vec3::new(2.4, 1.0, 0.4);
        s.add(g, NodeSpec::shape("Fente", slot).at(Vec3::new(0.0, 0.7, 0.0)).rot(Quat::from_rotation_y(0.6)).mode(CombineMode::Carve));
    }
    s
}

/// The PC test part (pillow block), first `n` operations: base, bearing
/// (lying cylinder on the base), bore, mirrored fixing holes.
pub fn support(n: usize) -> Scene {
    let mut s = Scene::new();
    let root = s.root;
    let lying = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::new(26.0, 5.0, 16.0);
    b.round = 1.5;
    s.add(root, NodeSpec::shape("Socle", b).at(Vec3::new(0.0, 5.0, 0.0)));
    if n > 1 {
        let mut c = Shape::new(PrimKind::Cylinder);
        c.size = Vec3::new(11.0, 14.0, 0.0);
        c.round = 1.0;
        s.add(root, NodeSpec::shape("Palier", c).at(Vec3::new(0.0, 19.0, 0.0)).rot(lying).blend(2.0));
    }
    if n > 2 {
        let mut c = Shape::new(PrimKind::Cylinder);
        c.size = Vec3::new(6.0, 20.0, 0.0);
        s.add(root, NodeSpec::shape("Alésage", c).at(Vec3::new(0.0, 19.0, 0.0)).rot(lying).mode(CombineMode::Carve).blend(0.6));
    }
    if n > 3 {
        let mut c = Shape::new(PrimKind::Cylinder);
        c.size = Vec3::new(2.6, 8.0, 0.0);
        s.add(
            root,
            NodeSpec::shape("Trous de fixation", c)
                .at(Vec3::new(19.0, 5.0, 10.0))
                .mode(CombineMode::Carve)
                .blend(0.4)
                .mirror(scene::Mirror { x: true, y: false, z: true }),
        );
    }
    s
}

/// The same base with a standing cylinder: the case the bake already handles.
pub fn support_boss() -> Scene {
    let mut s = support(1);
    let root = s.root;
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(8.0, 6.0, 0.0);
    c.round = 1.0;
    s.add(root, NodeSpec::shape("Bossage", c).at(Vec3::new(0.0, 14.0, 0.0)).blend(2.0));
    s
}

pub fn variant(at: Vec3, rot: Quat, size: Vec3, mode: CombineMode, blend: f32) -> Scene {
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::splat(10.0);
    b.round = round();
    s.add(root, NodeSpec::shape("cube", b));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = size;
    s.add(root, NodeSpec::shape("outil", c).at(at).rot(rot).mode(mode).blend(blend));
    s
}

/// Hand-built target topology for `cube_hole`, laid on the exact surface.
pub fn target_mesh() -> bake::Baked {
    use glam::DVec3;
    let scene = cube_hole();
    let field = Field::new(&compile(&scene));
    // the rounded cube: 8 flat segments per face, 4 bevel rows per edge
    let pr = &field.prims[0];
    let a = 10.0 - round() as f64;
    let gs = gen::GenSettings { tol: 0.03, max_edge: 2.0 * a / 8.0, segs: 0 };
    let cube = gen::generate(pr, 0.0, &gs);
    let mut pos: Vec<DVec3> = cube.pos.clone();
    let on_cap = |p: DVec3, y: f64| (p.y - y).abs() < 1e-6 && p.x.abs() <= a + 1e-6 && p.z.abs() <= a + 1e-6;
    let mut faces: Vec<Vec<u32>> = cube.faces.iter().filter(|f| !(f.iter().all(|v| on_cap(pos[*v as usize], 10.0)) || f.iter().all(|v| on_cap(pos[*v as usize], -10.0)))).cloned().collect();
    let segs = 32usize;
    let add = |pos: &mut Vec<DVec3>, p: DVec3| -> u32 {
        pos.push(p);
        pos.len() as u32 - 1
    };
    for &(y, sgn) in &[(10.0f64, 1.0f64), (-10.0, -1.0)] {
        // square border of the flat cap, 8 segments per side, counter-clockwise from +x
        let find = |pos: &Vec<DVec3>, x: f64, z: f64| -> u32 {
            pos.iter().position(|p| (p.x - x).abs() < 1e-6 && (p.z - z).abs() < 1e-6 && (p.y - y).abs() < 1e-6).unwrap() as u32
        };
        let mut square = vec![];
        for j in 0..segs {
            let ang = std::f64::consts::TAU * j as f64 / segs as f64;
            // point of the square border at the same index (equal steps along the sides)
            let side = (j + 4) / 8 % 4;
            let k = ((j + 4) % 8) as f64; // 0..8 along the side
            let t = -a + 2.0 * a * k / 8.0;
            let (x, z) = match side {
                0 => (a, t),
                1 => (-t, a),
                2 => (-a, -t),
                _ => (t, -a),
            };
            let _ = ang;
            square.push(find(&pos, x, z));
        }
        let rings_r = [0.75, 0.5, 0.25];
        let circle = |r: f64, yy: f64, j: usize| {
            let ang = std::f64::consts::TAU * j as f64 / segs as f64;
            DVec3::new(r * ang.cos(), yy, r * ang.sin())
        };
        let mut rings: Vec<Vec<u32>> = vec![square.clone()];
        for &w in &rings_r {
            let ring: Vec<u32> = (0..segs)
                .map(|j| {
                    let sq = pos[square[j] as usize];
                    let ci = circle(4.8, y, j);
                    add(&mut pos, ci.lerp(sq, w))
                })
                .collect();
            rings.push(ring);
        }
        // fillet: rail on the cap, middle, rail on the wall
        rings.push((0..segs).map(|j| add(&mut pos, circle(4.8, y, j))).collect());
        rings.push((0..segs).map(|j| add(&mut pos, circle(4.0, y, j))).collect());
        rings.push((0..segs).map(|j| add(&mut pos, circle(4.0, y - 0.8 * sgn, j))).collect());
        if sgn > 0.0 {
            // wall rows down to the bottom rail (shared with the bottom cap)
            for r in 1..9 {
                let yy = 9.2 - 18.4 * r as f64 / 9.0;
                rings.push((0..segs).map(|j| add(&mut pos, circle(4.0, yy, j))).collect());
            }
        }
        for w in rings.windows(2) {
            for j in 0..segs {
                let j2 = (j + 1) % segs;
                let f = vec![w[0][j], w[0][j2], w[1][j2], w[1][j]];
                faces.push(f);
            }
        }
        if sgn > 0.0 {
            // remember the last wall row to join it to the bottom
            let last = rings.last().unwrap().clone();
            faces.push(vec![u32::MAX]); // marker
            faces.push(last);
        }
    }
    // join the top wall's last row to the bottom wall rail
    let mi = faces.iter().position(|f| f == &vec![u32::MAX]).unwrap();
    let last = faces.remove(mi + 1);
    faces.remove(mi);
    let bottom_rail: Vec<u32> = (0..segs)
        .map(|j| {
            let ang = std::f64::consts::TAU * j as f64 / segs as f64;
            let target = DVec3::new(4.0 * ang.cos(), -9.2, 4.0 * ang.sin());
            pos.iter().position(|p| p.distance(target) < 1e-9).unwrap() as u32
        })
        .collect();
    for j in 0..segs {
        let j2 = (j + 1) % segs;
        faces.push(vec![last[j], last[j2], bottom_rail[j2], bottom_rail[j]]);
    }
    for p in pos.iter_mut() {
        if let Some(q) = field.project(*p) {
            *p = q;
        }
    }
    // outward orientation, exact normals
    let mut b = bake::Baked::default();
    for mut f in faces {
        let mut n = DVec3::ZERO;
        let mut c = DVec3::ZERO;
        for k in 0..f.len() {
            n += pos[f[k] as usize].cross(pos[f[(k + 1) % f.len()] as usize]);
            c += pos[f[k] as usize];
        }
        if n.dot(field.grad(c / f.len() as f64)) < 0.0 {
            f.reverse();
        }
        let normals = f.iter().map(|v| field.grad(pos[*v as usize]).normalize_or_zero()).collect();
        b.faces.push(bake::BFace { verts: f, normals, mat: 0, patch: 0 });
    }
    b.positions = pos;
    b
}
