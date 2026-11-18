//! From the app state to the renderer: the scan (uploaded when opened), the
//! NURBS objects (tessellated, with their rings and seams), and what the
//! tool in hand draws over it.

use crate::render::{line_vertices, linear, FaceGpu, Frame, LineVertex, MeshVertex, Renderer};
use crate::state::{AppState, Shape};
use crate::theme;
use glam::DVec3;
use rayon::prelude::*;
use rectify_core::mesh::crease_normals;
use rectify_core::nurbs::{Curve, Surface};
use rectify_core::path::{Snap, SnapKind};
use rectify_core::Mesh;

/// The scan's colour: grey clay (sRGB).
pub const CLAY: [f32; 3] = [0.52, 0.52, 0.51];
/// Sharp where the surface turns more than this (degrees), else smooth.
pub const CREASE: f64 = 40.0;
/// The NURBS objects: kept, and being placed (sRGB).
pub const OBJECT: [f32; 3] = [0.78, 0.52, 0.33];
pub const OBJECT_LIVE: [f32; 3] = [0.96, 0.56, 0.24];

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
    let to32 = |l: Vec<DVec3>| l.iter().map(|p| p.as_vec3().to_array()).collect::<Vec<_>>();
    let isos: Vec<Vec<[f32; 3]>> = s.isos(24).into_iter().map(to32).collect();
    line_vertices(&isos, line, 1.0, wire);
    if let Some(col) = hull {
        let net: Vec<Vec<[f32; 3]>> = s.hull().into_iter().map(to32).collect();
        line_vertices(&net, col, 0.6, wire);
    }
}

/// A NURBS curve as a line: each of its knot spans sampled (a straight one
/// stays straight).
fn curve_lines(c: &Curve) -> Vec<[f32; 3]> {
    let mut ks: Vec<f64> = c.knots[c.degree..=c.pts.len()].to_vec();
    ks.dedup();
    let mut out = vec![c.eval(ks[0]).as_vec3().to_array()];
    for w in ks.windows(2) {
        for k in 1..=16 {
            out.push(c.eval(w[0] + (w[1] - w[0]) * k as f64 / 16.0).as_vec3().to_array());
        }
    }
    out
}

/// What caught the pointer, drawn on the scan: the face (a small square on
/// its plane), the edge (along the faces), the corner (its three edges).
fn snap_lines(s: &Snap, size: f64, out: &mut Vec<LineVertex>) {
    let col = [40, 140, 255, 240];
    let p = |x: DVec3| x.as_vec3().to_array();
    match s.kind {
        SnapKind::Scan => {}
        SnapKind::Plane => {
            let pl = &s.planes[0];
            let u = pl.n.any_orthonormal_vector() * size;
            let v = pl.n.cross(u.normalize()) * size;
            let c = s.p;
            line_vertices(&[vec![p(c - u - v), p(c + u - v), p(c + u + v), p(c - u + v), p(c - u - v)]], [40, 140, 255, 170], 1.0, out);
        }
        SnapKind::Edge => {
            let (a, b) = (s.p - s.dir * s.run.0, s.p + s.dir * s.run.1);
            line_vertices(&[vec![p(a), p(b)]], col, 1.2, out);
        }
        SnapKind::Corner => {
            let pl = &s.planes;
            for (i, j) in [(0, 1), (1, 2), (0, 2)] {
                let e = pl[i].n.cross(pl[j].n).normalize_or_zero();
                // along the side where the two faces are
                let k = 3 - i - j;
                let e = if pl[k].dist(s.p + e) > 0.0 { -e } else { e };
                line_vertices(&[vec![p(s.p), p(s.p + e * size * 3.0)]], col, 1.2, out);
            }
        }
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

    /// The NURBS objects: those kept, and the one being placed (with its
    /// control net).
    fn sync_objects(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &AppState) {
        if self.objects_gen == st.objects_gen {
            return;
        }
        self.objects_gen = st.objects_gen;
        let (mut verts, mut wire) = (vec![], vec![]);
        for o in &st.objects {
            match &o.shape {
                Shape::Surface(s) => surface_vertices(s, 1, [60, 32, 14, 200], None, &mut verts, &mut wire),
                Shape::Curve(c) => line_vertices(&[curve_lines(c)], [226, 92, 14, 255], 1.8, &mut wire),
            }
        }
        if let Some(ce) = &st.circle {
            surface_vertices(&ce.round.surface, 2, [255, 240, 225, 240], Some([255, 196, 140, 110]), &mut verts, &mut wire);
        }
        if let Some(se) = &st.slot {
            surface_vertices(&se.laid.surface, 2, [255, 240, 225, 240], Some([255, 196, 140, 110]), &mut verts, &mut wire);
        }
        // the path being drawn, and its control points
        if let Some(b) = st.path.as_ref().and_then(|pe| pe.built.as_ref()) {
            let hull: Vec<[f32; 3]> = b.curve.pts.iter().map(|x| x.as_vec3().to_array()).collect();
            line_vertices(&[hull], [255, 150, 70, 110], 0.6, &mut wire);
            line_vertices(&[curve_lines(&b.curve)], [255, 118, 24, 255], 2.0, &mut wire);
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
        // the Circle drag: the diameter drawn, its circle on the plane
        if let Some(cd) = &st.circle_drag {
            let n = cd.n;
            let b = cd.b - n * n.dot(cd.b - cd.a);
            line_vertices(&[vec![p(cd.a), p(b)]], [235, 235, 235, 220], 1.0, &mut out);
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
            line_vertices(&[ring], [255, 150, 60, 200], 1.2, &mut out);
        }
        // the Slot drag: along the slot, on the plane under the press
        if let Some(cd) = &st.slot_drag {
            let n = cd.n;
            let b = cd.b - n * n.dot(cd.b - cd.a);
            line_vertices(&[vec![p(cd.a), p(b)]], [255, 150, 60, 220], 1.2, &mut out);
        }
        // the Path: the next segment, and what catches the pointer
        if let Some(pe) = &st.path {
            if let (Some(pv), Some(b)) = (&pe.preview, &pe.built) {
                let n = b.breaks.len() - 1;
                let rest: Vec<Vec<[f32; 3]>> = pv.polylines().into_iter().skip(n).map(|l| l.iter().map(|x| p(*x)).collect()).collect();
                line_vertices(&rest, [255, 118, 24, 170], 1.2, &mut out);
            } else if let (Some(h), true) = (&st.path_hover, pe.pts.len() == 1) {
                let a = if pe.params.planes { pe.pts[0].p } else { pe.pts[0].hit };
                line_vertices(&[vec![p(a), p(h.p)]], [255, 118, 24, 170], 1.2, &mut out);
            }
        }
        if let Some(h) = &st.path_hover {
            let size = {
                let v = st.view();
                let (fwd, _, _) = v.basis();
                v.pixel_size_at((h.p.as_vec3() - v.eye).dot(fwd).max(1e-3)) as f64 * 10.0
            };
            snap_lines(h, size, &mut out);
        }
        // the surface placed: its axis
        if let Some(ce) = &st.circle {
            let ci = &ce.circle;
            line_vertices(&[vec![p(ci.c - ci.a * (ci.r * 0.3)), p(ci.c + ci.a * (ce.params.height + ci.r * 0.3))]], [255, 150, 60, 160], 0.8, &mut out);
        }
        // the slot placed: its centre line
        if let Some(se) = &st.slot {
            let sl = &se.slot;
            line_vertices(&[vec![p(sl.a), p(sl.b)]], [255, 150, 60, 160], 0.8, &mut out);
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
            let face = |c: [f32; 3]| FaceGpu { color: linear(c), flags: 0, metal: 0.0, rough: 0.55, pad: [0.0; 2] };
            self.r.upload_faces(device, queue, &[face(CLAY), face(OBJECT), face(OBJECT_LIVE)]);
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
            scan: !st.hide_scan,
            base: CLAY,
            sel: (sel, 0.6),
            hover: (sel, 0.25),
            paint,
            textures,
        };
        self.r.render(device, queue, target, size, frame);
    }
}
