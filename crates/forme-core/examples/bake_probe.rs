//! Bake by construction lines (`bake::layout`): test scenes, CPU renders.
//!
//! cargo run --release -p forme-core --example bake_probe -- parlignes   (CASE=name for one, OUT=dir)
//! cargo run --release -p forme-core --example bake_probe -- lignes      (construction lines over the part)
//! cargo run --release -p forme-core --example bake_probe -- decime      (optional flat-loop pass, DEG=5)
use forme_core::bake::{self, field::Field};
use forme_core::glam::Vec3;
use forme_core::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()).unwrap_or("parlignes") {
        "parlignes" => par_lignes(),
        "lignes" => construction(),
        "decime" => decime(),
        other => eprintln!("cas inconnu : {other} (parlignes, lignes, decime)"),
    }
}

/// CPU render of a baked mesh: shaded polygons, polygon edges, open edges in red.
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
            // roles: fill paths blue, fillet strips orange
            if f.patch == bake::P_FILLET {
                [0.95, 0.6, 0.3]
            } else {
                [0.55, 0.68, 0.86]
            }
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

/// Open and non-manifold edge counts, largest vertex distance to the surface.
fn mesh_check(b: &bake::Baked, full: &Field) -> (usize, usize, f64) {
    let edges = edge_counts(b);
    let open = edges.values().filter(|&&c| c == 1).count();
    let nonman = edges.values().filter(|&&c| c > 2).count();
    let dev = b.positions.iter().map(|p| full.dist(*p).abs()).fold(0.0, f64::max);
    (open, nonman, dev)
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

/// A box standing on a box, added with a junction fillet (blend).
pub fn raccord(k: f32) -> Scene {
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::new(10.0, 5.0, 10.0);
    s.add(root, NodeSpec::shape("Socle", b).at(Vec3::new(0.0, 5.0, 0.0)));
    let mut c = Shape::new(PrimKind::Box);
    c.size = Vec3::new(2.0, 5.0, 4.0);
    s.add(root, NodeSpec::shape("Ailette", c).at(Vec3::new(0.0, 12.0, 0.0)).blend(k));
    s
}

/// A round boss standing on a block, joined by a junction blend `k` (0: sharp).
pub fn bossage(k: f32) -> Scene {
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::new(10.0, 5.0, 10.0);
    s.add(root, NodeSpec::shape("Socle", b).at(Vec3::new(0.0, 5.0, 0.0)));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(3.0, 2.5, 0.0);
    s.add(root, NodeSpec::shape("Bossage", c).at(Vec3::new(0.0, 12.0, 0.0)).blend(k));
    s
}

/// A plate with a round hole through its middle (no fillet).
pub fn trou() -> Scene {
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::new(20.0, 10.0, 20.0);
    s.add(root, NodeSpec::shape("Plaque", b).at(Vec3::new(0.0, 10.0, 0.0)));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(6.0, 14.0, 0.0);
    s.add(root, NodeSpec::shape("Trou", c).at(Vec3::new(0.0, 10.0, 0.0)).mode(CombineMode::Carve));
    s
}

/// Bake from the construction lines (bake::layout) on Romain's scene and
/// its variants: fillet strips orange, fill patches blue (BY_STEP colours).
fn par_lignes() {
    use glam::DVec3;
    let out = std::env::var("OUT").unwrap_or_else(|_| "/tmp".into());
    let romain = Scene::from_json(include_str!("scenes/romain-coin.forme")).expect("scène de Romain");
    let mut vif = romain.clone();
    for id in vif.nodes.keys().copied().collect::<Vec<_>>() {
        if let Some(sh) = vif.node_mut(id).and_then(|n| n.shape_mut()) {
            sh.edges.clear();
        }
    }
    let mut seul = Scene::new();
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::new(10.0, 10.0, 10.0);
    b.set_edge(4, 3.0);
    let r0 = seul.root;
    seul.add(r0, NodeSpec::shape("Boîte", b));
    let t = DVec3::new(39.5, 19.4, 51.5);
    let back = DVec3::new(-0.8, 0.7, -1.0);
    let front = DVec3::new(0.8, 0.7, 1.0);
    let aretes = Scene::from_json(include_str!("scenes/romain-aretes.forme")).expect("scène de Romain (fillets par arête)");
    // the same scene cut down: which part the lines bake does not handle yet
    let variante = |edges: &[usize], hole: bool, notch: bool| {
        let mut sc = aretes.clone();
        let ids: Vec<NodeId> = sc.nodes.keys().copied().collect();
        for id in ids {
            let name = sc.node(id).map(|n| n.name.clone()).unwrap_or_default();
            if name == "Cube 3" && !hole || name == "Cube 10" && !notch {
                sc.remove(id);
            } else if name == "Boîte 5" {
                if let Some(sh) = sc.node_mut(id).and_then(|n| n.shape_mut()) {
                    let all = sh.edges.clone();
                    sh.edges = vec![0.0; 12];
                    for &e in edges {
                        sh.edges[e] = all[e];
                    }
                }
            }
        }
        sc
    };
    // Romain's scene of 10 nov: notch, carved cylinder on top, small box added
    // with a junction blend; and the same without the blend / without the box
    let cyl = Scene::from_json(include_str!("scenes/romain-cylindre.forme")).expect("scène de Romain (cylindre creusé)");
    let cyl_var = |blend: bool, fin: bool| {
        let mut sc = cyl.clone();
        let ids: Vec<NodeId> = sc.nodes.keys().copied().collect();
        for id in ids {
            if sc.node(id).is_some_and(|n| n.name == "Cube 3") {
                if !fin {
                    sc.remove(id);
                } else if !blend {
                    if let Some(n) = sc.node_mut(id) {
                        n.combine.blend = 0.0;
                    }
                }
            }
        }
        sc
    };
    // his scene of 10 nov, 10:55: a cylinder standing on the top face with a
    // 2.8 mm junction blend (Cube 3 without blend, hole removed)
    let bossage_r = Scene::from_json(include_str!("scenes/romain-bossage.forme")).expect("scène de Romain (bossage)");
    let mut bossage_vif = bossage_r.clone();
    for n in bossage_vif.nodes.values_mut() {
        n.combine.blend = 0.0;
    }
    // the same, the cylinder moved (centre x, z)
    let bossage_at = |x: f32, z: f32| {
        let mut sc = bossage_r.clone();
        let ids: Vec<NodeId> = sc.nodes.keys().copied().collect();
        for id in ids {
            if let Some(n) = sc.node_mut(id).filter(|n| n.name == "Cylindre 1") {
                n.transform.pos.x = x;
                n.transform.pos.z = z;
            }
        }
        sc
    };
    // Cube 3 moved by (dx, dz): its corner nearly on the ring's square in his scene
    let cube3_at = |dx: f32, dz: f32| {
        let mut sc = bossage_r.clone();
        let ids: Vec<NodeId> = sc.nodes.keys().copied().collect();
        for id in ids {
            if let Some(n) = sc.node_mut(id).filter(|n| n.name == "Cube 3") {
                n.transform.pos.x += dx;
                n.transform.pos.z += dz;
            }
        }
        sc
    };
    // his scene of 10 nov, 16:07: the cylinder's top rim rounded 0.7 mm, its
    // 1 mm junction blend, and Cube 3 now crossing the cylinder; cut down:
    // rim (rounded or sharp), blend (on or off), Cube 3 (kept or removed)
    let arrondi = Scene::from_json(include_str!("scenes/romain-cylindre-arrondi.forme")).expect("scène de Romain (cylindre arrondi)");
    let arrondi_var = |rim: bool, blend: bool, cube3: bool| {
        let mut sc = arrondi.clone();
        let ids: Vec<NodeId> = sc.nodes.keys().copied().collect();
        for id in ids {
            let name = sc.node(id).map(|n| n.name.clone()).unwrap_or_default();
            if name == "Cube 3" && !cube3 {
                sc.remove(id);
            } else if name == "Cylindre 1" {
                if let Some(n) = sc.node_mut(id) {
                    if !blend {
                        n.combine.blend = 0.0;
                    }
                    if let Some(sh) = n.shape_mut() {
                        if !rim {
                            sh.edges.clear();
                        }
                    }
                }
            }
        }
        sc
    };
    let cases = [
        ("arrondi", arrondi_var(true, true, true), t, 80.0, true, front),
        ("arrondi-seul", arrondi_var(true, false, false), t, 80.0, true, front),
        ("arrondi-raccord", arrondi_var(true, true, false), t, 80.0, true, front),
        ("arrondi-cube3", arrondi_var(true, false, true), t, 80.0, true, front),
        ("arrondi-vif", arrondi_var(false, true, true), t, 80.0, true, front),
        ("arrondi-vif-sans-raccord", arrondi_var(false, false, true), t, 80.0, true, front),
        ("arrondi-vif-sans-cube3", arrondi_var(false, true, false), t, 80.0, true, front),
        ("bossage-c3z+1", cube3_at(0.0, 1.0), t, 80.0, true, front),
        ("bossage-c3z+2", cube3_at(0.0, 2.0), t, 80.0, true, front),
        ("bossage-c3x+1", cube3_at(1.0, 0.0), t, 80.0, true, front),
        ("bossage-c3z-1", cube3_at(0.0, -1.0), t, 80.0, true, front),
        ("bossage-48", bossage_at(40.0, 48.0), t, 80.0, true, front),
        ("bossage-46", bossage_at(38.83, 46.0), t, 80.0, true, front),
        ("bossage-50", bossage_at(36.0, 50.0), t, 80.0, true, front),
        ("bossage-44", bossage_at(42.0, 44.5), t, 80.0, true, front),
        ("boss", bossage(2.0), DVec3::new(0.0, 10.0, 0.0), 30.0, true, front),
        ("boss-vif", bossage(0.0), DVec3::new(0.0, 10.0, 0.0), 30.0, true, front),
        ("bossage", bossage_r, t, 80.0, true, front),
        ("bossage-vif", bossage_vif, t, 80.0, true, front),
        ("raccord", raccord(2.0), DVec3::new(0.0, 10.0, 0.0), 30.0, true, front),
        ("rc", cyl.clone(), t, 80.0, true, front),
        ("rc-sans-raccord", cyl_var(false, true), t, 80.0, true, front),
        ("rc-sans-cube3", cyl_var(false, false), t, 80.0, true, front),
        ("romain-aretes", aretes.clone(), t, 80.0, true, front),
        ("ra-coin3", variante(&[3, 5, 8], false, false), t, 80.0, true, front),
        ("ra-encoche9", variante(&[9], false, true), t, 80.0, true, front),
        ("ra-encoche10", variante(&[10], false, true), t, 80.0, true, front),
        ("ra-encoche3", variante(&[3], false, true), t, 80.0, true, front),
        ("ra-trou", variante(&[], true, false), t, 80.0, true, front),
        ("ra-sans-coin", variante(&[9, 10], true, true), t, 80.0, true, front),
        ("romain", romain.clone(), t, 80.0, true, back),
        ("romain-droit", romain, t, 80.0, false, back),
        ("romain-vif", vif, t, 80.0, true, back),
        ("boite-arrondie", seul, DVec3::ZERO, 32.0, true, back),
        ("coin", coin(0.0), DVec3::new(0.0, 20.0, 0.0), 70.0, true, front),
        ("trou", trou(), DVec3::new(0.0, 10.0, 0.0), 70.0, true, front),
    ];
    std::env::set_var("BY_STEP", "1");
    for (name, scene, target, ext, diagonal, eye) in cases {
        if let Ok(only) = std::env::var("CASE") {
            if only != name {
                continue;
            }
        }
        // QUALITE=legere|dense for the other densities of the Bake panel
        let q = match std::env::var("QUALITE").as_deref() {
            Ok("legere") => bake::Quality::Draft,
            Ok("dense") => bake::Quality::Max,
            _ => bake::Quality::Standard,
        };
        let st = bake::BakeSettings::preset(q);
        let size = bake::plan(&scene, &st).unwrap_or(5.0);
        let ls = bake::layout::LayoutSettings { size, tol: st.tol * 2.0, threads: st.threads, diagonal };
        match bake::layout::build(&scene, &ls, &bake::Progress::default()) {
            Ok(b) => {
                let s = &b.stats;
                println!(
                    "{name} : {} faces ({} quads, {} tris, {} n-gones), {} patchs, écart moy {:.4} max {:.4} mm, {} bords ouverts, {:.0} ms",
                    b.faces.len(),
                    s.quads,
                    s.tris,
                    s.ngons,
                    s.patches,
                    s.mean_dev,
                    s.max_dev,
                    s.open_edges,
                    s.seconds * 1000.0
                );
                // faces whose normal leaves the surface (folds, twisted quads)
                let field = Field::new(&compile(&scene));
                let mut worst = (1.0f64, 0usize);
                let mut bad = 0;
                for (i, f) in b.faces.iter().enumerate() {
                    let p: Vec<DVec3> = f.verts.iter().map(|&v| b.positions[v as usize]).collect();
                    if p.len() != 4 {
                        continue;
                    }
                    let c = (p[0] + p[1] + p[2] + p[3]) * 0.25;
                    let g = field.grad(c).normalize_or_zero();
                    for k in 0..4 {
                        let n = (p[(k + 1) % 4] - p[k]).cross(p[(k + 3) % 4] - p[k]).normalize_or_zero();
                        let d = n.dot(g);
                        if d < worst.0 {
                            worst = (d, i);
                        }
                        if d < 0.9 {
                            bad += 1;
                        }
                    }
                }
                let wf = &b.faces[worst.1];
                println!("  coins de quad hors surface (< 0.9) : {bad}, pire {:.3} face {} {:?}", worst.0, worst.1, wf.verts.iter().map(|&v| b.positions[v as usize]).collect::<Vec<_>>());
                render(&b, &format!("{out}/parlignes-{name}-a.png"), eye, target, ext, 1400, 1050);
                render(&b, &format!("{out}/parlignes-{name}-b.png"), DVec3::new(0.9, -0.6, 0.8), target, ext, 1400, 1050);
                render(&b, &format!("{out}/parlignes-{name}-dessus.png"), DVec3::new(0.001, 1.0, 0.0), target, ext * 0.6, 1400, 1050);
                // one more view: VUE="eye x,y,z;target x,y,z;extent"
                if let Ok(v) = std::env::var("VUE") {
                    let f: Vec<f64> = v.split([';', ',']).filter_map(|x| x.trim().parse().ok()).collect();
                    if f.len() == 7 {
                        render(&b, &format!("{out}/parlignes-{name}-vue.png"), DVec3::new(f[0], f[1], f[2]), DVec3::new(f[3], f[4], f[5]), f[6], 1400, 1050);
                    }
                }
            }
            Err(e) => println!("{name} : ÉCHEC {e}"),
        }
    }
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
        let part = bake::bake_blocking(&scene, bake::Quality::Standard).ok();
        let lines: Vec<(&Vec<DVec3>, [u8; 3], bool, bool)> =
            l.lines.iter().map(|x| (&x.pts, if x.kind.is_rail() { [255, 140, 40] } else { [250, 215, 70] }, false, false)).collect();
        render_lines(part.as_ref(), None, &lines, &format!("{out}/construction-{name}.png"), eye, target, ext, 1400, 1050);
    }
}

/// The optional pass (bake::decimate) on the bake of Romain's scene: the
/// dissolved loops in red, then the result.
fn decime() {
    use glam::DVec3;
    let out = std::env::var("OUT").unwrap_or_else(|_| "/tmp".into());
    let deg: f64 = std::env::var("DEG").ok().and_then(|v| v.parse().ok()).unwrap_or(5.0);
    let scene = Scene::from_json(include_str!("scenes/romain-coin.forme")).expect("scène de Romain");
    let full = Field::new(&compile(&scene));
    let b = match bake::bake_blocking(&scene, bake::Quality::Standard) {
        Ok(b) => b,
        Err(e) => return println!("bake impossible : {e}"),
    };
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
    let (target, ext) = (DVec3::new(39.5, 19.4, 51.5), 80.0);
    for (v, dir) in [("arriere", DVec3::new(-0.8, 0.7, -1.0)), ("avant", DVec3::new(0.8, 0.7, 1.0))] {
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
