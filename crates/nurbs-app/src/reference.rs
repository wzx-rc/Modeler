//! The reference mesh (retopology on a mesh, asked 19 nov. 2026, 14 h 23):
//! a scan or an AI-made mesh (.obj) shown grey in the view to model on, in
//! its own coordinates (millimetres, Y up; a file in centimetres is scaled),
//! never changed. The document keeps its path (`Doc.reference`); the app
//! loads it in a thread (the test pistol, 1.95 M triangles, takes seconds)
//! with a BVH (`rectify_core::bvh`) for the pointer's ray.

use crate::render::MeshVertex;
use glam::DVec3;
use rectify_core::bvh::Bvh;
use rectify_core::mesh::{crease_normals, Mesh, Topo};
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::Instant;

/// The face index the reference's vertices carry: the shader draws them
/// with the reference's colour, not a face of the part.
pub const REF_FACE: u32 = u32::MAX;
/// Shading: sharp where the surface turns more than this (degrees), else
/// smooth (as in Forme Rectify).
const CREASE: f64 = 40.0;

pub struct Reference {
    /// The file, as the document names it.
    pub path: String,
    /// Its name without folder or extension.
    pub name: String,
    pub mesh: Mesh,
    /// Its adjacency (the tools walk the scan from the triangle pressed).
    pub topo: Topo,
    pub bvh: Bvh,
    /// The median edge length (mm): the scan's grain.
    pub edge: f64,
    pub lo: DVec3,
    pub hi: DVec3,
    /// The triangles' corners for the renderer (normals sharp past 40°).
    pub verts: Vec<MeshVertex>,
    /// How long it took to load (s).
    pub secs: f32,
}

/// A load under way: the path and the thread's answer.
pub struct Loading {
    pub path: String,
    pub rx: Receiver<Result<Reference, String>>,
}

impl Reference {
    /// Reads the file and builds what the view and the tools need (seconds
    /// for a big scan: call it from a thread, `start`).
    pub fn load(path: &str) -> Result<Reference, String> {
        let t0 = Instant::now();
        let mesh = Mesh::load_obj(Path::new(path))?;
        if mesh.tris.is_empty() {
            return Err(format!("{}: no triangles", name_of(path)));
        }
        let (bvh, topo, verts) = std::thread::scope(|s| {
            let b = s.spawn(|| Bvh::new(&mesh));
            let topo = Topo::new(&mesh);
            let normals = crease_normals(&mesh, &topo, CREASE);
            let mut verts = Vec::with_capacity(mesh.tris.len() * 3);
            for (t, tri) in mesh.tris.iter().enumerate() {
                for c in 0..3 {
                    verts.push(MeshVertex { pos: mesh.pos[tri[c] as usize].as_vec3().to_array(), nrm: normals[t][c].as_vec3().to_array(), face: REF_FACE });
                }
            }
            (b.join().expect("bvh"), topo, verts)
        });
        let (lo, hi) = mesh.bounds();
        let edge = mesh.edge_median();
        Ok(Reference { path: path.to_string(), name: name_of(path), mesh, topo, bvh, edge, lo, hi, verts, secs: t0.elapsed().as_secs_f32() })
    }

    /// Loads it in a thread.
    pub fn start(path: &str) -> Loading {
        let (tx, rx) = std::sync::mpsc::channel();
        let p = path.to_string();
        std::thread::spawn(move || {
            let _ = tx.send(Reference::load(&p));
        });
        Loading { path: path.to_string(), rx }
    }

    /// The nearest point where a ray meets it, and its distance along `d`
    /// (a unit vector).
    pub fn hit(&self, o: DVec3, d: DVec3) -> Option<(DVec3, f64)> {
        self.hit_tri(o, d).map(|(_, p, s)| (p, s))
    }

    /// The same with the triangle met.
    pub fn hit_tri(&self, o: DVec3, d: DVec3) -> Option<(u32, DVec3, f64)> {
        self.bvh.ray(&self.mesh, o, d).map(|(t, s)| (t, o + d * s, s))
    }

    pub fn triangles(&self) -> usize {
        self.mesh.tris.len()
    }
}

/// "GUN_A_ORIGIN" from ".../GUN_A_ORIGIN.obj".
pub fn name_of(path: &str) -> String {
    Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/// "1.95 M", "48.2 k", "512".
pub fn short_count(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.2} M", n as f64 / 1e6)
    } else if n >= 10_000 {
        format!("{:.1} k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}
