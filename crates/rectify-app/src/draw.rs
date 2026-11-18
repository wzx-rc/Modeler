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
        if self.overlay_gen == Some((st.overlay_gen, st.bare)) {
            return;
        }
        self.overlay_gen = Some((st.overlay_gen, st.bare));
        let mut out: Vec<LineVertex> = vec![];
        // the bare view (captures before / after): the mesh alone
        if st.bare {
            self.r.upload_overlay(device, queue, &out);
            return;
        }
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
        // the Line placed: its edge
        if let Some(le) = &st.line_edit {
            line_vertices(&[vec![p(le.a), p(le.b)]], [235, 235, 235, 120], 0.8, &mut out);
            line_vertices(&[vec![p(le.fit.a), p(le.fit.b)]], [255, 150, 60, 255], 2.0, &mut out);
            for e in [le.fit.a, le.fit.b] {
                line_vertices(&[cross(e, st)], [255, 150, 60, 255], 1.4, &mut out);
            }
        }
        // the Circle drag: the diameter drawn; the part found: its rings
        if let Some(cd) = &st.circle_drag {
            // on the plane under the press
            let n = cd.n;
            let b = cd.b - n * n.dot(cd.b - cd.a);
            line_vertices(&[vec![p(cd.a), p(b)]], [235, 235, 235, 220], 1.0, &mut out);
            let c = (cd.a + b) * 0.5;
            let r = (b - cd.a).length() * 0.5;
            let u = (b - cd.a).normalize_or_zero();
            let v = n.cross(u).normalize_or_zero();
            let ring: Vec<[f32; 3]> = (0..=96).map(|i| {
                let t = i as f64 / 96.0 * std::f64::consts::TAU;
                p(c + (u * t.cos() + v * t.sin()) * r)
            }).collect();
            line_vertices(&[ring], [255, 150, 60, 200], 1.2, &mut out);
        }
        if let Some(ce) = &st.circle {
            let ci = &ce.circle;
            let u = ci.a.any_orthonormal_vector();
            let v = ci.a.cross(u);
            let mut rings = vec![];
            for &(hh, r) in &ce.rings {
                let o = ci.c + ci.a * hh;
                rings.push((0..=96).map(|i| {
                    let t = i as f64 / 96.0 * std::f64::consts::TAU;
                    p(o + (u * t.cos() + v * t.sin()) * r)
                }).collect::<Vec<_>>());
            }
            line_vertices(&rings, [255, 150, 60, 255], 1.5, &mut out);
            let mut sides = vec![];
            for k in 0..8 {
                let t = k as f64 / 8.0 * std::f64::consts::TAU;
                let dir = u * t.cos() + v * t.sin();
                sides.push(ce.rings.iter().map(|&(hh, r)| p(ci.c + ci.a * hh + dir * r)).collect::<Vec<_>>());
            }
            line_vertices(&sides, [255, 150, 60, 140], 0.9, &mut out);
            let top = ce.rings.last().map(|r| r.0).unwrap_or(0.0);
            line_vertices(&[vec![p(ci.c - ci.a * (ci.r * 0.3)), p(ci.c + ci.a * (top + ci.r * 0.3))]], [255, 150, 60, 160], 0.8, &mut out);
        }
        // the mirror plane: its outline across the mesh's box, and a grid
        if let (crate::state::Tool::Mirror, Some(f), Some(d)) = (st.tool, st.sym.found, &st.doc) {
            let (lo, hi) = d.original.bounds();
            let c = (lo + hi) * 0.5;
            let n = f.mirror.n;
            let c = c - n * (n.dot(c) - f.mirror.d);
            let a = if n.x.abs() > 0.9 { DVec3::Y } else { DVec3::X };
            let u = (a - n * a.dot(n)).normalize();
            let v = n.cross(u);
            let ext = (hi - lo) * 0.5 * 1.08;
            let (su, sv) = (ext.dot(u.abs()), ext.dot(v.abs()));
            let corner = |i: f64, j: f64| p(c + u * su * i + v * sv * j);
            let mut lines = vec![vec![corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0), corner(-1.0, -1.0)]];
            for k in 1..8 {
                let t = -1.0 + 2.0 * k as f64 / 8.0;
                lines.push(vec![corner(t, -1.0), corner(t, 1.0)]);
                lines.push(vec![corner(-1.0, t), corner(1.0, t)]);
            }
            line_vertices(&lines[..1], [255, 150, 60, 255], 1.6, &mut out);
            line_vertices(&lines[1..], [255, 150, 60, 110], 0.8, &mut out);
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
