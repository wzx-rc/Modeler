//! From the app state to the renderer: the part's mesh and edges (uploaded
//! when a new model arrives), the faces' state (selection, pre-selection),
//! the outlines over the part.

use crate::render::{line_vertices, Frame, LineVertex, MeshVertex, Renderer, FACE_HOVERED, FACE_SELECTED};
use crate::state::{AppState, SelMode, Tool};
use crate::theme;
use nurbs_kernel::Op;

/// The part's colour: grey clay (sRGB).
pub const CLAY: [f32; 3] = [0.50, 0.50, 0.49];

pub struct Drawer {
    pub r: Renderer,
    model_gen: u64,
    flags: Vec<u32>,
    overlay: Vec<LineVertex>,
}

fn rgb(c: egui::Color32) -> [f32; 3] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

impl Drawer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Drawer {
        Drawer { r: Renderer::new(device, format), model_gen: u64::MAX, flags: vec![], overlay: vec![] }
    }

    fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &AppState) {
        // the part
        if self.model_gen != st.model_gen {
            self.model_gen = st.model_gen;
            let mut verts = vec![];
            let mut edges = vec![];
            if let Some(m) = &st.model {
                verts.reserve(m.triangles.len() * 3);
                for (t, tri) in m.triangles.iter().enumerate() {
                    let face = m.tri_face[t];
                    for &v in tri {
                        verts.push(MeshVertex { pos: m.positions[v as usize], nrm: m.normals[v as usize], face });
                    }
                }
                // the edges a person draws: dark and thin like a CAD view
                let lines: Vec<Vec<[f32; 3]>> = m.edges.iter().filter(|e| !e.seam).map(|e| e.pts.clone()).collect();
                line_vertices(&lines, [24, 26, 30, 210], 1.0, &mut edges);
            }
            self.r.upload_part(device, queue, &verts, &edges);
            self.flags.clear();
        }
        // the faces' state
        let flags: Vec<u32> = match &st.model {
            Some(m) => m
                .faces
                .iter()
                .map(|f| match f.tag {
                    Some(t) if st.selection.contains(&t.feature) => FACE_SELECTED,
                    Some(t) if st.hover_feature == Some(t.feature) => FACE_HOVERED,
                    _ => 0,
                })
                .collect(),
            None => vec![],
        };
        if flags != self.flags || flags.is_empty() {
            self.r.upload_faces(device, queue, &flags);
            self.flags = flags;
        }
        // outlines: the shape being created, the shapes being moved, the
        // selected cutters (their faces are the part's holes)
        let mut over = vec![];
        let carve = theme::carve();
        let accent = theme::accent();
        let moving = st.drag.is_some();
        for f in st.doc.features.iter().filter(|f| f.is_solid()) {
            let creating = st.creation.as_ref().map(|c| c.id == f.id).unwrap_or(false);
            let selected = st.selection.contains(&f.id);
            let show = creating || (selected && (moving || f.op == Op::Carve || !f.visible)) || (st.tool == Tool::Select && st.hover_feature == Some(f.id) && f.op == Op::Carve);
            if !show || (creating && !f.visible) {
                continue;
            }
            let c = if f.op == Op::Carve { carve } else { accent };
            let alpha = if selected || creating { 230 } else { 140 };
            let lines: Vec<Vec<[f32; 3]>> = f.outline().iter().map(|l| l.iter().map(|p| p.as_vec3().to_array()).collect()).collect();
            line_vertices(&lines, [c.r(), c.g(), c.b(), alpha], 1.2, &mut over);
        }
        // Edge mode: the picked edges, whole, and the one under the pointer
        if st.sel_mode == SelMode::Edge {
            if let Some(m) = &st.model {
                let y = theme::EDGE_SEL;
                let picked: Vec<usize> = st.edge_sel.iter().filter_map(|r| st.model_edge_of(r)).collect();
                let lines: Vec<Vec<[f32; 3]>> = picked.iter().map(|i| m.edges[*i].pts.clone()).collect();
                line_vertices(&lines, [y.r(), y.g(), y.b(), 235], 1.6, &mut over);
                if let Some(i) = st.hover_edge.filter(|i| !picked.contains(i)) {
                    line_vertices(&[m.edges[i].pts.clone()], [y.r(), y.g(), y.b(), 150], 1.6, &mut over);
                }
            }
        }
        if over.len() != self.overlay.len() || bytemuck::cast_slice::<_, u8>(&over) != bytemuck::cast_slice::<_, u8>(&self.overlay) {
            self.r.upload_overlay(device, queue, &over);
            self.overlay = over;
        }
    }

    /// Draws the view and the UI into `target` (`size` pixels, `ppp` pixels per point).
    #[allow(clippy::too_many_arguments)]
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, size: [u32; 2], ppp: f32, st: &AppState, paint: &[egui::ClippedPrimitive], textures: &mut egui::TexturesDelta) {
        self.sync(device, queue, st);
        let sel = rgb(theme::accent());
        let frame = Frame {
            view: st.view().scaled(ppp),
            points_to_px: ppp,
            light: st.settings.light,
            grid: st.settings.grid,
            origin: st.settings.origin,
            base: CLAY,
            sel: (sel, 0.45),
            hover: (sel, 0.2),
            paint,
            textures,
        };
        self.r.render(device, queue, target, size, frame);
    }
}
