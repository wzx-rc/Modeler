//! Runs the whole pipeline on an .obj and prints what each step found.
//! cargo run --release -p rectify-core --example probe -- in.obj [out.obj]

use rectify_core::script::Script;
use rectify_core::{Kind, Mesh, Params, Pipeline, Stage, Step};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("in.obj");
    let t = Instant::now();
    let m = Mesh::load_obj(path.as_ref()).expect("load");
    let (lo, hi) = m.bounds();
    println!("load {:.1}s: {} vertices, {} triangles, unit {} mm, uv {}, size {:.1} × {:.1} × {:.1} mm, edge {:.3} mm", t.elapsed().as_secs_f64(), m.pos.len(), m.tris.len(), m.unit_mm, m.uv.len(), hi.x - lo.x, hi.y - lo.y, hi.z - lo.z, m.edge_median());
    let t = Instant::now();
    let mut p = Pipeline::new(m);
    println!("topo {:.1}s", t.elapsed().as_secs_f64());
    let mut params = Params::default();
    if let Ok(s) = std::env::var("RECTIFY_PARAMS") {
        for k in params.apply(&s) {
            eprintln!("unknown setting {k}");
        }
    }
    // RECTIFY_SCRIPT=path: stage 3 too, with these edits
    let lanes: Vec<usize> = match std::env::var("RECTIFY_SCRIPT") {
        Ok(path) => {
            p.script = Script::load(path.as_ref()).expect("script");
            vec![0, 1]
        }
        Err(_) => vec![0],
    };
    for (lane, s) in lanes.iter().flat_map(|l| Step::ALL.into_iter().filter(move |s| *l == 0 || s.lane_index().is_some()).map(move |s| (*l, s))) {
        p.run(s, lane, &params, &|_| {});
        let secs = p.time_of(s, lane).unwrap_or(0.0);
        let l = &p.lanes[lane];
        if lane == 1 && s == Step::Regularize {
            for (e, r) in p.script.edits.iter().zip(&p.edit_log) {
                println!("   edit {:?}: {:?}", e.cmd, r);
            }
        }
        match s {
            Step::Symmetry => {
                if let Some(r) = &p.sym {
                    println!("symmetry {:.1}s: n {:.4} {:.4} {:.4} d {:.3}, median {:.3} mm, matched {:.1} %", secs, r.mirror.n.x, r.mirror.n.y, r.mirror.n.z, r.mirror.d, r.median, r.matched * 100.0);
                }
            }
            Step::Smooth => println!("smooth {:.1}s", secs),
            Step::Segment => {
                let sg = p.seg_raw.as_ref().unwrap();
                let areas = p.smooth_out().areas();
                println!("regions {:.1}s: {} planes, {} cylinders, {} spheres, {:.1} % covered", secs, sg.count(Kind::Plane), sg.count(Kind::Cylinder), sg.count(Kind::Sphere), sg.covered(&areas) * 100.0);
                let mut rs: Vec<_> = sg.regions.iter().collect();
                rs.sort_by(|a, b| b.area.partial_cmp(&a.area).unwrap());
                for r in rs.iter().take(if std::env::var("RECTIFY_LIST").is_ok() { 25 } else { 0 }) {
                    println!("   {:9} {:8.1} mm²  rms {:.3}  {}", r.prim.kind().label(), r.area, r.rms, r.prim.describe());
                }
            }
            Step::Regularize => {
                if let Some(r) = &l.reg {
                    println!("regularize {:.1}s: {:?}", secs, r);
                }
            }
            Step::Snap => {
                if let Some(r) = &l.snap {
                    println!("snap {:.1}s: edges {} (tangent {}, rejected {}, radius groups {}) radii p10 {:.2} p50 {:.2} p90 {:.2}, on edge {} faded {} free {} eased {}, mean {:.3} max {:.3}", secs, r.edges, r.tangent, r.rejected, r.radius_groups, r.radii.get(r.radii.len() / 10).unwrap_or(&0.0), r.radii.get(r.radii.len() / 2).unwrap_or(&0.0), r.radii.get(r.radii.len() * 9 / 10).unwrap_or(&0.0), r.on_edge, r.faded, r.free, r.eased, r.mean_move, r.max_move);
                }
            }
            Step::Revolve => {
                if let Some(r) = &l.revolve {
                    println!("lane {lane} round {:.1}s: {} pieces done of {}, {} vertices moved", secs, r.done(), r.pieces.len(), r.moved);
                    for pc in r.pieces.iter().filter(|pc| std::env::var("RECTIFY_NEAR").ok().and_then(|s| { let v: Vec<f64> = s.split(',').filter_map(|t| t.parse().ok()).collect(); (v.len() == 4).then(|| (pc.p - glam::DVec3::new(v[0], v[1], v[2])).length() < v[3]) }).unwrap_or(true)).take(if std::env::var("RECTIFY_LIST").is_ok() { 40 } else { 0 }) {
                        println!("   r {:.2} h {:.2}..{:.2} at {:.1?} axis {:.2?} verts {} moved {} details {} thick {:.3} {:?}", pc.r_max, pc.h.0, pc.h.1, pc.p, pc.a, pc.verts, pc.moved, pc.details, pc.thick, pc.skipped);
                    }
                }
            }
            Step::Clean => {
                if let Some(r) = &l.tidy {
                    println!("lane {lane} clean {:.1}s: {:?}", secs, r);
                }
            }
        }
    }
    let _ = Stage::ALL;
    if let (Some(out), Some(m)) = (args.get(2), p.stage_mesh(if lanes.len() > 1 { Stage::Guided } else { Stage::Cleaned })) {
        std::fs::write(out, m.obj(None)).expect("write");
        println!("wrote {out}");
    }
}
