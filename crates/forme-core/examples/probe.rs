use forme_core::*;
use glam::Vec3;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let s = Scene::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    for (id, n) in &s.nodes {
        if let Some(sh) = n.shape() {
            println!("{id} {} {:?} {:?} size={:?} pos={:?} rot={:?} mode={:?} aabb={:?}", n.name, sh.kind, n.repeat, sh.size, n.transform.pos, n.transform.rot, n.combine.mode, s.shape_world_aabb(*id));
        }
    }
    let p = compile(&s);
    let top = 34.5;
    for (x, z) in [(10.0f32, 0.0f32), (-8.0, 18.0), (-26.0, 0.0), (5.0, 12.7), (10.0, 10.0), (12.0, -12.0), (4.0, 4.0)] {
        let q = Vec3::new(x, top, z);
        let mut line = format!("({x:5.1},{z:5.1}) total={:.3}", p.dist(q));
        for (id, n) in &s.nodes { if n.shape().is_some() { line += &format!(" {}={:.2}", n.name, compile_single(&s, *id).dist(q)); } }
        println!("{line}");
    }
}
