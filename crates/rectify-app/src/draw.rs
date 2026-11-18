//! From the app state to the renderer: the mesh (all of it when opened, then
//! only the triangles an edit moved), the mesh as it came (Space), and what
//! the tool in hand draws over it.

use crate::render::{line_vertices, linear, FaceGpu, Frame, LineVertex, MeshVertex, Renderer};
use crate::state::AppState;
use crate::theme;
use glam::DVec3;
use rayon::prelude::*;
use rectify_core::mesh::{crease_normals, crease_normals_of};
use rectify_core::Mesh;

/// The mesh's colour: grey clay (sRGB).
pub const CLAY: [f32; 3] = [0.52, 0.52, 0.51];
/// Sharp where the surface turns more than this (degrees), else smooth.
pub const CREASE: f64 = 40.0;

pub struct Drawer {
    pub r: Renderer,
    gen: u64,
    overlay_gen: Option<u64>,
    faces_done: bool,
}

/// The triangles as the renderer takes them (three corners each).
fn mesh_vertices(m: &Mesh, normals: &[[DVec3; 3]]) -> Vec<MeshVertex> {
    (0..m.tris.len())
        .into_par_iter()
        .flat_map_iter(|t| {
            let tri = m.tris[t];
            let n = normals[t];
            (0..3).map(move |c| MeshVertex { pos: m.pos[tri[c] as usize].as_vec3().to_array(), nrm: n[c].as_vec3().to_array(), face: 0 })
        })
        .collect()
}

impl Drawer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Drawer {
        Drawer { r: Renderer::new(device, format), gen: 0, overlay_gen: None, faces_done: false }
    }

    fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &mut AppState) {
        let Some(d) = &st.doc else { return };
        if self.gen != st.gen {
            self.gen = st.gen;
            st.dirty.clear();
            let verts = mesh_vertices(&d.mesh, &crease_normals(&d.mesh, &d.topo, CREASE));
            self.r.upload_part(device, queue, &verts, &[]);
            let orig = mesh_vertices(&d.original, &crease_normals(&d.original, &d.topo, CREASE));
            self.r.upload_alt(device, queue, &orig);
            return;
        }
        if st.dirty.is_empty() {
            return;
        }
        // the triangles whose corners' normals changed: around every vertex
        // of a triangle that holds a moved vertex
        let m = &d.mesh;
        let topo = &d.topo;
        let mut near = vec![];
        for &v in &st.dirty {
            for &t in topo.tris_of(v as usize) {
                near.extend_from_slice(&m.tris[t as usize]);
            }
        }
        near.sort_unstable();
        near.dedup();
        let mut tris: Vec<u32> = near.iter().flat_map(|&v| topo.tris_of(v as usize).iter().copied()).collect();
        tris.sort_unstable();
        tris.dedup();
        st.dirty.clear();
        // written in runs (small gaps written again as they are)
        let mut runs: Vec<(u32, u32)> = vec![];
        for &t in &tris {
            match runs.last_mut() {
                Some(r) if t <= r.1 + 64 => r.1 = t,
                _ => runs.push((t, t)),
            }
        }
        for (a, b) in runs {
            let list: Vec<u32> = (a..=b).collect();
            let normals = crease_normals_of(m, topo, CREASE, &list);
            let verts: Vec<MeshVertex> = list
                .iter()
                .zip(&normals)
                .flat_map(|(&t, n)| {
                    let tri = m.tris[t as usize];
                    (0..3).map(move |c| MeshVertex { pos: m.pos[tri[c] as usize].as_vec3().to_array(), nrm: n[c].as_vec3().to_array(), face: 0 })
                })
                .collect();
            self.r.patch_part(queue, a * 3, &verts);
        }
    }

    /// What the tool draws over the mesh.
    fn sync_overlay(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &AppState) {
        if self.overlay_gen == Some(st.overlay_gen) {
            return;
        }
        self.overlay_gen = Some(st.overlay_gen);
        let mut out: Vec<LineVertex> = vec![];
        let p = |x: DVec3| x.as_vec3().to_array();
        if let Some(ld) = &st.line_drag {
            // the segment as drawn, thin and light
            line_vertices(&[vec![p(ld.a), p(ld.b)]], [235, 235, 235, 200], 0.9, &mut out);
            match &ld.fit {
                Some(Ok(f)) => {
                    line_vertices(&[vec![p(f.a), p(f.b)]], [255, 150, 60, 255], 2.0, &mut out);
                    for e in [f.a, f.b] {
                        line_vertices(&[cross(e, st)], [255, 150, 60, 255], 1.4, &mut out);
                    }
                }
                Some(Err(_)) => line_vertices(&[vec![p(ld.a), p(ld.b)]], [230, 70, 60, 255], 1.6, &mut out),
                None => {}
            }
        }
        self.r.upload_overlay(device, queue, &out);
    }

    /// Draws the view and the UI into `target` (`size` pixels, `ppp` pixels per point).
    #[allow(clippy::too_many_arguments)]
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, size: [u32; 2], ppp: f32, st: &mut AppState, paint: &[egui::ClippedPrimitive], textures: &mut egui::TexturesDelta) {
        self.sync(device, queue, st);
        self.sync_overlay(device, queue, st);
        if !self.faces_done {
            self.r.upload_faces(device, queue, &[FaceGpu { color: linear(CLAY), flags: 0, metal: 0.0, rough: 0.55, pad: [0.0; 2] }]);
            self.faces_done = true;
        }
        let a = theme::accent();
        let sel = [a.r() as f32 / 255.0, a.g() as f32 / 255.0, a.b() as f32 / 255.0];
        let frame = Frame {
            view: st.view().scaled(ppp),
            points_to_px: ppp,
            light: st.settings.light,
            grid: st.settings.grid,
            origin: false,
            zebra: st.settings.zebra,
            base: CLAY,
            alt: st.compare,
            sel: (sel, 0.6),
            hover: (sel, 0.25),
            paint,
            textures,
        };
        self.r.render(device, queue, target, size, frame);
    }
}

/// A small cross at a point, square to the view (a few points wide).
fn cross(at: DVec3, st: &AppState) -> Vec<[f32; 3]> {
    let v = st.view();
    let s = v.pixel_size_at((at.as_vec3() - v.eye).length()) as f64 * 5.0;
    let up = v.basis().2.as_dvec3();
    vec![(at - up * s).as_vec3().to_array(), (at + up * s).as_vec3().to_array()]
}
