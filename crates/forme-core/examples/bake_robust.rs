//! Robustness of the Bake on simple parts: one plate, one feature (hole,
//! blind hole, boss, box, pocket, with or without a junction blend), moved
//! over a 1 mm grid across the top face and past its edges; fillet patterns
//! and radius sweeps. Every case is baked at Standard and sorted:
//!
//! - ok: closed, all quads, no thin or squashed fill quad, no fold;
//! - défaut: baked, but with a thin row, a squashed quad or a fold;
//! - règle: Err from a rule of the layout (patch à 3 coins, counts…): the
//!   foundation to make solid;
//! - périmètre: Err "pas encore géré" (outside what the Bake handles);
//! - panique: the bake panicked.
//!
//! cargo run --release -p forme-core --example bake_robust -- [family…]
//! OUT=dir (default /tmp/robust): one PNG map per family, results.tsv, résumé.txt
use forme_core::bake::{self, field::Field};
use forme_core::glam::{DVec3, Vec3};
use forme_core::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// The base part: a 20 x 10 x 20 mm plate, top face at y = 10, spanning
/// x, z in [-10, 10]. `edges`: (edge index, radius).
fn plate(edges: &[(usize, f32)]) -> Scene {
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::new(10.0, 5.0, 10.0);
    for &(e, r) in edges {
        b.set_edge(e, r);
    }
    s.add(root, NodeSpec::shape("Plaque", b).at(Vec3::new(0.0, 5.0, 0.0)));
    s
}

fn cyl(s: &mut Scene, name: &str, r: f32, half: f32, at: Vec3, mode: CombineMode, blend: f32) {
    let root = s.root;
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(r, half, 0.0);
    s.add(root, NodeSpec::shape(name, c).at(at).mode(mode).blend(blend));
}

/// A cylinder with rounded rims (`rims`: top, bottom radius, 0 sharp).
#[allow(clippy::too_many_arguments)]
fn cyl_rims(s: &mut Scene, name: &str, r: f32, half: f32, at: Vec3, mode: CombineMode, blend: f32, rims: [f32; 2]) {
    let root = s.root;
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(r, half, 0.0);
    for (i, &v) in rims.iter().enumerate() {
        c.set_edge(i, v);
    }
    s.add(root, NodeSpec::shape(name, c).at(at).mode(mode).blend(blend));
}

fn boxe(s: &mut Scene, name: &str, half: Vec3, at: Vec3, mode: CombineMode, blend: f32) {
    let root = s.root;
    let mut c = Shape::new(PrimKind::Box);
    c.size = half;
    s.add(root, NodeSpec::shape(name, c).at(at).mode(mode).blend(blend));
}

/// Top edges 0, 3, 8, 9; vertical 4..7; bottom 1, 2, 10, 11.
const TOP: [usize; 4] = [0, 3, 8, 9];
const VERT: [usize; 4] = [4, 5, 6, 7];

struct Case {
    /// Grid cell (column, row), or position in a 1D sweep (row 0).
    cell: (usize, usize),
    tag: String,
    scene: Scene,
}

struct Family {
    name: &'static str,
    what: &'static str,
    cols: usize,
    rows: usize,
    /// A map over the plate (x, z), or a strip of labelled cases.
    map: bool,
    cases: Vec<Case>,
}

/// Grid of feature centres over the top face and 1 mm past its edges.
const G: [f32; 23] = [-11., -10., -9., -8., -7., -6., -5., -4., -3., -2., -1., 0., 1., 2., 3., 4., 5., 6., 7., 8., 9., 10., 11.];

fn grid(name: &'static str, what: &'static str, f: impl Fn(f32, f32) -> Scene) -> Family {
    let mut cases = vec![];
    for (iz, &z) in G.iter().enumerate() {
        for (ix, &x) in G.iter().enumerate() {
            cases.push(Case { cell: (ix, iz), tag: format!("x {x} z {z}"), scene: f(x, z) });
        }
    }
    Family { name, what, cols: G.len(), rows: G.len(), map: true, cases }
}

fn families() -> Vec<Family> {
    use CombineMode::{Add, Carve};
    let mut out = vec![
        grid("trou", "trou traversant r 3", |x, z| {
            let mut s = plate(&[]);
            cyl(&mut s, "Trou", 3.0, 7.0, Vec3::new(x, 5.0, z), Carve, 0.0);
            s
        }),
        grid("trou-petit", "trou traversant r 1,2 (un demi-quad)", |x, z| {
            let mut s = plate(&[]);
            cyl(&mut s, "Trou", 1.2, 7.0, Vec3::new(x, 5.0, z), Carve, 0.0);
            s
        }),
        grid("trou-borgne", "trou borgne r 3, 3 mm de fond", |x, z| {
            let mut s = plate(&[]);
            cyl(&mut s, "Trou", 3.0, 3.0, Vec3::new(x, 10.0, z), Carve, 0.0);
            s
        }),
        grid("bossage", "cylindre posé r 3, vif", |x, z| {
            let mut s = plate(&[]);
            cyl(&mut s, "Bossage", 3.0, 2.5, Vec3::new(x, 12.0, z), Add, 0.0);
            s
        }),
        grid("bossage-raccord", "cylindre posé r 3, raccord 1,5", |x, z| {
            let mut s = plate(&[]);
            cyl(&mut s, "Bossage", 3.0, 2.5, Vec3::new(x, 12.0, z), Add, 1.5);
            s
        }),
        grid("bossage-arrondi", "cylindre posé r 3, bord du dessus arrondi 0,7", |x, z| {
            let mut s = plate(&[]);
            cyl_rims(&mut s, "Bossage", 3.0, 2.5, Vec3::new(x, 12.0, z), Add, 0.0, [0.7, 0.0]);
            s
        }),
        grid("bossage-arrondi-raccord", "cylindre posé r 3, bord du dessus arrondi 0,7, raccord 1", |x, z| {
            let mut s = plate(&[]);
            cyl_rims(&mut s, "Bossage", 3.0, 2.5, Vec3::new(x, 12.0, z), Add, 1.0, [0.7, 0.0]);
            s
        }),
        grid("trou-borgne-arrondi", "trou borgne r 3, 3 mm de fond, fond arrondi 1", |x, z| {
            let mut s = plate(&[]);
            cyl_rims(&mut s, "Trou", 3.0, 3.0, Vec3::new(x, 10.0, z), Carve, 0.0, [0.0, 1.0]);
            s
        }),
        grid("boite", "boîte posée 4 x 6, vive", |x, z| {
            let mut s = plate(&[]);
            boxe(&mut s, "Boîte", Vec3::new(2.0, 2.5, 3.0), Vec3::new(x, 12.0, z), Add, 0.0);
            s
        }),
        grid("boite-raccord", "boîte posée 4 x 6, raccord 1,5", |x, z| {
            let mut s = plate(&[]);
            boxe(&mut s, "Boîte", Vec3::new(2.0, 2.5, 3.0), Vec3::new(x, 12.0, z), Add, 1.5);
            s
        }),
        grid("poche", "poche 5 x 7, 3 mm de fond", |x, z| {
            let mut s = plate(&[]);
            boxe(&mut s, "Poche", Vec3::new(2.5, 3.0, 3.5), Vec3::new(x, 10.0, z), Carve, 0.0);
            s
        }),
        grid("trou-arrondie", "trou r 3 dans une plaque arrondie (verticales et dessus r 2)", |x, z| {
            let e: Vec<(usize, f32)> = VERT.iter().chain(TOP.iter()).map(|&e| (e, 2.0)).collect();
            let mut s = plate(&e);
            cyl(&mut s, "Trou", 3.0, 7.0, Vec3::new(x, 5.0, z), Carve, 0.0);
            s
        }),
        grid("deux-trous", "trou r 2,5 fixe en (-4, -2), second trou r 2,5", |x, z| {
            let mut s = plate(&[]);
            cyl(&mut s, "Trou 1", 2.5, 7.0, Vec3::new(-4.0, 5.0, -2.0), Carve, 0.0);
            cyl(&mut s, "Trou 2", 2.5, 7.0, Vec3::new(x, 5.0, z), Carve, 0.0);
            s
        }),
        grid("trou-boite", "trou r 2,5 fixe en (-4, -2), boîte posée 4 x 6", |x, z| {
            let mut s = plate(&[]);
            cyl(&mut s, "Trou", 2.5, 7.0, Vec3::new(-4.0, 5.0, -2.0), Carve, 0.0);
            boxe(&mut s, "Boîte", Vec3::new(2.0, 2.5, 3.0), Vec3::new(x, 12.0, z), Add, 0.0);
            s
        }),
    ];

    // fillet patterns on the plate alone
    let patterns: [(&str, Vec<usize>); 8] = [
        ("1 arête du dessus", vec![0]),
        ("1 verticale", vec![4]),
        ("2 arêtes en coin", vec![0, 8]),
        ("3 arêtes en coin", vec![0, 8, 4]),
        ("4 verticales", VERT.to_vec()),
        ("tour du dessus", TOP.to_vec()),
        ("dessus + verticales", TOP.iter().chain(VERT.iter()).copied().collect()),
        ("12 arêtes", (0..12).collect()),
    ];
    let radii = [0.5f32, 1.0, 2.0, 3.0, 4.5];
    let mut cases = vec![];
    for (row, (label, edges)) in patterns.iter().enumerate() {
        for (col, &r) in radii.iter().enumerate() {
            let e: Vec<(usize, f32)> = edges.iter().map(|&e| (e, r)).collect();
            cases.push(Case { cell: (col, row), tag: format!("{label}, r {r}"), scene: plate(&e) });
        }
    }
    out.push(Family { name: "fillets", what: "plaque seule, motifs de fillets (colonnes : r 0,5 1 2 3 4,5)", cols: radii.len(), rows: patterns.len(), map: false, cases });

    // sizes: hole and boss radius, blend size, all centred
    let mut cases = vec![];
    for (col, i) in (1..=19).enumerate() {
        let r = i as f32 * 0.5;
        let mut s = plate(&[]);
        cyl(&mut s, "Trou", r, 7.0, Vec3::new(0.0, 5.0, 0.0), Carve, 0.0);
        cases.push(Case { cell: (col, 0), tag: format!("trou r {r}"), scene: s });
        let mut s = plate(&[]);
        cyl(&mut s, "Bossage", r, 2.5, Vec3::new(0.0, 12.0, 0.0), Add, 0.0);
        cases.push(Case { cell: (col, 1), tag: format!("bossage r {r}"), scene: s });
        let k = i as f32 * 0.25;
        let mut s = plate(&[]);
        cyl(&mut s, "Bossage", 3.0, 2.5, Vec3::new(0.0, 12.0, 0.0), Add, k);
        cases.push(Case { cell: (col, 2), tag: format!("bossage r 3 raccord {k}"), scene: s });
        let mut s = plate(&[]);
        boxe(&mut s, "Boîte", Vec3::new(2.0, 2.5, 3.0), Vec3::new(0.0, 12.0, 0.0), Add, k);
        cases.push(Case { cell: (col, 3), tag: format!("boîte raccord {k}"), scene: s });
    }
    out.push(Family { name: "tailles", what: "au centre : trou r, bossage r (0,5 à 9,5), raccord k du bossage r 3 et de la boîte (0,25 à 4,75)", cols: 19, rows: 4, map: false, cases });

    // rounded rims of a cylinder, centred: the rim radius (0 sharp, 0,25 to 4,75)
    // on a boss, a boss with a junction blend, a blind hole's floor, and a
    // boss rounded at both ends standing on its rounded foot
    let mut cases = vec![];
    for (col, i) in (0..=19).enumerate() {
        let r = i as f32 * 0.25;
        let mut s = plate(&[]);
        cyl_rims(&mut s, "Bossage", 5.0, 2.5, Vec3::new(0.0, 12.0, 0.0), Add, 0.0, [r, 0.0]);
        cases.push(Case { cell: (col, 0), tag: format!("bossage r 5, bord r {r}"), scene: s });
        let mut s = plate(&[]);
        cyl_rims(&mut s, "Bossage", 5.0, 2.5, Vec3::new(0.0, 12.0, 0.0), Add, 1.0, [r, 0.0]);
        cases.push(Case { cell: (col, 1), tag: format!("bossage r 5, bord r {r}, raccord 1"), scene: s });
        let mut s = plate(&[]);
        cyl_rims(&mut s, "Trou", 5.0, 3.0, Vec3::new(0.0, 10.0, 0.0), Carve, 0.0, [0.0, r]);
        cases.push(Case { cell: (col, 2), tag: format!("trou borgne r 5, fond r {r}"), scene: s });
        let mut s = plate(&[]);
        cyl_rims(&mut s, "Bossage", 5.0, 2.5, Vec3::new(0.0, 12.0, 0.0), Add, 0.0, [r, r]);
        cases.push(Case { cell: (col, 3), tag: format!("bossage r 5, deux bords r {r}"), scene: s });
    }
    out.push(Family { name: "cylindre-arrondi", what: "au centre : bord arrondi du cylindre r 0 (vif) à 4,75 (bossage, bossage avec raccord 1, fond de trou borgne, bossage arrondi aux deux bouts)", cols: 20, rows: 4, map: false, cases });

    // Romain's scenes, as validated
    let mut cases = vec![];
    for (col, (name, json)) in [
        ("romain-coin", include_str!("scenes/romain-coin.forme")),
        ("romain-aretes", include_str!("scenes/romain-aretes.forme")),
        ("romain-cylindre", include_str!("scenes/romain-cylindre.forme")),
        ("romain-bossage", include_str!("scenes/romain-bossage.forme")),
        ("romain-cylindre-arrondi", include_str!("scenes/romain-cylindre-arrondi.forme")),
    ]
    .into_iter()
    .enumerate()
    {
        cases.push(Case { cell: (col, 0), tag: name.into(), scene: Scene::from_json(json).expect(name) });
    }
    out.push(Family { name: "romain", what: "tes scènes (la dernière : pas encore réglée)", cols: 5, rows: 1, map: false, cases });
    out
}

#[derive(Clone, Debug)]
enum Outcome {
    Ok { quads: usize, thin: usize, squashed: usize, folded: usize },
    Scope(String),
    Rule(String),
    Panic(String),
}

impl Outcome {
    fn class(&self) -> &'static str {
        match self {
            Outcome::Ok { thin: 0, squashed: 0, folded: 0, .. } => "ok",
            Outcome::Ok { .. } => "défaut",
            Outcome::Scope(_) => "périmètre",
            Outcome::Rule(_) => "règle",
            Outcome::Panic(_) => "panique",
        }
    }
    fn detail(&self) -> String {
        match self {
            Outcome::Ok { quads, thin, squashed, folded } => format!("{quads} quads, {thin} fins, {squashed} écrasés, {folded} plis"),
            Outcome::Scope(e) | Outcome::Rule(e) | Outcome::Panic(e) => e.clone(),
        }
    }
    fn colour(&self) -> [u8; 3] {
        match self.class() {
            "ok" => [61, 160, 84],
            "défaut" => [236, 172, 48],
            "périmètre" => [150, 150, 156],
            "règle" => [214, 64, 60],
            _ => [40, 20, 60],
        }
    }
}

fn scope(e: &str) -> bool {
    e.contains("pas encore") || e.contains("pour l'instant")
}

/// Bake one scene and grade it. Fill quads only for thin / squashed (fillet
/// rows are thin across by design): thin = one width across under a quarter
/// of the other and under an eighth of a quad, not running on into a fillet,
/// squashed = a corner under 12° or over 170°, fold = a corner whose normal
/// leaves the surface normal by more than 60°.
fn grade(scene: &Scene) -> Outcome {
    let mut st = bake::BakeSettings::preset(bake::Quality::Standard);
    st.threads = 1;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| bake::bake(scene, &st, &bake::Progress::default())));
    let b = match r {
        Err(p) => {
            let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            return Outcome::Panic(msg);
        }
        Ok(Err(e)) if scope(&e) => return Outcome::Scope(e),
        Ok(Err(e)) => return Outcome::Rule(e),
        Ok(Ok(b)) => b,
    };
    if let Ok(path) = std::env::var("OBJ") {
        // the mesh, for a look: "v x y z", "f a b c d role"
        let mut o = String::new();
        for p in &b.positions {
            o += &format!("v {} {} {}\n", p.x, p.y, p.z);
        }
        for f in &b.faces {
            o += &format!("f {} {}\n", f.verts.iter().map(|v| (v + 1).to_string()).collect::<Vec<_>>().join(" "), f.patch);
        }
        let _ = std::fs::write(path, o);
    }
    let size = b.stats.quad_size.max(1e-6);
    let field = Field::new(&compile(scene));
    // narrow fill quads: (narrow width, the two short sides as vertex pairs)
    let nf = b.faces.len();
    let mut narrow: Vec<Option<[(u32, u32); 2]>> = vec![None; nf];
    let mut side_faces: std::collections::HashMap<(u32, u32), Vec<usize>> = Default::default();
    let key = |a: u32, c: u32| (a.min(c), a.max(c));
    let (mut thin, mut squashed, mut folded) = (0, 0, 0);
    for (i, f) in b.faces.iter().enumerate() {
        let p: Vec<DVec3> = f.verts.iter().map(|&v| b.positions[v as usize]).collect();
        if p.len() != 4 {
            continue;
        }
        let v = &f.verts;
        for k in 0..4 {
            side_faces.entry(key(v[k], v[(k + 1) % 4])).or_default().push(i);
        }
        let g = field.grad((p[0] + p[1] + p[2] + p[3]) * 0.25).normalize_or_zero();
        let mut fold = false;
        let mut sq = false;
        for k in 0..4 {
            let (a, c) = (p[(k + 1) % 4] - p[k], p[(k + 3) % 4] - p[k]);
            let n = a.cross(c).normalize_or_zero();
            if n.dot(g) < 0.5 {
                fold = true;
            }
            let ang = a.normalize_or_zero().dot(c.normalize_or_zero()).clamp(-1.0, 1.0).acos().to_degrees();
            if !(12.0..=170.0).contains(&ang) {
                sq = true;
            }
        }
        if fold {
            folded += 1;
        }
        if f.patch != bake::P_FILLET {
            // a row: the two widths across (between opposite side middles)
            // far apart, and the narrow one under an eighth of a quad
            let w1 = ((p[0] + p[1]) - (p[2] + p[3])).length() * 0.5;
            let w2 = ((p[1] + p[2]) - (p[3] + p[0])).length() * 0.5;
            if w1.min(w2) < w1.max(w2) * 0.25 && w1.min(w2) < size * 0.12 {
                // w1 narrow: the short sides are 1-2 and 3-0
                narrow[i] = Some(if w1 < w2 { [key(v[1], v[2]), key(v[3], v[0])] } else { [key(v[0], v[1]), key(v[2], v[3])] });
            }
            if sq {
                squashed += 1;
                if std::env::var("DUMP_THIN").is_ok() {
                    eprintln!("écrasé : {}", p.iter().map(|q| format!("({:.2} {:.2} {:.2})", q.x, q.y, q.z)).collect::<Vec<_>>().join(" "));
                }
            }
        }
    }
    // a narrow row that runs on into a fillet strip carries on the fillet's
    // rows (as the bevel modifier would): not a sliver. A sliver is a narrow
    // row that ends on both sides without reaching a fillet.
    for i in 0..nf {
        let Some(short) = narrow[i] else { continue };
        let mut reaches = false;
        for start in short {
            let (mut cur, mut side) = (i, start);
            for _ in 0..64 {
                let Some(&n) = side_faces.get(&side).and_then(|fs| fs.iter().find(|&&j| j != cur)) else { break };
                if b.faces[n].patch == bake::P_FILLET {
                    reaches = true;
                    break;
                }
                // on through the quad strip while the band stays narrow
                let v = &b.faces[n].verts;
                if v.len() != 4 {
                    break;
                }
                let Some(k) = (0..4).find(|&k| key(v[k], v[(k + 1) % 4]) == side) else { break };
                let opp = key(v[(k + 2) % 4], v[(k + 3) % 4]);
                if b.positions[opp.0 as usize].distance(b.positions[opp.1 as usize]) > size * 0.2 {
                    break;
                }
                side = opp;
                cur = n;
            }
            if reaches {
                break;
            }
        }
        if !reaches {
            thin += 1;
            if std::env::var("DUMP_THIN").is_ok() {
                let p: Vec<DVec3> = b.faces[i].verts.iter().map(|&v| b.positions[v as usize]).collect();
                eprintln!("fin : {}", p.iter().map(|q| format!("({:.2} {:.2} {:.2})", q.x, q.y, q.z)).collect::<Vec<_>>().join(" "));
            }
        }
    }
    Outcome::Ok { quads: b.faces.len(), thin, squashed, folded }
}

fn main() {
    let out = std::env::var("OUT").unwrap_or_else(|_| "/tmp/robust".into());
    let _ = std::fs::create_dir_all(&out);
    let want: Vec<String> = std::env::args().skip(1).collect();
    let mut fams: Vec<Family> = families().into_iter().filter(|f| want.is_empty() || want.iter().any(|w| w == f.name)).collect();
    // CAS="x 8 z 0": that case only (with OBJ=file to write its mesh)
    if let Ok(only) = std::env::var("CAS") {
        for f in &mut fams {
            f.cases.retain(|c| c.tag == only);
        }
    }
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let mut tsv = String::from("famille\tcas\tclasse\tdétail\n");
    let mut summary = String::new();
    let mut totals: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut reasons: std::collections::BTreeMap<String, usize> = Default::default();
    // keep panics quiet: they are counted
    std::panic::set_hook(Box::new(|_| {}));
    for fam in &fams {
        let t0 = std::time::Instant::now();
        let results: Mutex<Vec<Option<Outcome>>> = Mutex::new(vec![None; fam.cases.len()]);
        let next = AtomicUsize::new(0);
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= fam.cases.len() {
                        break;
                    }
                    let o = grade(&fam.cases[i].scene);
                    results.lock().unwrap()[i] = Some(o);
                });
            }
        });
        let results: Vec<Outcome> = results.into_inner().unwrap().into_iter().map(|o| o.unwrap()).collect();
        let mut count: std::collections::BTreeMap<&str, usize> = Default::default();
        let mut why: std::collections::BTreeMap<String, usize> = Default::default();
        for (c, o) in fam.cases.iter().zip(&results) {
            *count.entry(o.class()).or_default() += 1;
            *totals.entry(o.class()).or_default() += 1;
            if !matches!(o, Outcome::Ok { .. }) {
                // patch counts name a chart: keep the reason only
                let mut d = o.detail();
                if let Some(i) = d.find(" sur FaceId") {
                    d.truncate(i);
                }
                *why.entry(format!("{} : {d}", o.class())).or_default() += 1;
                *reasons.entry(format!("{} : {d}", o.class())).or_default() += 1;
            } else if o.class() == "défaut" {
                let Outcome::Ok { thin, squashed, folded, .. } = o else { unreachable!() };
                for (n, w) in [(thin, "quads fins"), (squashed, "quads écrasés"), (folded, "plis")] {
                    if *n > 0 {
                        *why.entry(format!("défaut : {w}")).or_default() += 1;
                        *reasons.entry(format!("défaut : {w}")).or_default() += 1;
                    }
                }
            }
            tsv += &format!("{}\t{}\t{}\t{}\n", fam.name, c.tag, o.class(), o.detail());
        }
        let line = format!(
            "{} ({}) : {} cas, {} ok, {} défaut, {} règle, {} périmètre, {} panique, {:.1} s",
            fam.name,
            fam.what,
            fam.cases.len(),
            count.get("ok").unwrap_or(&0),
            count.get("défaut").unwrap_or(&0),
            count.get("règle").unwrap_or(&0),
            count.get("périmètre").unwrap_or(&0),
            count.get("panique").unwrap_or(&0),
            t0.elapsed().as_secs_f64()
        );
        println!("{line}");
        summary += &line;
        summary += "\n";
        let mut w: Vec<_> = why.into_iter().collect();
        w.sort_by(|a, b| b.1.cmp(&a.1));
        for (r, n) in w.iter().take(8) {
            println!("    {n:4}  {r}");
            summary += &format!("    {n:4}  {r}\n");
        }
        draw(fam, &results, &format!("{out}/robuste-{}.png", fam.name));
    }
    let line = format!(
        "TOTAL : {} ok, {} défaut, {} règle, {} périmètre, {} panique",
        totals.get("ok").unwrap_or(&0),
        totals.get("défaut").unwrap_or(&0),
        totals.get("règle").unwrap_or(&0),
        totals.get("périmètre").unwrap_or(&0),
        totals.get("panique").unwrap_or(&0)
    );
    println!("{line}");
    summary += &line;
    summary += "\n";
    let mut r: Vec<_> = reasons.into_iter().collect();
    r.sort_by(|a, b| b.1.cmp(&a.1));
    for (w, n) in &r {
        summary += &format!("    {n:5}  {w}\n");
    }
    let _ = std::fs::write(format!("{out}/results.tsv"), tsv);
    let _ = std::fs::write(format!("{out}/résumé.txt"), summary);
}

/// One cell per case; on a map, the plate's outline (x, z in [-10, 10]),
/// x to the right, z downwards (front at the bottom).
fn draw(fam: &Family, results: &[Outcome], path: &str) {
    let cell = 18u32;
    let pad = 12u32;
    let (w, h) = (fam.cols as u32 * cell + pad * 2, fam.rows as u32 * cell + pad * 2);
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([250, 250, 248]));
    for (c, o) in fam.cases.iter().zip(results) {
        let (x0, y0) = (pad + c.cell.0 as u32 * cell, pad + c.cell.1 as u32 * cell);
        for y in y0 + 1..y0 + cell - 1 {
            for x in x0 + 1..x0 + cell - 1 {
                img.put_pixel(x, y, image::Rgb(o.colour()));
            }
        }
    }
    if fam.map {
        // plate edges at grid values -10 and 10 (index 1 and 21): through the cell centres
        let a = pad + cell + cell / 2;
        let b = pad + 21 * cell + cell / 2;
        for t in a..=b {
            for (x, y) in [(t, a), (t, b), (a, t), (b, t)] {
                img.put_pixel(x, y, image::Rgb([30, 30, 34]));
            }
        }
    }
    let _ = img.save(path);
}
