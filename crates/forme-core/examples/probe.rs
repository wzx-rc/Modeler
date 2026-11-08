use forme_core::*;
use glam::Vec3;
fn main() {
    let s = demo::grenade();
    let p = compile(&s);
    for z in [36.0f32, 34.0, 33.0, 32.0, 31.0, 30.0, 29.0, 28.0, 27.0, 26.0] {
        let q = Vec3::new(0.0, 46.0, z);
        print!("z={z:5.1} total={:7.3}  ", p.dist(q));
        for (id, n) in &s.nodes {
            if n.shape().is_none() { continue; }
            let d = compile_single(&s, *id).dist(q);
            if d.abs() < 4.0 { print!("{}={:.2} ", n.name, d); }
        }
        println!();
    }
}
