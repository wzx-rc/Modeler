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

fn report(name: &str, scene: &Scene, q: Quality) {
    let b = bake::bake_blocking(scene, q);
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
    let gs = gen::GenSettings { tol: 0.01, max_edge: 4.0 };
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
            report("grenade std", &demo::grenade(), Quality::Standard);
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
    let gs = gen::GenSettings { tol: st.tol, max_edge: st.max_edge };
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
        let col = if std::env::var("BY_PATCH").is_ok() {
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

#[allow(dead_code)]
pub fn check_cuts(scene: &Scene, q: Quality) {
    use bake::patch::{self, PatchSettings, Term};
    let st = bake::BakeSettings::preset(q);
    let field = Field::new(&compile(scene));
    let gs = gen::GenSettings { tol: st.tol, max_edge: st.max_edge };
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
    let gs = gen::GenSettings { tol: st.tol, max_edge: st.max_edge };
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
