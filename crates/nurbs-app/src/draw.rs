//! From the app state to the renderer: the part's mesh and edges (uploaded
//! when a new model arrives), the faces' state (selection, pre-selection),
//! the outlines over the part.

use crate::materials;
use crate::render::{line_vertices, linear, FaceGpu, Frame, LineVertex, MeshVertex, RefDraw, Renderer, FACE_HOVERED, FACE_SELECTED, NO_DEV};
use crate::state::{AppState, SelMode, Tool};
use crate::theme;
use nurbs_kernel::Op;

/// The part's colour: grey clay (sRGB).
pub const CLAY: [f32; 3] = [0.50, 0.50, 0.49];
/// The reference mesh: a cooler, darker grey than the part (sRGB).
pub const REF_GREY: [f32; 3] = [0.30, 0.315, 0.345];
/// Quads view: the fillet and chamfer bands (sRGB).
pub const BAND: [f32; 3] = [0.92, 0.47, 0.15];

pub struct Drawer {
    pub r: Renderer,
    model_gen: u64,
    ref_gen: u64,
    /// The part was uploaded as quads.
    quads_shown: bool,
    flags: Vec<FaceGpu>,
    overlay: Vec<LineVertex>,
}

fn rgb(c: egui::Color32) -> [f32; 3] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

impl Drawer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Drawer {
        Drawer { r: Renderer::new(device, format), model_gen: u64::MAX, ref_gen: u64::MAX, quads_shown: false, flags: vec![], overlay: vec![] }
    }

    fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &AppState) {
        // the reference mesh
        if self.ref_gen != st.ref_gen {
            self.ref_gen = st.ref_gen;
            self.r.upload_reference(device, queue, st.reference.as_ref().map(|r| r.verts.as_slice()).unwrap_or(&[]));
        }
        // the part
        let quads = st.quad_mesh();
        if self.model_gen != st.model_gen || self.quads_shown != quads.is_some() {
            self.model_gen = st.model_gen;
            self.quads_shown = quads.is_some();
            let mut verts = vec![];
            let mut edges = vec![];
            if let Some(m) = &st.model {
                // the faces the quads leave out are drawn from the triangles
                let left: std::collections::HashSet<u32> = match quads {
                    Some(q) => q.skipped.iter().map(|(f, _)| *f).collect(),
                    None => (0..m.faces.len() as u32).collect(),
                };
                // the distance to the reference, when shown and measured on
                // this model: its finer triangles instead
                let dev = st.deviation_shown().filter(|_| quads.is_none());
                if let Some(d) = dev {
                    verts.extend_from_slice(d);
                } else {
                    verts.reserve(m.triangles.len() * 3);
                    for (t, tri) in m.triangles.iter().enumerate() {
                        let face = m.tri_face[t];
                        if !left.contains(&face) {
                            continue;
                        }
                        for &v in tri {
                            verts.push(MeshVertex { pos: m.positions[v as usize], nrm: m.normals[v as usize], face, dev: NO_DEV });
                        }
                    }
                }
                if let Some(q) = quads {
                    quad_vertices(q, &mut verts);
                    // the quads' wire, light over the matter
                    let mut seen = std::collections::HashSet::new();
                    let mut wire = vec![];
                    for k in &q.quads {
                        for i in 0..4 {
                            let (a, b) = (k[i], k[(i + 1) % 4]);
                            if seen.insert((a.min(b), a.max(b))) {
                                wire.push(vec![q.positions[a as usize], q.positions[b as usize]]);
                            }
                        }
                    }
                    line_vertices(&wire, [16, 18, 22, 150], 0.8, &mut edges);
                }
                // the edges a person draws: dark and thin like a CAD view
                let lines: Vec<Vec<[f32; 3]>> = m.edges.iter().filter(|e| !e.seam).map(|e| e.pts.clone()).collect();
                line_vertices(&lines, [24, 26, 30, 210], 1.0, &mut edges);
            }
            self.r.upload_part(device, queue, &verts, &edges);
            self.flags.clear();
        }
        // the faces' material and state: in Face mode (and with the Inset /
        // Extrude tools) face by face, else by the feature they come from
        let by_face = face_mode(st);
        let picked = if by_face { st.picked_faces() } else { vec![] };
        let flags: Vec<FaceGpu> = match &st.model {
            Some(m) => m
                .faces
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    // a material ball dragged over it: shown on it already
                    if let Some(d) = st.mat_drag.as_ref().filter(|d| d.target.map(|t| st.face_in_target(i, &t)).unwrap_or(false)) {
                        let mat = materials::get(d.material);
                        return FaceGpu { color: linear(mat.color), flags: 0, metal: mat.metal, rough: mat.rough, pad: [0.0; 2] };
                    }
                    let flags = if by_face {
                        if picked.contains(&i) || f.tag.map(|t| st.selection.contains(&t.feature)).unwrap_or(false) {
                            FACE_SELECTED
                        } else if st.hover_face == Some(i) && st.drag.is_none() && st.hot_handle.is_none() {
                            FACE_HOVERED
                        } else {
                            0
                        }
                    } else {
                        match f.tag {
                            Some(t) if st.selection.contains(&t.feature) => FACE_SELECTED,
                            Some(t) if st.hover_feature == Some(t.feature) => FACE_HOVERED,
                            _ => 0,
                        }
                    };
                    let sheet = f.tag.and_then(|t| st.doc.feature(t.feature)).map(|x| x.is_sheet()).unwrap_or(false);
                    let flags = if sheet { flags | crate::render::FACE_SHEET } else { flags };
                    let mat = materials::get(st.face_material(i));
                    // Quads view: the fillet and chamfer bands in orange
                    let band = self.quads_shown && f.tag.and_then(|t| st.doc.feature(t.feature)).map(|x| x.round.is_some()).unwrap_or(false);
                    if band {
                        return FaceGpu { color: linear(BAND), flags, metal: 0.0, rough: 0.6, pad: [0.0; 2] };
                    }
                    FaceGpu { color: linear(mat.color), flags, metal: mat.metal, rough: mat.rough, pad: [0.0; 2] }
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
        // paths: always drawn (they have no faces), white when selected
        for f in st.doc.features.iter().filter(|f| f.visible && f.is_path()) {
            let selected = st.selection.contains(&f.id);
            let hovered = st.hover_feature == Some(f.id);
            let (c, w) = if selected { (theme::accent(), 2.2) } else if hovered { (theme::PATH, 2.2) } else { (theme::PATH, 1.6) };
            let lines: Vec<Vec<[f32; 3]>> = f.outline().iter().map(|l| l.iter().map(|p| p.as_vec3().to_array()).collect()).collect();
            line_vertices(&lines, [c.r(), c.g(), c.b(), 240], w, &mut over);
        }
        // the Slot dragged along a slot of the reference: the drag, and a
        // tick across each end on the scan's face
        if let Some(ss) = &st.scan_slot {
            let e = ss.b - ss.a;
            let across = ss.n.cross(e).normalize_or_zero() * (e.length() * 0.12).max(0.3);
            let lines = vec![
                vec![ss.a.as_vec3().to_array(), ss.b.as_vec3().to_array()],
                vec![(ss.a - across).as_vec3().to_array(), (ss.a + across).as_vec3().to_array()],
                vec![(ss.b - across).as_vec3().to_array(), (ss.b + across).as_vec3().to_array()],
            ];
            line_vertices(&lines, [accent.r(), accent.g(), accent.b(), 235], 1.8, &mut over);
        }
        // the Cylinder dragged across a round part of the reference
        if let Some(sr) = &st.scan_round {
            let lines = vec![sr.ring(96).iter().map(|p| p.as_vec3().to_array()).collect::<Vec<_>>(), vec![sr.a.as_vec3().to_array(), sr.b.as_vec3().to_array()]];
            line_vertices(&lines, [accent.r(), accent.g(), accent.b(), 235], 1.8, &mut over);
        }
        // the arcs read on the scan for the selected fillet (Fit to scan)
        if let Some(fs) = st.fillet_show.as_ref().filter(|fs| st.selection == [fs.id] && st.doc.feature(fs.id).is_some()) {
            let lines: Vec<Vec<[f32; 3]>> = fs.arcs.iter().map(|a| a.iter().map(|p| p.as_vec3().to_array()).collect()).collect();
            line_vertices(&lines, [accent.r(), accent.g(), accent.b(), 235], 1.8, &mut over);
        }
        for f in st.doc.features.iter().filter(|f| f.is_solid() && !f.is_path()) {
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
        // faces picked or being worked on: green, like their mode
        let sel = if face_mode(st) || st.push.is_some() || st.selected_push().is_some() { rgb(theme::FACE_SEL) } else { rgb(theme::accent()) };
        let frame = Frame {
            view: st.view().scaled(ppp),
            points_to_px: ppp,
            light: st.settings.light,
            grid: st.settings.grid,
            origin: st.settings.origin,
            base: CLAY,
            sel: (sel, 0.45),
            hover: (sel, 0.2),
            reference: match (st.reference.is_some(), st.ref_shown, st.ref_ghost) {
                (true, true, false) => RefDraw::Solid,
                (true, true, true) => RefDraw::Ghost,
                _ => RefDraw::Hidden,
            },
            ref_color: REF_GREY,
            xray: st.xray,
            deviation: st.deviation_shown().map(|_| (st.dev_range as f32, st.dev_tol as f32)),
            paint,
            textures,
        };
        self.r.render(device, queue, target, size, frame);
    }
}

/// Faces light up one by one: Face mode, or the Inset / Extrude tool waiting
/// for a press (not while one is under way: then its new faces light up, as
/// the selected step's do).
fn face_mode(st: &AppState) -> bool {
    st.push.is_none() && ((st.tool == Tool::Select && st.sel_mode == SelMode::Face) || matches!(st.tool, Tool::Push(_)))
}

/// The quads as triangles, each corner with the normal of the quads around
/// it on the same face (smooth inside a face, sharp between faces).
fn quad_vertices(q: &nurbs_kernel::Quads, out: &mut Vec<MeshVertex>) {
    let p = |i: u32| glam::Vec3::from(q.positions[i as usize]);
    let mut sums: std::collections::HashMap<(u32, u32), glam::Vec3> = std::collections::HashMap::new();
    for (k, f) in q.quads.iter().zip(&q.quad_face) {
        let n = (p(k[2]) - p(k[0])).cross(p(k[3]) - p(k[1]));
        for v in k {
            *sums.entry((*f, *v)).or_default() += n;
        }
    }
    out.reserve(q.quads.len() * 6);
    for (k, f) in q.quads.iter().zip(&q.quad_face) {
        for i in [0, 1, 2, 0, 2, 3] {
            let v = k[i];
            let n = sums[&(*f, v)].normalize_or_zero();
            out.push(MeshVertex { pos: q.positions[v as usize], nrm: n.to_array(), face: *f, dev: NO_DEV });
        }
    }
}
