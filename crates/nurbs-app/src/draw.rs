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
    /// The part was uploaded moved by a drag's preview.
    previewed: bool,
}

fn rgb(c: egui::Color32) -> [f32; 3] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

impl Drawer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Drawer {
        Drawer { r: Renderer::new(device, format), model_gen: u64::MAX, ref_gen: u64::MAX, quads_shown: false, flags: vec![], overlay: vec![], previewed: false }
    }

    fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &AppState) {
        // the reference mesh
        if self.ref_gen != st.ref_gen {
            self.ref_gen = st.ref_gen;
            self.r.upload_reference(device, queue, st.reference.as_ref().map(|r| r.verts.as_slice()).unwrap_or(&[]));
        }
        // the part
        let quads = st.quad_mesh();
        // whole shapes dragged in Object mode: their faces follow at once
        let preview = if quads.is_none() { st.drag_preview().filter(|p| !p.is_empty()) } else { None };
        if self.model_gen != st.model_gen || self.quads_shown != quads.is_some() || preview.is_some() || self.previewed {
            self.model_gen = st.model_gen;
            self.previewed = preview.is_some();
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
                // the dragged elements' faces and edges, moved
                let face_m: Vec<Option<glam::Mat4>> = match &preview {
                    Some(pv) => {
                        let els = element_map(st, m);
                        m.faces.iter().map(|f| f.tag.and_then(|t| pv.get(els.get(&t.feature).unwrap_or(&t.feature))).map(|x| x.as_mat4())).collect()
                    }
                    None => vec![],
                };
                if !face_m.is_empty() && dev_none(st) {
                    for v in verts.iter_mut() {
                        if let Some(Some(x)) = face_m.get(v.face as usize) {
                            v.pos = x.transform_point3(glam::Vec3::from(v.pos)).to_array();
                            v.nrm = x.inverse().transpose().transform_vector3(glam::Vec3::from(v.nrm)).normalize_or_zero().to_array();
                        }
                    }
                }
                // the edges a person draws: dark and thin like a CAD view
                let moved = |e: &nurbs_kernel::Edge| e.faces.iter().find_map(|f| usize::try_from(*f).ok().and_then(|f| face_m.get(f).copied().flatten()));
                let lines: Vec<Vec<[f32; 3]>> = m.edges.iter().filter(|e| !e.seam).map(|e| match moved(e) {
                    Some(x) => e.pts.iter().map(|p| x.transform_point3(glam::Vec3::from(*p)).to_array()).collect(),
                    None => e.pts.clone(),
                }).collect();
                line_vertices(&lines, [24, 26, 30, 210], 1.0, &mut edges);
            }
            self.r.upload_part(device, queue, &verts, &edges);
            self.flags.clear();
        }
        // the faces' material and state: in Face mode (and with the Inset /
        // Extrude tools) face by face, else by the feature they come from
        let by_face = face_mode(st);
        let picked = if by_face { st.picked_faces() } else { vec![] };
        // Object mode: a face lights with its element (the shape and all
        // its operations), a selected operation alone with its own faces
        let elements = st.model.as_ref().map(|m| element_map(st, m)).unwrap_or_default();
        let el = |t: nurbs_kernel::FaceTag| elements.get(&t.feature).copied().unwrap_or(t.feature);
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
                            Some(t) if st.selection.contains(&t.feature) || st.selection.contains(&el(t)) => FACE_SELECTED,
                            Some(t) if st.hover_feature == Some(t.feature) || st.hover_feature == Some(el(t)) => FACE_HOVERED,
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
        // a 2D boolean's operands dimmer (hidden with it), the boolean as
        // the kernel made it
        for f in st.doc.features.iter().filter(|f| f.visible && f.is_path()) {
            let selected = st.selection.contains(&f.id);
            let hovered = st.hover_feature == Some(f.id);
            let inside = st.curve_inside(f.id);
            if inside.is_some_and(|b| !b.visible) && !selected {
                continue;
            }
            let (c, a, w) = match (selected, hovered, inside) {
                (true, _, _) => (theme::accent(), 240, 2.2),
                (_, true, Some(_)) => (theme::PATH, 170, 1.6),
                (_, false, Some(b)) => (theme::PATH, if st.boolean_open(b) { 120 } else { 70 }, 1.2),
                (_, true, None) => (theme::PATH, 240, 2.2),
                _ => (theme::PATH, 240, 1.6),
            };
            let lines: Vec<Vec<[f32; 3]>> = st.curve_lines(f).iter().map(|l| l.iter().map(|p| p.as_vec3().to_array()).collect()).collect();
            line_vertices(&lines, [c.r(), c.g(), c.b(), a], w, &mut over);
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
        // the Slot drawn by hand: its outline on the plane
        if let Some(sd) = &st.slot_draw {
            let o: Vec<[f32; 3]> = sd.outline().iter().map(|p| p.as_vec3().to_array()).collect();
            let line = vec![sd.plane.at(sd.a).as_vec3().to_array(), sd.plane.at(sd.b).as_vec3().to_array()];
            line_vertices(&[o, line], [accent.r(), accent.g(), accent.b(), 235], 1.6, &mut over);
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
            // an Object-mode drag shows the faces themselves (no cage)
            let moving = moving && st.dragged_shapes().is_none();
            let show = creating || (selected && (moving || f.op == Op::Carve || !f.visible)) || (st.tool == Tool::Select && st.hover_feature == Some(f.id) && f.op == Op::Carve);
            if !show || (creating && !f.visible) {
                continue;
            }
            let c = if f.op == Op::Carve { carve } else { accent };
            let alpha = if selected || creating { 230 } else { 140 };
            let lines: Vec<Vec<[f32; 3]>> = f.outline().iter().map(|l| l.iter().map(|p| p.as_vec3().to_array()).collect()).collect();
            line_vertices(&lines, [c.r(), c.g(), c.b(), alpha], 1.2, &mut over);
        }
        // Object mode (Maya): the selected element drawn by all its edges in
        // the accent, its faces only lightly tinted; the one under the
        // pointer fainter. Components are never drawn in this mode.
        if object_mode(st) && (!st.selection.is_empty() || st.hover_feature.is_some()) {
            if let Some(m) = &st.model {
                let elements = element_map(st, m);
                let of = |fi: i32| -> Option<nurbs_kernel::FeatureId> {
                    let t = m.faces.get(usize::try_from(fi).ok()?)?.tag?;
                    Some(elements.get(&t.feature).copied().unwrap_or(t.feature))
                };
                // moved with the drag's preview, as the faces are
                let pv = st.drag_preview().unwrap_or_default();
                let place = |e: &nurbs_kernel::Edge, els: &[Option<nurbs_kernel::FeatureId>; 2]| -> Vec<[f32; 3]> {
                    match els.iter().flatten().find_map(|x| pv.get(x)) {
                        Some(x) => {
                            let x = x.as_mat4();
                            e.pts.iter().map(|p| x.transform_point3(glam::Vec3::from(*p)).to_array()).collect()
                        }
                        None => e.pts.clone(),
                    }
                };
                let (mut sel, mut hov) = (vec![], vec![]);
                for e in m.edges.iter().filter(|e| !e.seam) {
                    let els = [of(e.faces[0]), of(e.faces[1])];
                    if els.iter().flatten().any(|x| st.selection.contains(x)) {
                        sel.push(place(e, &els));
                    } else if st.hover_feature.map(|h| els.contains(&Some(h))).unwrap_or(false) {
                        hov.push(e.pts.clone());
                    }
                }
                let a = theme::accent();
                line_vertices(&sel, [a.r(), a.g(), a.b(), 240], 1.7, &mut over);
                line_vertices(&hov, [a.r(), a.g(), a.b(), 120], 1.4, &mut over);
            }
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
            sel: (sel, if object_mode(st) { 0.18 } else { 0.45 }),
            hover: (sel, if object_mode(st) { 0.08 } else { 0.2 }),
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

/// The part is drawn from the kernel's triangles (not the deviation's).
fn dev_none(st: &AppState) -> bool {
    st.deviation_shown().is_none()
}

/// Each feature whose faces the model shows, with its element.
pub fn element_map(st: &AppState, m: &nurbs_kernel::Model) -> std::collections::HashMap<nurbs_kernel::FeatureId, nurbs_kernel::FeatureId> {
    let mut out = std::collections::HashMap::new();
    for f in &m.faces {
        if let Some(t) = f.tag {
            out.entry(t.feature).or_insert_with(|| st.element_of(t.feature));
        }
    }
    out
}

/// Object mode with the Select tool: whole elements, Maya's look.
fn object_mode(st: &AppState) -> bool {
    st.tool == Tool::Select && st.sel_mode == SelMode::Object && st.push.is_none() && st.selected_push().is_none()
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
