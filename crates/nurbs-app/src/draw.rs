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
    /// The part was uploaded with a form drawn from its poles.
    previewed: bool,
    /// The drag's preview the part was uploaded with (`preview_key`).
    preview_key: Option<u64>,
    /// Each face's matrix in that preview (none: empty).
    face_m: Vec<Option<glam::Mat4>>,
}

fn rgb(c: egui::Color32) -> [f32; 3] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

impl Drawer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Drawer {
        Drawer { r: Renderer::new(device, format), model_gen: u64::MAX, ref_gen: u64::MAX, quads_shown: false, flags: vec![], overlay: vec![], previewed: false, preview_key: None, face_m: vec![] }
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
        // a form pulled by a brush: drawn from its poles (`forms.rs`)
        let form = if quads.is_none() { st.form_preview() } else { None };
        // (a preview that did not change since it was uploaded, the pointer
        // held still or the same grid step: nothing to upload again; on a
        // heavy part that is every vertex, each frame)
        let pushes = if quads.is_none() { st.push_preview() } else { None };
        let pkey = match (preview.as_ref().map(preview_key), &pushes) {
            (k, None) => k,
            (k, Some(pp)) => Some(push_key(k.unwrap_or(0), pp)),
        };
        if self.model_gen != st.model_gen || self.quads_shown != quads.is_some() || pkey != self.preview_key || form.is_some() || self.previewed {
            self.model_gen = st.model_gen;
            self.previewed = form.is_some();
            self.preview_key = pkey;
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
                let dev = st.deviation_shown().filter(|_| quads.is_none() && form.is_none());
                // the form's own faces: drawn from its poles instead
                let form_faces: std::collections::HashSet<u32> = match &form {
                    Some((id, _)) => m.faces.iter().enumerate().filter(|(_, f)| f.tag.map(|t| t.feature == *id).unwrap_or(false)).map(|(i, _)| i as u32).collect(),
                    None => Default::default(),
                };
                if let Some(d) = dev {
                    verts.extend_from_slice(d);
                } else {
                    verts.reserve(m.triangles.len() * 3);
                    for (t, tri) in m.triangles.iter().enumerate() {
                        let face = m.tri_face[t];
                        if !left.contains(&face) || form_faces.contains(&face) {
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
                    Some(pv) => preview_faces(st, m, pv),
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
                // extrusions whose depth changes: their ends moved, their
                // walls stretched
                let push_m = match &pushes {
                    Some(pp) if dev_none(st) => Some(push_moves(m, pp)).filter(|x| !x.is_empty()),
                    _ => None,
                };
                if let Some(x) = &push_m {
                    for v in verts.iter_mut() {
                        if let Some(p) = x.apply(v.face as usize, v.pos) {
                            v.pos = p;
                        }
                    }
                }
                if let (Some((_, tris)), Some(face)) = (&form, form_faces.iter().min()) {
                    verts.extend(tris.iter().map(|(p, n)| MeshVertex { pos: *p, nrm: *n, face: *face, dev: NO_DEV }));
                }
                // the edges a person draws: dark and thin like a CAD view
                let moved = |e: &nurbs_kernel::Edge| e.faces.iter().find_map(|f| usize::try_from(*f).ok().and_then(|f| face_m.get(f).copied().flatten()));
                let on_form = |e: &nurbs_kernel::Edge| e.faces.iter().any(|f| u32::try_from(*f).map(|f| form_faces.contains(&f)).unwrap_or(false));
                let pushed = |e: &nurbs_kernel::Edge| push_m.as_ref().and_then(|x| e.faces.iter().find_map(|f| usize::try_from(*f).ok().filter(|f| x.moves(*f))).map(|f| (x, f)));
                let lines: Vec<Vec<[f32; 3]>> = m.edges.iter().filter(|e| !e.seam && !on_form(e)).map(|e| match (moved(e), pushed(e)) {
                    (Some(x), _) => e.pts.iter().map(|p| x.transform_point3(glam::Vec3::from(*p)).to_array()).collect(),
                    (None, Some((x, f))) => e.pts.iter().map(|p| x.apply(f, *p).unwrap_or(*p)).collect(),
                    (None, None) => e.pts.clone(),
                }).collect();
                line_vertices(&lines, [24, 26, 30, 210], 1.0, &mut edges);
                self.face_m = face_m;
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
                        return face_gpu(&st.look(d.material), 0);
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
                            Some(t) if st.hover_feature == Some(t.feature) || st.hover_feature == Some(el(t)) || st.hover_also.contains(&el(t)) => FACE_HOVERED,
                            _ => 0,
                        }
                    };
                    let sheet = f.tag.and_then(|t| st.doc.feature(t.feature)).map(|x| st.doc.makes(x) == nurbs_kernel::Makes::Sheet).unwrap_or(false);
                    let flags = if sheet { flags | crate::render::FACE_SHEET } else { flags };
                    let mat = st.look(st.face_material(i));
                    // Quads view: the fillet and chamfer bands in orange
                    let band = self.quads_shown && f.tag.and_then(|t| st.doc.feature(t.feature)).map(|x| x.round.is_some()).unwrap_or(false);
                    if band {
                        return FaceGpu { color: linear(BAND), flags, metal: 0.0, rough: 0.6, spec: materials::SPECULAR, alpha: 1.0 };
                    }
                    face_gpu(&mat, flags)
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
        // paths: always drawn (they have no faces), white when selected;
        // a 2D boolean as the kernel made it, its curves hidden (Romain 20
        // nov., 9 h 25: "n'avoir que le résultat de visible") while none of
        // them is picked; the boolean selected, the one under the pointer
        // shows (a click picks it)
        for f in st.doc.features.iter().filter(|f| f.visible && f.is_path()) {
            let selected = st.selection.contains(&f.id);
            let hovered = st.hover_feature == Some(f.id) || st.hover_also.contains(&f.id);
            let inside = st.curve_inside(f.id);
            let shown = |b: &nurbs_kernel::Feature| {
                b.visible && (b.shape_bool.as_ref().is_some_and(|s| s.operands.iter().any(|o| st.selection.contains(o))) || (hovered && st.boolean_open(b)))
            };
            if inside.is_some_and(|b| !shown(b)) && !selected {
                continue;
            }
            // a curve of a 2D boolean being moved, turned or scaled: hidden
            // again, only the result shows (Romain 20 nov., 9 h 45)
            if inside.is_some() && st.drag.is_some() {
                continue;
            }
            // the curve under the pointer (pre-selection, Maya): lighter
            // and wider
            let pre = egui::Color32::from_rgb(0xd8, 0xf6, 0xfa);
            let (c, a, w) = match (selected, hovered, inside) {
                (true, _, _) => (theme::accent(), 240, 2.2),
                (_, true, Some(_)) => (pre, 200, 2.0),
                (_, false, Some(b)) => (theme::PATH, if st.boolean_open(b) { 120 } else { 70 }, 1.2),
                (_, true, None) => (pre, 250, 2.6),
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
            let show = creating || (selected && (moving || f.op != Op::Add || !f.visible)) || (st.tool == Tool::Select && (st.hover_feature == Some(f.id) || st.hover_also.contains(&f.id)) && f.op != Op::Add);
            if !show || (creating && !f.visible) {
                continue;
            }
            let c = match f.op { Op::Add => accent, Op::Carve => carve, Op::Intersect => theme::intersect() };
            let alpha = if selected || creating { 230 } else { 140 };
            let lines: Vec<Vec<[f32; 3]>> = st.shape_outline(f).iter().map(|l| l.iter().map(|p| p.as_vec3().to_array()).collect()).collect();
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
                let face_m = &self.face_m;
                let place = |e: &nurbs_kernel::Edge, _els: &[Option<nurbs_kernel::FeatureId>; 2]| -> Vec<[f32; 3]> {
                    match e.faces.iter().find_map(|f| usize::try_from(*f).ok().and_then(|f| face_m.get(f).copied().flatten())) {
                        Some(x) => e.pts.iter().map(|p| x.transform_point3(glam::Vec3::from(*p)).to_array()).collect(),
                        None => e.pts.clone(),
                    }
                };
                let (mut sel, mut hov) = (vec![], vec![]);
                for e in m.edges.iter().filter(|e| !e.seam) {
                    let els = [of(e.faces[0]), of(e.faces[1])];
                    if els.iter().flatten().any(|x| st.selection.contains(x)) {
                        sel.push(place(e, &els));
                    } else if st.hover_feature.map(|h| els.contains(&Some(h))).unwrap_or(false) || els.iter().flatten().any(|x| st.hover_also.contains(x)) {
                        hov.push(e.pts.clone());
                    }
                }
                let a = theme::accent();
                line_vertices(&sel, [a.r(), a.g(), a.b(), 240], 1.7, &mut over);
                line_vertices(&hov, [a.r(), a.g(), a.b(), 120], 1.4, &mut over);
            }
        }
        // the groups: the selected one's box by its corners, an open one's
        // whole box (`groups.rs`)
        for (lines, alpha) in st.group_lines() {
            let lines: Vec<Vec<[f32; 3]>> = lines.iter().map(|l| l.iter().map(|p| p.as_vec3().to_array()).collect()).collect();
            line_vertices(&lines, [accent.r(), accent.g(), accent.b(), alpha], 1.3, &mut over);
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

/// A preview's matrices as one number (the same again: nothing to upload).
fn preview_key(pv: &std::collections::HashMap<nurbs_kernel::FeatureId, glam::DMat4>) -> u64 {
    let mut ids: Vec<&nurbs_kernel::FeatureId> = pv.keys().collect();
    ids.sort();
    let mut h = 0xcbf29ce484222325u64;
    let mut eat = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x100000001b3);
    };
    for id in ids {
        eat(*id as u64);
        for c in pv[id].to_cols_array() {
            eat(c.to_bits());
        }
    }
    h
}

/// The extrusions' depths in a preview as one number, with the shapes'.
fn push_key(k: u64, pp: &std::collections::HashMap<nurbs_kernel::FeatureId, (f64, f64)>) -> u64 {
    let mut ids: Vec<&nurbs_kernel::FeatureId> = pp.keys().collect();
    ids.sort();
    let mut h = k ^ 0x7075736800000000;
    for id in ids {
        for v in [*id as u64, pp[id].0.to_bits(), pp[id].1.to_bits()] {
            h ^= v;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

/// How the faces of extrusions move while their depth changes
/// (`AppState::push_preview`): each point along the outward normal of the
/// nearest point of its extrusion's end face (its own, on the end face), by
/// `delta` on the end face, nothing on the face it was pulled from (`depth`
/// behind it), in proportion between (the walls). A round face's end too
/// (its points each along their own normal); after a Mirror both sides
/// carry the step's tags, each wall finds its own end nearest.
pub struct PushMoves {
    /// Each model face's extrusion (an index in `ends`).
    face: Vec<Option<usize>>,
    ends: Vec<PushEnd>,
}

struct PushEnd {
    pts: Vec<(glam::Vec3, glam::Vec3)>,
    grid: std::collections::HashMap<(i32, i32, i32), Vec<u32>>,
    cell: f32,
    depth: f32,
    delta: f32,
}

impl PushEnd {
    fn key(&self, p: glam::Vec3) -> (i32, i32, i32) {
        let q = (p / self.cell).floor();
        (q.x as i32, q.y as i32, q.z as i32)
    }

    fn nearest(&self, p: glam::Vec3) -> Option<(glam::Vec3, glam::Vec3)> {
        let (x, y, z) = self.key(p);
        let mut best: Option<(f32, u32)> = None;
        for i in -1..=1 {
            for j in -1..=1 {
                for k in -1..=1 {
                    for &v in self.grid.get(&(x + i, y + j, z + k)).into_iter().flatten() {
                        let d = self.pts[v as usize].0.distance_squared(p);
                        if best.is_none_or(|b| d < b.0) {
                            best = Some((d, v));
                        }
                    }
                }
            }
        }
        if best.is_none() {
            best = self.pts.iter().enumerate().map(|(v, q)| (q.0.distance_squared(p), v as u32)).min_by(|a, b| a.0.total_cmp(&b.0));
        }
        best.map(|(_, v)| self.pts[v as usize])
    }

    fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let q = glam::Vec3::from(p);
        let Some((e, n)) = self.nearest(q) else { return p };
        let s = (1.0 + (q - e).dot(n) / self.depth).clamp(0.0, 1.0);
        (q + n * (s * self.delta)).to_array()
    }
}

impl PushMoves {
    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// The point `p` of model face `f` where the extrusions now put it
    /// (None: a face no extrusion moves).
    pub fn apply(&self, f: usize, p: [f32; 3]) -> Option<[f32; 3]> {
        let e = (*self.face.get(f)?)?;
        Some(self.ends[e].apply(p))
    }

    pub fn moves(&self, f: usize) -> bool {
        self.face.get(f).copied().flatten().is_some()
    }
}

/// The moves of the faces of the extrusions changing depth: their end
/// faces' points (tagged 1) with their normals, in cells as big as the
/// depth so a wall's point finds its end's nearest point at once.
pub fn push_moves(m: &nurbs_kernel::Model, pp: &std::collections::HashMap<nurbs_kernel::FeatureId, (f64, f64)>) -> PushMoves {
    use glam::Vec3;
    let mut index: std::collections::HashMap<nurbs_kernel::FeatureId, usize> = Default::default();
    let mut ends: Vec<PushEnd> = vec![];
    let mut seen: std::collections::HashSet<u32> = Default::default();
    for (t, tri) in m.triangles.iter().enumerate() {
        let Some(tag) = m.faces.get(m.tri_face[t] as usize).and_then(|f| f.tag) else { continue };
        if tag.sub != 1 {
            continue;
        }
        let Some(&(depth, delta)) = pp.get(&tag.feature) else { continue };
        let e = *index.entry(tag.feature).or_insert_with(|| {
            ends.push(PushEnd { pts: vec![], grid: Default::default(), cell: (depth.abs() as f32 * 1.5).max(1.0), depth: depth as f32, delta: delta as f32 });
            ends.len() - 1
        });
        for &v in tri {
            if seen.insert(v) {
                let (p, n) = (Vec3::from(m.positions[v as usize]), Vec3::from(m.normals[v as usize]).normalize_or_zero());
                ends[e].pts.push((p, n));
            }
        }
    }
    for e in ends.iter_mut() {
        for (i, (p, _)) in e.pts.iter().enumerate() {
            let k = e.key(*p);
            e.grid.entry(k).or_default().push(i as u32);
        }
    }
    let face = m.faces.iter().map(|f| f.tag.and_then(|t| index.get(&t.feature).copied())).collect();
    PushMoves { face, ends }
}

/// Each face's matrix while whole shapes are dragged (`drag_preview`): its
/// element's. After a Mirror the faces on both sides carry the same tags:
/// those on the other side of the plane from the shape itself move the
/// mirrored way (R X R, R the plane's reflection; several planes: each one
/// the face is across). A shape whose faces lie across a plane is left to
/// the kernel's model (its two halves are one there).
pub fn preview_faces(st: &AppState, m: &nurbs_kernel::Model, pv: &std::collections::HashMap<nurbs_kernel::FeatureId, glam::DMat4>) -> Vec<Option<glam::Mat4>> {
    use glam::{DMat4, DVec3};
    let els = element_map(st, m);
    let el: Vec<Option<nurbs_kernel::FeatureId>> = m.faces.iter().map(|f| f.tag.map(|t| *els.get(&t.feature).unwrap_or(&t.feature)).filter(|e| pv.contains_key(e))).collect();
    let planes = st.mirror_planes();
    if planes.is_empty() {
        return el.iter().map(|e| e.and_then(|e| pv.get(&e)).map(|x| x.as_mat4())).collect();
    }
    // each moved face's extent along the axes
    let mut lo = vec![[f32::MAX; 3]; m.faces.len()];
    let mut hi = vec![[f32::MIN; 3]; m.faces.len()];
    for (t, tri) in m.triangles.iter().enumerate() {
        let f = m.tri_face[t] as usize;
        if el.get(f).copied().flatten().is_none() {
            continue;
        }
        for &v in tri {
            let p = m.positions[v as usize];
            for a in 0..3 {
                lo[f][a] = lo[f][a].min(p[a]);
                hi[f][a] = hi[f][a].max(p[a]);
            }
        }
    }
    // where each moved shape itself was when the model was made (its box's
    // middle brought back): the side its own faces are on
    let home: std::collections::HashMap<nurbs_kernel::FeatureId, DVec3> = pv
        .iter()
        .filter_map(|(id, x)| {
            let (a, b) = st.doc.feature(*id)?.bounds();
            Some((*id, x.inverse().transform_point3((a + b) * 0.5)))
        })
        .collect();
    const EPS: f64 = 1e-3;
    let mut across: std::collections::HashSet<nurbs_kernel::FeatureId> = Default::default();
    let mut flips = vec![0u8; m.faces.len()];
    for f in 0..m.faces.len() {
        let (Some(e), true) = (el[f], lo[f][0] <= hi[f][0]) else { continue };
        let Some(h) = home.get(&e) else { continue };
        for (k, (a, off)) in planes.iter().enumerate().take(8) {
            let (l, u) = (lo[f][*a] as f64 - off, hi[f][*a] as f64 - off);
            if l < -EPS && u > EPS {
                across.insert(e);
            } else if (l + u > 0.0) != (h[*a] - off >= 0.0) {
                flips[f] |= 1 << k;
            }
        }
    }
    (0..m.faces.len())
        .map(|f| {
            let e = el[f].filter(|e| !across.contains(e))?;
            let x = pv.get(&e)?;
            let mut r = DMat4::IDENTITY;
            for (k, (a, off)) in planes.iter().enumerate().take(8) {
                if flips[f] & (1 << k) != 0 {
                    let (mut s, mut t) = (DVec3::ONE, DVec3::ZERO);
                    s[*a] = -1.0;
                    t[*a] = 2.0 * off;
                    r = DMat4::from_translation(t) * DMat4::from_scale(s) * r;
                }
            }
            // (reflections across planes square to the axes: r is its own inverse)
            Some((r * *x * r).as_mat4())
        })
        .collect()
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

/// A face as the shader reads it, from its material's look.
fn face_gpu(l: &materials::Look, flags: u32) -> FaceGpu {
    FaceGpu { color: linear(l.color), flags, metal: l.metal, rough: l.rough, spec: l.specular, alpha: l.opacity() }
}
