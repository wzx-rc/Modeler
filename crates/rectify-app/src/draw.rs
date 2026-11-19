//! From the app state to the renderer: the scan (uploaded when opened), the
//! shapes (bodies as OpenCascade meshed them, with their edges; the Slot's
//! and the Box's surfaces with their rings), and what the tool in hand
//! draws over it.

use crate::render::{line_vertices, linear, FaceGpu, Frame, LineVertex, MeshVertex, Renderer, FACE_HOVERED, FACE_SELECTED};
use crate::state::{AppState, Comp, SelMode, Shape, Tool};
use glam::{DVec2, DVec3};
use rayon::prelude::*;
use rectify_core::mesh::crease_normals;
use rectify_core::nurbs::Surface;
use rectify_core::slot::Kind;
use rectify_core::Mesh;
use std::collections::HashSet;

/// The scan's colour: grey clay (sRGB).
pub const CLAY: [f32; 3] = [0.52, 0.52, 0.51];
/// Sharp where the surface turns more than this (degrees), else smooth.
pub const CREASE: f64 = 40.0;
/// The shapes: kept, and being placed (sRGB).
pub const OBJECT: [f32; 3] = [0.78, 0.52, 0.33];
pub const OBJECT_LIVE: [f32; 3] = [0.96, 0.56, 0.24];
/// The renderer's materials: 0 the scan, 1 a surface, 2 one being placed,
/// 3 a body, 4 its faces picked, 5 under the pointer, 6 a body just made.
const BODY: u32 = 3;
const BODY_SEL: u32 = 4;
const BODY_HOVER: u32 = 5;
const BODY_FRESH: u32 = 6;
/// Read by the shader: not lifted along its normal (a body is moved off
/// the scan as a whole, so its faces stay joined at their edges).
const NO_LIFT: u32 = 0x8000;

pub struct Drawer {
    pub r: Renderer,
    gen: u64,
    overlay_gen: Option<(u64, bool)>,
    objects_gen: u64,
    faces_done: bool,
}

/// A NURBS surface as triangles (`face`: the renderer's material) and its
/// lines: the knot lines (its rings and seams) and, `hull`, the control net.
fn surface_vertices(s: &Surface, face: u32, line: [u8; 4], hull: Option<[u8; 4]>, verts: &mut Vec<MeshVertex>, wire: &mut Vec<LineVertex>) {
    let to32 = |l: Vec<DVec3>| l.iter().map(|p| p.as_vec3().to_array()).collect::<Vec<_>>();
    let su = 96;
    let sv = ((s.nv.saturating_sub(s.dv)) * 6).clamp(8, 256);
    let (pts, nrm) = s.tessellate(su, sv);
    let at = |i: usize, j: usize| j * (su + 1) + i;
    let vx = |k: usize| MeshVertex { pos: pts[k].as_vec3().to_array(), nrm: nrm[k].as_vec3().to_array(), face };
    for j in 0..sv {
        for i in 0..su {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            verts.extend_from_slice(&[vx(a), vx(b), vx(c), vx(a), vx(c), vx(d)]);
        }
    }
    let isos: Vec<Vec<[f32; 3]>> = s.isos(24).into_iter().map(to32).collect();
    line_vertices(&isos, line, 1.0, wire);
    if let Some(col) = hull {
        let net: Vec<Vec<[f32; 3]>> = s.hull().into_iter().map(to32).collect();
        line_vertices(&net, col, 0.6, wire);
    }
}

/// How far a body is drawn off the scan, along its plane's normal (mm):
/// next to nothing, so it shows where it is (the shader pulls it towards
/// the eye along the view instead, which does not move it on screen).
fn lift_of(st: &AppState, b: &crate::body::Body) -> f64 {
    let e = st.doc.as_ref().map(|d| d.edge).unwrap_or(0.1);
    if b.built.solid() {
        (e * 0.05).clamp(0.002, 0.01)
    } else {
        (e * 0.1).clamp(0.005, 0.02)
    }
}

/// A body's triangles (its faces picked or under the pointer tinted) and
/// its edges (open ones, which can be pulled, brighter).
fn body_vertices(st: &AppState, obj: usize, verts: &mut Vec<MeshVertex>, wire: &mut Vec<LineVertex>) {
    let Some(b) = st.body(obj) else { return };
    let t = &b.built.topo;
    let lift = (b.frame.n * lift_of(st, b)).as_vec3();
    let mut sel: HashSet<usize> = HashSet::new();
    for c in &st.sel {
        sel.extend(st.comp_faces(c).into_iter().filter(|f| f.0 == obj).map(|f| f.1));
    }
    let hover: HashSet<usize> = st.hover.as_ref().map(|c| st.comp_faces(c).into_iter().filter(|f| f.0 == obj).map(|f| f.1).collect()).unwrap_or_default();
    let fresh = st.fresh == Some(obj);
    let at = |j: u32| (glam::Vec3::from_array(t.positions[j as usize]) + lift).to_array();
    for (k, tri) in t.triangles.iter().enumerate() {
        let f = t.tri_face.get(k).copied().unwrap_or(0) as usize;
        let m = if sel.contains(&f) {
            BODY_SEL
        } else if hover.contains(&f) {
            BODY_HOVER
        } else if fresh {
            BODY_FRESH
        } else {
            BODY
        };
        for &j in tri {
            verts.push(MeshVertex { pos: at(j), nrm: t.normals.get(j as usize).copied().unwrap_or([0.0, 1.0, 0.0]), face: m | NO_LIFT });
        }
    }
    // its edges: picked, under the pointer, open, the others
    let mut picked: HashSet<usize> = HashSet::new();
    for c in &st.sel {
        picked.extend(st.comp_edges(c).into_iter().filter(|e| e.0 == obj).map(|e| e.1));
    }
    let hovered: HashSet<usize> = st.hover.as_ref().map(|c| st.comp_edges(c).into_iter().filter(|e| e.0 == obj).map(|e| e.1).collect()).unwrap_or_default();
    let edge_mode = st.tool == Tool::Select && st.mode == SelMode::Edge;
    for (e, ed) in t.edges.iter().enumerate() {
        if ed.seam {
            continue;
        }
        let line: Vec<[f32; 3]> = ed.pts.iter().map(|p| (DVec3::from_array(*p).as_vec3() + lift).to_array()).collect();
        let (col, w) = if picked.contains(&e) {
            ([255, 255, 255, 255], 2.6)
        } else if hovered.contains(&e) {
            ([255, 214, 170, 240], 2.2)
        } else if b.built.edge_open(e) {
            ([255, 150, 60, 255], if edge_mode { 1.6 } else { 1.3 })
        } else {
            ([48, 28, 14, 230], if edge_mode { 1.3 } else { 1.0 })
        };
        line_vertices(&[line], col, w, wire);
    }
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

/// A square grid on a plane round `c` (`size`: half its side).
fn plane_grid(f: &crate::body::Frame, c: DVec3, size: f64, col: [u8; 4], out: &mut Vec<LineVertex>) {
    let p = |x: DVec3| x.as_vec3().to_array();
    let n = 4;
    let mut lines = vec![];
    for i in 0..=n {
        let s = -size + 2.0 * size * i as f64 / n as f64;
        lines.push(vec![p(c + f.x * s - f.y * size), p(c + f.x * s + f.y * size)]);
        lines.push(vec![p(c - f.x * size + f.y * s), p(c + f.x * size + f.y * s)]);
    }
    line_vertices(&lines, col, 0.8, out);
    // its normal, short
    line_vertices(&[vec![p(c), p(c + f.n * size * 0.5)]], col, 1.0, out);
}

impl Drawer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Drawer {
        Drawer { r: Renderer::new(device, format), gen: 0, overlay_gen: None, objects_gen: 0, faces_done: false }
    }

    fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &mut AppState) {
        let Some(d) = &st.doc else { return };
        if self.gen != st.gen {
            self.gen = st.gen;
            let verts = mesh_vertices(&d.mesh, &crease_normals(&d.mesh, &d.topo, CREASE));
            self.r.upload_part(device, queue, &verts, &[]);
        }
    }

    /// The shapes: those kept, and the one being placed (with its control
    /// net).
    fn sync_objects(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &AppState) {
        if self.objects_gen == st.objects_gen {
            return;
        }
        self.objects_gen = st.objects_gen;
        let (mut verts, mut wire) = (vec![], vec![]);
        for (i, o) in st.objects.iter().enumerate() {
            match &o.shape {
                Shape::Surface(s) => {
                    let picked = st.sel.contains(&Comp::Object(i));
                    let hovered = st.hover == Some(Comp::Object(i));
                    let m = if picked { BODY_SEL } else if hovered { BODY_HOVER } else { 1 };
                    surface_vertices(s, m, [60, 32, 14, 200], None, &mut verts, &mut wire);
                }
                Shape::Body(_) => body_vertices(st, i, &mut verts, &mut wire),
            }
        }
        if let Some(se) = &st.slot {
            surface_vertices(&se.laid.surface, 2, [255, 240, 225, 240], Some([255, 196, 140, 110]), &mut verts, &mut wire);
        }
        self.r.upload_parts(device, queue, &verts, &wire);
    }

    /// What the tool draws over the scan.
    fn sync_overlay(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &AppState) {
        if self.overlay_gen == Some((st.overlay_gen, st.bare)) {
            return;
        }
        self.overlay_gen = Some((st.overlay_gen, st.bare));
        let mut out: Vec<LineVertex> = vec![];
        if st.bare {
            self.r.upload_overlay(device, queue, &out);
            return;
        }
        let p = |x: DVec3| x.as_vec3().to_array();
        // the Pen: the plane under the pointer before the first point
        if let (Some(h), Some(hv)) = (&st.pen_plane, &st.pen_hover) {
            let size = st.px_at(hv.p) * 46.0;
            let col = if h.flat { [70, 160, 255, 210] } else { [200, 200, 200, 150] };
            plane_grid(&h.frame, hv.p, size, col, &mut out);
        }
        // the outline being drawn, the next side, and its plane round the
        // pointer
        if let Some(pe) = &st.pen {
            let f = &pe.frame;
            let pts: Vec<[f32; 3]> = pe.pts.iter().map(|q| p(f.to3(*q))).collect();
            // the corners rounded so far (the last one too, towards the
            // pointer)
            let mut q2 = pe.pts.clone();
            if let Some(h) = &st.pen_hover {
                q2.push(h.q);
            }
            let round = crate::body::rounded_outline(&q2, &pe.radii, false);
            let mut placed: Vec<[f32; 3]> = round.iter().map(|q| p(f.to3(*q))).collect();
            if st.pen_hover.is_some() {
                placed.pop();
            }
            if placed.len() >= 2 {
                line_vertices(&[placed.clone()], [255, 118, 24, 255], 2.0, &mut out);
            }
            // a fillet being dragged: its circle round the point
            if let Some(i) = pe.drag {
                let r = pe.radii.get(i).copied().unwrap_or(0.0);
                if r > 0.0 {
                    let c = pe.pts[i];
                    let ring: Vec<[f32; 3]> = (0..=64)
                        .map(|k| {
                            let t = k as f64 / 64.0 * std::f64::consts::TAU;
                            p(f.to3(c + DVec2::new(t.cos(), t.sin()) * r))
                        })
                        .collect();
                    line_vertices(&[ring], [255, 214, 170, 230], 1.2, &mut out);
                }
            }
            if let (Some(h), Some(last)) = (&st.pen_hover, placed.last().or(pts.last())) {
                line_vertices(&[vec![*last, p(h.p)]], [255, 160, 90, 220], 1.4, &mut out);
                if pe.pts.len() >= 2 {
                    line_vertices(&[vec![p(h.p), pts[0]]], [255, 160, 90, 70], 0.8, &mut out);
                }
                let size = st.px_at(h.p) * 30.0;
                plane_grid(f, h.p, size, if pe.flat { [70, 160, 255, 90] } else { [200, 200, 200, 70] }, &mut out);
            }
        }
        // the Knife: the cut's points, and the line to the pointer
        if let Some((obj, k, last)) = st.knife {
            if let Some(crate::body::Step::Cut { pts, .. }) = st.body(obj).and_then(|b| b.steps.get(k)) {
                if let Some(lp) = pts.get(last) {
                    if let (Some(h), Tool::Knife) = (st.knife_hover, st.tool) {
                        line_vertices(&[vec![p(lp.at), p(h)]], [255, 255, 255, 230], 1.4, &mut out);
                    }
                }
            }
        }
        // the Circle being drawn: its diameter and its ring on the plane
        if let Some(cd) = &st.circle {
            let n = cd.ring_normal();
            let b = cd.b;
            line_vertices(&[vec![p(cd.a), p(b)]], [235, 235, 235, 200], 1.0, &mut out);
            let c = (cd.a + b) * 0.5;
            let r = (b - cd.a).length() * 0.5;
            let u = (b - cd.a).normalize_or_zero();
            let v = n.cross(u).normalize_or_zero();
            let ring: Vec<[f32; 3]> = (0..=96)
                .map(|i| {
                    let t = i as f64 / 96.0 * std::f64::consts::TAU;
                    p(c + (u * t.cos() + v * t.sin()) * r)
                })
                .collect();
            line_vertices(&[ring], [255, 150, 60, 230], 1.6, &mut out);
        }
        // the Slot and Box drag: along the slot, on the plane under the press
        if let Some(cd) = &st.slot_drag {
            let n = cd.n;
            let b = cd.b - n * n.dot(cd.b - cd.a);
            line_vertices(&[vec![p(cd.a), p(b)]], [255, 150, 60, 220], 1.2, &mut out);
        }
        // the slot placed: its centre line
        if let Some(se) = &st.slot {
            let sl = &se.slot;
            if sl.kind != Kind::Round {
                line_vertices(&[vec![p(sl.a), p(sl.b)]], [255, 150, 60, 160], 0.8, &mut out);
            }
        }
        // a thickness being pulled: its arrow
        if let Some(crate::state::Gesture::Thick { o, n, .. }) = &st.gesture {
            let size = st.px_at(*o) * 40.0;
            line_vertices(&[vec![p(*o - *n * size), p(*o + *n * size)]], [120, 190, 255, 200], 1.2, &mut out);
        }
        self.r.upload_overlay(device, queue, &out);
    }

    /// Draws the view and the UI into `target` (`size` pixels, `ppp` pixels per point).
    #[allow(clippy::too_many_arguments)]
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, size: [u32; 2], ppp: f32, st: &mut AppState, paint: &[egui::ClippedPrimitive], textures: &mut egui::TexturesDelta) {
        self.sync(device, queue, st);
        self.sync_overlay(device, queue, st);
        self.sync_objects(device, queue, st);
        if !self.faces_done {
            let face = |c: [f32; 3], flags: u32| FaceGpu { color: linear(c), flags, metal: 0.0, rough: 0.55, pad: [0.0; 2] };
            self.r.upload_faces(device, queue, &[face(CLAY, 0), face(OBJECT, 0), face(OBJECT_LIVE, 0), face(OBJECT, 0), face(OBJECT, FACE_SELECTED), face(OBJECT, FACE_HOVERED), face(OBJECT_LIVE, 0)]);
            self.faces_done = true;
        }
        let frame = Frame {
            view: st.view().scaled(ppp),
            points_to_px: ppp,
            light: st.settings.light,
            grid: st.settings.grid,
            origin: false,
            zebra: st.settings.zebra,
            scan: !st.hide_scan,
            base: CLAY,
            sel: ([1.0, 0.6, 0.22], 0.55),
            hover: ([1.0, 0.82, 0.62], 0.3),
            paint,
            textures,
        };
        self.r.render(device, queue, target, size, frame);
    }
}
