//! How rough each stage is inside a box, next to the AI mesh (bend, umbrella,
//! stray), and how much of the snap the clean-up kept, by snap kind.
//! cargo run --release -p rectify-core --example rough -- in.obj "x0,y0,z0,x1,y1,z1"

use glam::DVec3;
use rectify_core::mesh::{Mesh, Topo};
use rectify_core::tidy::bend;
use rectify_core::{Params, Pipeline, Stage};

fn umbrella(m: &Mesh, topo: &Topo) -> Vec<f32> {
    let vn = m.vertex_normals(topo);
    (0..m.pos.len())
        .map(|v| {
            let ns = topo.verts_of(v);
            if ns.is_empty() {
                return 0.0;
            }
            let c = ns.iter().map(|&u| m.pos[u as usize]).sum::<DVec3>() / ns.len() as f64;
            let r = ns.iter().map(|&u| (m.pos[u as usize] - m.pos[v]).length()).sum::<f64>() / ns.len() as f64;
            ((m.pos[v] - c).dot(vn[v]).abs() / r.max(1e-9)).atan().to_degrees() as f32
        })
        .collect()
}

fn dev(m: &Mesh, topo: &Topo) -> Vec<f32> {
    let vn = m.vertex_normals(topo);
    (0..m.pos.len())
        .map(|v| {
            let ns = topo.verts_of(v);
            let a = ns.iter().map(|&u| vn[u as usize]).sum::<DVec3>().normalize_or_zero();
            vn[v].dot(a).clamp(-1.0, 1.0).acos().to_degrees() as f32
        })
        .collect()
}

fn stats(name: &str, inb: &[usize], a: &[f32], b: &[f32]) {
    let mut x: Vec<f32> = inb.iter().map(|&v| a[v]).collect();
    x.sort_by(|p, q| p.partial_cmp(q).unwrap());
    let q = |f: f64| x[((x.len() - 1) as f64 * f) as usize];
    let mut y: Vec<f32> = inb.iter().map(|&v| b[v]).collect();
    y.sort_by(|p, q| p.partial_cmp(q).unwrap());
    let w = |f: f64| y[((y.len() - 1) as f64 * f) as usize];
    println!("{name:10} bend p50 {:5.1} p90 {:5.1} p99 {:5.1} | umbrella p50 {:5.1} p90 {:5.1} p99 {:5.1}", q(0.5), q(0.9), q(0.99), w(0.5), w(0.9), w(0.99));
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let m = Mesh::load_obj(a[1].as_ref()).unwrap();
    let b: Vec<f64> = a[2].split(',').map(|s| s.parse().unwrap()).collect();
    let (lo, hi) = (DVec3::new(b[0], b[1], b[2]), DVec3::new(b[3], b[4], b[5]));
    let mut p = Pipeline::new(m);
    let mut params = Params::default();
    if let Ok(s) = std::env::var("RECTIFY_PARAMS") {
        params.apply(&s);
    }
    p.run_stage(Stage::Cleaned, &params, &|_, _| {});
    let inb: Vec<usize> = (0..p.original.pos.len()).filter(|&v| {
        let x = p.original.pos[v];
        x.cmpge(lo).all() && x.cmple(hi).all()
    }).collect();
    println!("{} vertices in the box", inb.len());
    let topo = &p.topo;
    for (name, m) in [("original", Some(&p.original)), ("base", Some(p.base())), ("smooth", Some(p.smooth_out())), ("stage1", p.stage_mesh(Stage::Algorithm)), ("stage2", p.stage_mesh(Stage::Cleaned))] {
        if let Some(m) = m {
            stats(name, &inb, &bend(m, topo), &umbrella(m, topo));
            let d = dev(m, topo);
            let mut x: Vec<f32> = inb.iter().map(|&v| d[v]).collect();
            x.sort_by(|p, q| p.partial_cmp(q).unwrap());
            let q = |f: f64| x[((x.len() - 1) as f64 * f) as usize];
            println!("{:10} dev  p50 {:5.2} p90 {:5.2} p99 {:5.2}", "", q(0.5), q(0.9), q(0.99));
        }
    }
    println!("tidy {:?}", p.lanes[0].tidy);
    // the moves' own roughness, by kind
    let kinds = &p.lanes[0].snap.as_ref().unwrap().kinds;
    let before = p.lanes[0].diff_result.as_ref().unwrap();
    let after = p.lanes[0].diff_cleaned.as_ref().unwrap();
    let nv = p.original.pos.len();
    for k in [0u8, 2, 3, 4, 5] {
        let n = (0..nv).filter(|&v| kinds.get(v) == Some(&k)).count();
        let rb = (0..nv).filter(|&v| kinds.get(v) == Some(&k) && before.worse[v] > 0.0).count();
        let mv = (0..nv).filter(|&v| kinds.get(v) == Some(&k)).map(|v| before.moved[v] as f64).sum::<f64>() / n.max(1) as f64;
        let ma = (0..nv).filter(|&v| kinds.get(v) == Some(&k)).map(|v| after.moved[v] as f64).sum::<f64>() / n.max(1) as f64;
        println!("kind {k}: {n} vertices, rough before {rb}, mean move {mv:.4} -> {ma:.4} mm");
    }
}
