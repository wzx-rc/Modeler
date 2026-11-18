//! From the app state to the renderer: the mesh of the current view (the
//! original, the regions in colour, or the result), the lines where regions
//! meet, and each region's colour and state.

use crate::render::{line_vertices, linear, FaceGpu, Frame, LineVertex, MeshVertex, Renderer, FACE_SELECTED};
use crate::state::{AppState, ViewMode};
use crate::theme;
use rayon::prelude::*;
use rectify_core::snap::crease_normals;
use rectify_core::{Kind, NONE};

/// The mesh's colour: grey clay (sRGB).
pub const CLAY: [f32; 3] = [0.52, 0.52, 0.51];
/// Faces left free (no shape) in the Regions view.
pub const FREE: [f32; 3] = [0.20, 0.20, 0.21];

pub struct Drawer {
    pub r: Renderer,
    gen: u64,
    mode: Option<ViewMode>,
    flags: Vec<FaceGpu>,
}

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// A region's colour: planes in cool hues, cylinders warm, spheres pink;
/// neighbours told apart by a golden-ratio walk.
pub fn region_color(i: u32, k: Kind) -> [f32; 3] {
    let g = (i as f32 * 0.618_034).fract();
    let (h, s, v) = match k {
        Kind::Plane => (0.47 + g * 0.22, 0.35 + 0.25 * ((i * 7 % 5) as f32 / 4.0), 0.62 + 0.28 * ((i * 3 % 4) as f32 / 3.0)),
        Kind::Cylinder => (0.0 + g * 0.14, 0.55 + 0.25 * ((i * 7 % 5) as f32 / 4.0), 0.75 + 0.2 * ((i * 3 % 4) as f32 / 3.0)),
        Kind::Sphere => (0.86 + g * 0.08, 0.45, 0.85),
    };
    hsv(h, s, v)
}

impl Drawer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Drawer {
        Drawer { r: Renderer::new(device, format), gen: 0, mode: None, flags: vec![] }
    }

    fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &AppState) {
        // while a step runs the pipeline is away: what is uploaded stays
        if (self.gen == st.gen && self.mode == Some(st.shown())) || st.pipe.is_none() {
            return;
        }
        self.gen = st.gen;
        self.mode = Some(st.shown());
        let p = st.pipe.as_ref().unwrap();
        let mode = st.shown();
        let m = st.shown_mesh().unwrap();
        let seg = st.shown_seg();
        let label: Option<&[u32]> = seg.map(|s| s.label.as_slice());
        // the same in every mode, so that they compare: sharp where the
        // surface turns more than 40°, else smooth
        let normals = crease_normals(m, &p.topo, 40.0);
        let face_of = |t: usize| -> u32 {
            match (mode, label) {
                (ViewMode::Original, _) | (_, None) => 0,
                (_, Some(l)) => {
                    if l[t] == NONE {
                        1
                    } else {
                        l[t] + 2
                    }
                }
            }
        };
        let verts: Vec<MeshVertex> = (0..m.tris.len())
            .into_par_iter()
            .flat_map_iter(|t| {
                let tri = m.tris[t];
                let f = face_of(t);
                let n = normals[t];
                (0..3).map(move |c| MeshVertex { pos: m.pos[tri[c] as usize].as_vec3().to_array(), nrm: n[c].as_vec3().to_array(), face: f })
            })
            .collect();
        // the lines where regions meet
        let mut edges = vec![];
        if let (Some(l), true) = (label, st.settings.edges && mode != ViewMode::Original) {
            let lines: Vec<Vec<[f32; 3]>> = (0..m.tris.len())
                .into_par_iter()
                .flat_map_iter(|t| {
                    let tri = m.tris[t];
                    let adj = p.topo.adj[t];
                    (0..3).filter_map(move |k| {
                        let o = adj[k];
                        if o == NONE || (o as usize) < t || l[o as usize] == l[t] {
                            return None;
                        }
                        Some(vec![m.pos[tri[k] as usize].as_vec3().to_array(), m.pos[tri[(k + 1) % 3] as usize].as_vec3().to_array()])
                    })
                })
                .collect();
            let col = if mode == ViewMode::Regions { [12, 13, 16, 200] } else { [20, 22, 26, 170] };
            line_vertices(&lines, col, 0.9, &mut edges);
        }
        self.r.upload_part(device, queue, &verts, &edges);
        self.r.upload_overlay(device, queue, &[] as &[LineVertex]);
    }

    /// Each region's colour and state (cheap: compared every frame).
    fn sync_flags(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &AppState) {
        if st.pipe.is_none() {
            return;
        }
        let mode = st.shown();
        let seg = st.shown_seg();
        // colours: 0 clay, 1 free, 2.. the regions
        let mut flags = vec![FaceGpu { color: linear(CLAY), flags: 0, metal: 0.0, rough: 0.55, pad: [0.0; 2] }];
        flags.push(FaceGpu { color: linear(if mode == ViewMode::Regions { FREE } else { CLAY }), flags: 0, metal: 0.0, rough: 0.55, pad: [0.0; 2] });
        if let Some(s) = seg {
            for (i, r) in s.regions.iter().enumerate() {
                let color = if mode == ViewMode::Regions { region_color(i as u32, r.prim.kind()) } else { CLAY };
                let sel = st.selected == Some(i as u32);
                flags.push(FaceGpu { color: linear(color), flags: if sel { FACE_SELECTED } else { 0 }, metal: 0.0, rough: 0.55, pad: [0.0; 2] });
            }
        }
        if flags != self.flags {
            self.r.upload_faces(device, queue, &flags);
            self.flags = flags;
        }
    }

    /// Draws the view and the UI into `target` (`size` pixels, `ppp` pixels per point).
    #[allow(clippy::too_many_arguments)]
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, size: [u32; 2], ppp: f32, st: &AppState, paint: &[egui::ClippedPrimitive], textures: &mut egui::TexturesDelta) {
        self.sync(device, queue, st);
        self.sync_flags(device, queue, st);
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
