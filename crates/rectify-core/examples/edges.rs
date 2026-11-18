//! The Edges tool alone on a mesh: how many flat faces and edges, how long.
//! cargo run --release -p rectify-core --example edges -- mesh.obj [out.obj]

use rectify_core::edges::{self, EdgeParams};
use rectify_core::line::LineParams;
use rectify_core::{Mesh, Topo};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut m = Mesh::load_obj(args[1].as_ref()).expect("mesh");
    let topo = Topo::new(&m);
    let t = std::time::Instant::now();
    let found = edges::find(&m, &topo, &EdgeParams::default());
    eprintln!("{} edges in {:.1}s", found.len(), t.elapsed().as_secs_f64());
    for f in found.iter().take(12) {
        eprintln!("  {:.1} mm, {:.0}°, faces {:?}", (f.b - f.a).length(), f.angle, f.faces);
    }
    let t = std::time::Instant::now();
    let (fits, moves) = edges::fix_all(&m, &topo, &found, &vec![true; found.len()], &LineParams::default());
    let ok = fits.iter().filter(|f| f.is_ok()).count();
    eprintln!("{} vertices moved, {:.2} mm at most", moves.idx.len(), moves.max_move());
    eprintln!("{ok} fixed of {} in {:.1}s", fits.len(), t.elapsed().as_secs_f64());
    let mut errs: std::collections::HashMap<String, usize> = Default::default();
    for f in &fits {
        if let Err(e) = f {
            *errs.entry(e.split(|c: char| c == ':' || c.is_ascii_digit()).next().unwrap_or("").to_string()).or_default() += 1;
        }
    }
    for (e, n) in errs {
        eprintln!("  {n} × {e}");
    }
    if let Some(out) = args.get(2) {
        moves.apply(&mut m);
        std::fs::write(out, m.obj(None)).unwrap();
    }
}
