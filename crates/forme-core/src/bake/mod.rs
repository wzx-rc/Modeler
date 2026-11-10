//! Bake: parametric history → clean export mesh, by construction lines.
//!
//! One algorithm (`layout`), with the rules validated with Romain: the
//! fillets first, as strips of quads (orange in the role view), then the
//! fill paths between them, reusing the strips' edge vertices exactly. A
//! history it does not handle yet returns `Err(reason)`, shown as is in the
//! Bake panel: there is no fallback mesh.
//! One shot: the result does not keep any link with the scene.

pub mod bevel;
pub mod decimate;
pub mod field;
pub mod gen;
pub mod layout;
pub mod lines;

use crate::mesh::Mesh;
use crate::program::compile;
use crate::scene::{Material, Scene};
use field::Field;
use glam::{DVec2, DVec3, Vec3};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Draft,
    Standard,
    Max,
}

impl Quality {
    pub fn label(self) -> &'static str {
        match self {
            Quality::Draft => "Brouillon",
            Quality::Standard => "Standard",
            Quality::Max => "Max",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BakeSettings {
    /// Chord tolerance on the fillets (mm).
    pub tol: f64,
    pub threads: usize,
    /// Quads across the first shape's largest side.
    pub divisions: f64,
    /// Width (mm) of the pair bevel on the sharp edges (`bevel.rs`, Romain's
    /// Maya reference of 10 nov. 2026); 0 = sharp edges left as they are.
    pub bevel: f64,
}

impl BakeSettings {
    pub fn preset(q: Quality) -> Self {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(16);
        match q {
            Quality::Draft => BakeSettings { tol: 0.05, threads, divisions: 5.0, bevel: bevel::BEVEL_WIDTH },
            Quality::Standard => BakeSettings { tol: 0.015, threads, divisions: 8.0, bevel: bevel::BEVEL_WIDTH },
            Quality::Max => BakeSettings { tol: 0.006, threads, divisions: 12.0, bevel: bevel::BEVEL_WIDTH },
        }
    }
}

/// Shared progress, readable from the UI thread while a bake runs.
#[derive(Default)]
pub struct Progress {
    pub stage: AtomicU32,
    pub done: AtomicU32,
    pub total: AtomicU32,
    pub cancel: AtomicBool,
}

pub const STAGES: [&str; 4] = ["Lecture de l'historique", "Lignes de construction", "Boucles et quads", "Normales"];

impl Progress {
    pub fn fraction(&self) -> f32 {
        let s = self.stage.load(Ordering::Relaxed) as usize;
        let t = self.total.load(Ordering::Relaxed).max(1) as f32;
        let d = self.done.load(Ordering::Relaxed) as f32;
        let w = [0.05f32, 0.55, 0.2, 0.2];
        let mut f: f32 = w[..s.min(w.len())].iter().sum();
        if s < w.len() {
            f += w[s] * (d / t).min(1.0);
        }
        f.min(1.0)
    }
    fn set(&self, stage: u32, total: u32) {
        self.stage.store(stage, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }
    fn tick(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Debug, Default)]
pub struct BakeStats {
    /// Four-sided patches the faces were cut into.
    pub patches: usize,
    pub quads: usize,
    pub tris: usize,
    pub ngons: usize,
    pub verts: usize,
    pub open_edges: usize,
    pub nonmanifold_edges: usize,
    pub max_dev: f64,
    pub mean_dev: f64,
    pub seconds: f64,
    /// Target quad size (mm).
    pub quad_size: f64,
    /// Optional second pass (`decimate`): flat loops removed, 0 when off.
    pub flat_loops: usize,
    /// Faces before that pass.
    pub faces_before_flat: usize,
    pub flat_seconds: f64,
    /// Optional pass (`bevel`): sharp edges bevelled, 0 when off.
    pub bevel_edges: usize,
    /// Corners where the bevel had to be narrower (faces too small).
    pub bevel_clamped: usize,
    pub bevel_seconds: f64,
}

/// Role of a face, in `BFace::patch`: drawn in the role view.
pub const P_FILL: u32 = 1;
pub const P_FILLET: u32 = 2;

#[derive(Clone, Debug)]
pub struct BFace {
    pub verts: Vec<u32>,
    pub normals: Vec<DVec3>,
    pub mat: u32,
    /// Role of the face: `P_FILLET` (fillet strip) or `P_FILL` (fill path).
    pub patch: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Baked {
    pub positions: Vec<DVec3>,
    pub faces: Vec<BFace>,
    pub stats: BakeStats,
}

fn par_for<T: Send, F: Fn(usize) -> T + Sync>(n: usize, threads: usize, order: &[usize], f: F) -> Vec<Option<T>> {
    let next = AtomicU32::new(0);
    let out: Mutex<Vec<Option<T>>> = Mutex::new((0..n).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed) as usize;
                if i >= order.len() {
                    break;
                }
                let idx = order[i];
                let r = f(idx);
                out.lock().unwrap()[idx] = Some(r);
            });
        }
    });
    out.into_inner().unwrap()
}

/// Quad size for `scene` (mm), or why the bake does not handle its history.
pub fn plan(scene: &Scene, st: &BakeSettings) -> Result<f64, String> {
    let field = Field::new(&compile(scene));
    let steps = field.steps().ok_or("historique en arbre (groupes fusionnés), pas encore géré")?;
    let first = steps.first().ok_or("scène vide")?;
    layout::handles(scene)?;
    let pr = &field.prims[first.prim as usize];
    let (lo, hi) = gen::prim_bounds(pr, 0.0);
    let extent = ((hi - lo).max_element() - 0.4).max(1e-3) * pr.sc;
    Ok(extent / st.divisions.max(1.0))
}

/// Runs the whole bake. `Err("annulé")` when cancelled, else the reason the
/// history is not handled yet.
pub fn bake(scene: &Scene, st: &BakeSettings, progress: &Progress) -> Result<Baked, String> {
    progress.set(0, 1);
    let size = plan(scene, st)?;
    let ls = layout::LayoutSettings { size, tol: (st.tol * 2.0).max(0.01), threads: st.threads, diagonal: true };
    let r = layout::build(scene, &ls, progress);
    if progress.cancelled() {
        return Err("annulé".into());
    }
    // the bevel on the sharp edges is part of the Bake (Romain, 10 nov.
    // 2026): a corner it cannot bevel is an Err like any other
    let b = r?;
    if st.bevel > 0.0 {
        return bevel::bevel_sharp(scene, &b, st.bevel);
    }
    Ok(b)
}

pub(crate) fn finish(field: &Field, out: &mut Baked, st: &BakeSettings, progress: &Progress) {
    let pos = out.positions.clone();
    // exact normal once per vertex
    let nv_order: Vec<usize> = (0..pos.len()).collect();
    let vnorm: Vec<DVec3> = par_for(pos.len(), st.threads, &nv_order, |i| field.grad(pos[i]).normalize_or_zero()).into_iter().map(|n| n.unwrap()).collect();
    let n_faces = out.faces.len();
    let order: Vec<usize> = (0..n_faces).collect();
    let faces_ref = &out.faces;
    let res = par_for(n_faces, st.threads, &order, |fi| {
        let f = &faces_ref[fi];
        let m = f.verts.len();
        let c = f.verts.iter().fold(DVec3::ZERO, |s, v| s + pos[*v as usize]) / m as f64;
        let sc = field.eval(c);
        let mat = sc.mat;
        let mut newell = DVec3::ZERO;
        for i in 0..m {
            newell += pos[f.verts[i] as usize].cross(pos[f.verts[(i + 1) % m] as usize]);
        }
        let nface = newell.normalize_or_zero();
        // smooth where the exact normal agrees with the face, else crease
        let normals: Vec<DVec3> = f
            .verts
            .iter()
            .map(|&v| {
                let mut nv = vnorm[v as usize];
                if nface != DVec3::ZERO && nv.dot(nface) < 0.999 {
                    // on a sharp edge the gradient mixes both sides: take it
                    // just inside this face instead
                    let p = pos[v as usize];
                    let to_c = c - p;
                    let step = (to_c.length() * 1e-4).max(1e-5).min(to_c.length() * 0.5);
                    let g = field.grad(p + to_c.normalize_or_zero() * step).normalize_or_zero();
                    if g.dot(nv) < 0.999 {
                        nv = g;
                    }
                }
                if nv.dot(nface) > 0.9 || nface == DVec3::ZERO {
                    nv
                } else {
                    nface
                }
            })
            .collect();
        // deviation: face centre and longest edge midpoint against the exact surface
        let mut dev = sc.d.abs();
        let mut best = (0.0, 0);
        for i in 0..m {
            let l = pos[f.verts[i] as usize].distance(pos[f.verts[(i + 1) % m] as usize]);
            if l > best.0 {
                best = (l, i);
            }
        }
        let (a, b) = (pos[f.verts[best.1] as usize], pos[f.verts[(best.1 + 1) % m] as usize]);
        dev = dev.max(field.dist((a + b) * 0.5).abs());
        if fi % 64 == 0 {
            progress.done.fetch_add(64, Ordering::Relaxed);
        }
        (mat, normals, dev)
    });
    let mut sum = 0.0;
    let mut maxd = 0.0f64;
    for (f, r) in out.faces.iter_mut().zip(res) {
        let (mat, normals, dev) = r.unwrap();
        f.mat = mat;
        f.normals = normals;
        sum += dev;
        maxd = maxd.max(dev);
    }
    let s = &mut out.stats;
    s.max_dev = maxd;
    s.mean_dev = sum / n_faces.max(1) as f64;
    s.verts = out.positions.len();
    for f in &out.faces {
        match f.verts.len() {
            3 => s.tris += 1,
            4 => s.quads += 1,
            _ => s.ngons += 1,
        }
    }
    let mut und: HashMap<(u32, u32), u32> = HashMap::new();
    for f in &out.faces {
        let v = &f.verts;
        for i in 0..v.len() {
            let (a, b) = (v[i], v[(i + 1) % v.len()]);
            *und.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    s.open_edges = und.values().filter(|&&c| c == 1).count();
    s.nonmanifold_edges = und.values().filter(|&&c| c > 2).count();
}

impl Baked {
    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.faces.iter().map(|f| f.verts.len().saturating_sub(2)).sum()
    }

    /// Triangles (corner indices of the face) of one polygon.
    pub fn triangulate_face(&self, f: &BFace) -> Vec<[usize; 3]> {
        let m = f.verts.len();
        if m == 3 {
            return vec![[0, 1, 2]];
        }
        let p: Vec<DVec3> = f.verts.iter().map(|v| self.positions[*v as usize]).collect();
        let mut n = DVec3::ZERO;
        for i in 0..m {
            n += p[i].cross(p[(i + 1) % m]);
        }
        let n = n.normalize_or(DVec3::Y);
        let u = n.any_orthonormal_vector();
        let v = n.cross(u);
        let q: Vec<DVec2> = p.iter().map(|x| DVec2::new(x.dot(u), x.dot(v))).collect();
        gen::ear_clip(&q)
    }

    /// Triangle mesh with vertices split on creases and material changes.
    pub fn to_mesh(&self) -> Mesh {
        let mut mesh = Mesh::default();
        let mut map: HashMap<(u32, [i32; 3], u32), u32> = HashMap::new();
        for f in &self.faces {
            let ids: Vec<u32> = f
                .verts
                .iter()
                .zip(&f.normals)
                .map(|(&v, n)| {
                    let q = [(n.x * 1000.0).round() as i32, (n.y * 1000.0).round() as i32, (n.z * 1000.0).round() as i32];
                    *map.entry((v, q, f.mat)).or_insert_with(|| {
                        let p = self.positions[v as usize];
                        mesh.positions.push(Vec3::new(p.x as f32, p.y as f32, p.z as f32));
                        mesh.normals.push(Vec3::new(n.x as f32, n.y as f32, n.z as f32));
                        mesh.materials.push(f.mat);
                        mesh.positions.len() as u32 - 1
                    })
                })
                .collect();
            for t in self.triangulate_face(f) {
                mesh.indices.extend_from_slice(&[ids[t[0]], ids[t[1]], ids[t[2]]]);
            }
        }
        mesh
    }

    /// Wavefront OBJ keeping quads and n-gons, one group per material.
    pub fn to_obj(&self, materials: &[Material], name: &str) -> String {
        use std::fmt::Write as _;
        let mut s = String::with_capacity(self.positions.len() * 90 + self.faces.len() * 40);
        let st = &self.stats;
        let _ = writeln!(s, "# Forme bake — {} faces ({} quads, {} triangles, {} n-gons), {} sommets, unités : mm", self.faces.len(), st.quads, st.tris, st.ngons, st.verts);
        let _ = writeln!(s, "# écart max à la surface exacte : {:.4} mm", st.max_dev);
        let _ = writeln!(s, "o {}", name);
        // vertex colour = material of the first face using the vertex
        let mut vmat = vec![u32::MAX; self.positions.len()];
        for f in &self.faces {
            for &v in &f.verts {
                if vmat[v as usize] == u32::MAX {
                    vmat[v as usize] = f.mat;
                }
            }
        }
        for (p, m) in self.positions.iter().zip(&vmat) {
            let c = materials.get(*m as usize).map(|m| m.color).unwrap_or([0.6, 0.6, 0.6]);
            let _ = writeln!(s, "v {:.5} {:.5} {:.5} {:.4} {:.4} {:.4}", p.x, p.y, p.z, c[0], c[1], c[2]);
        }
        let mut nmap: HashMap<[i32; 3], usize> = HashMap::new();
        let mut nlist: Vec<DVec3> = vec![];
        let mut fn_idx: Vec<Vec<usize>> = vec![];
        for f in &self.faces {
            fn_idx.push(
                f.normals
                    .iter()
                    .map(|n| {
                        let q = [(n.x * 10000.0).round() as i32, (n.y * 10000.0).round() as i32, (n.z * 10000.0).round() as i32];
                        *nmap.entry(q).or_insert_with(|| {
                            nlist.push(*n);
                            nlist.len()
                        })
                    })
                    .collect(),
            );
        }
        for n in &nlist {
            let _ = writeln!(s, "vn {:.4} {:.4} {:.4}", n.x, n.y, n.z);
        }
        let mut by_mat: Vec<usize> = (0..self.faces.len()).collect();
        by_mat.sort_by_key(|&i| self.faces[i].mat);
        let mut cur = u32::MAX;
        for i in by_mat {
            let f = &self.faces[i];
            if f.mat != cur {
                cur = f.mat;
                let name = materials.get(cur as usize).map(|m| m.name.clone()).unwrap_or(format!("mat{cur}"));
                let _ = writeln!(s, "g {}", name.replace(' ', "_"));
            }
            s.push('f');
            for (v, n) in f.verts.iter().zip(&fn_idx[i]) {
                let _ = write!(s, " {}//{}", v + 1, n);
            }
            s.push('\n');
        }
        s
    }
}

/// Convenience used by tests and the capture tool.
pub fn bake_blocking(scene: &Scene, q: Quality) -> Result<Baked, String> {
    bake(scene, &BakeSettings::preset(q), &Progress::default())
}
