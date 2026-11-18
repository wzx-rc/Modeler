//! From the app state to the renderer: the scan (uploaded when opened), and
//! what the tool in hand draws over it.

use crate::render::{linear, FaceGpu, Frame, LineVertex, MeshVertex, Renderer};
use crate::state::AppState;
use crate::theme;
use glam::DVec3;
use rayon::prelude::*;
use rectify_core::mesh::crease_normals;
use rectify_core::Mesh;

/// The scan's colour: grey clay (sRGB).
pub const CLAY: [f32; 3] = [0.52, 0.52, 0.51];
/// Sharp where the surface turns more than this (degrees), else smooth.
pub const CREASE: f64 = 40.0;

pub struct Drawer {
    pub r: Renderer,
    gen: u64,
    overlay_gen: Option<(u64, bool)>,
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
            let verts = mesh_vertices(&d.mesh, &crease_normals(&d.mesh, &d.topo, CREASE));
            self.r.upload_part(device, queue, &verts, &[]);
        }
    }

    /// What the tool draws over the scan.
    fn sync_overlay(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &AppState) {
        if self.overlay_gen == Some((st.overlay_gen, st.bare)) {
            return;
        }
        self.overlay_gen = Some((st.overlay_gen, st.bare));
        let out: Vec<LineVertex> = vec![];
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
            sel: (sel, 0.6),
            hover: (sel, 0.25),
            paint,
            textures,
        };
        self.r.render(device, queue, target, size, frame);
    }
}
