//! From the app state to the renderer: the mesh of the current view (the
//! original, the regions in colour, or the result), the lines where regions
//! meet, and each region's colour and state.

use crate::render::{line_vertices, linear, FaceGpu, Frame, LineVertex, MeshVertex, Renderer, FACE_HOVERED, FACE_SELECTED};
use crate::state::{AppState, BrushMode, Style, Tool};
use glam::DVec3;
use rectify_core::Prim;
use crate::theme;
use rayon::prelude::*;
use rectify_core::snap::crease_normals;
use rectify_core::{Kind, Stage, NONE};

/// The mesh's colour: grey clay (sRGB).
pub const CLAY: [f32; 3] = [0.52, 0.52, 0.51];
/// Faces left free (no shape) in the Regions view.
pub const FREE: [f32; 3] = [0.20, 0.20, 0.21];

pub struct Drawer {
    pub r: Renderer,
    gen: u64,
    key: Option<(Stage, Style)>,
    flags: Vec<FaceGpu>,
    /// Display normals of the mesh shown, by (mesh generation, stage, style).
    normals: Option<((u64, Stage, bool), Vec<[DVec3; 3]>)>,
    stroke_gen: Option<u64>,
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
        Drawer { r: Renderer::new(device, format), gen: 0, key: None, flags: vec![], normals: None, stroke_gen: None }
    }

    fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &AppState) {
        // while a step runs the pipeline is away: what is uploaded stays
        let key = (st.shown(), st.style);
        if (self.gen == st.gen && self.key == Some(key)) || st.pipe.is_none() {
            return;
        }
        self.gen = st.gen;
        self.key = Some(key);
        let p = st.pipe.as_ref().unwrap();
        let (stage, style) = key;
        let m = st.shown_mesh().unwrap();
        let seg = if style == Style::Regions { st.shown_seg() } else { None };
        let label: Option<&[u32]> = seg.map(|s| s.label.as_slice());
        let diff = if style == Style::Diff { p.stage_diff(stage).filter(|d| d.moved.len() == m.pos.len()) } else { None };
        // the same in every stage, so that they compare: sharp where the
        // surface turns more than 40°, else smooth
        let nkey = (st.mesh_gen, stage, style == Style::Regions && stage == Stage::Scan);
        if self.normals.as_ref().map(|n| n.0 != nkey).unwrap_or(true) {
            self.normals = Some((nkey, crease_normals(m, &p.topo, 40.0)));
        }
        let normals = &self.normals.as_ref().unwrap().1;
        let hl: std::collections::HashSet<u32> = st.highlight.iter().copied().collect();
        let face_of = |t: usize| -> u32 {
            if let Some(d) = diff {
                let tri = m.tris[t];
                let worse = tri.iter().map(|&v| d.worse[v as usize]).fold(0.0f32, f32::max);
                if worse > 0.0 {
                    return DIFF_RED;
                }
                let mv = tri.iter().map(|&v| d.moved[v as usize]).fold(0.0f32, f32::max);
                return diff_bin(mv);
            }
            match label {
                None => 0,
                Some(l) => {
                    if !hl.is_empty() && (l[t] == NONE || !hl.contains(&l[t])) {
                        0
                    } else if l[t] == NONE {
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
        let lines_label = if style == Style::Surface && st.settings.edges && stage != Stage::Scan { st.shown_seg().map(|s| s.label.as_slice()) } else { label };
        if let (Some(l), true) = (lines_label, st.settings.edges && style != Style::Diff) {
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
            let col = if style == Style::Regions { [12, 13, 16, 200] } else { [20, 22, 26, 170] };
            line_vertices(&lines, col, 0.9, &mut edges);
        }
        self.r.upload_part(device, queue, &verts, &edges);
        self.stroke_gen = None;
    }

    /// The stroke being painted: the outline of what it covers, and the
    /// shape fitted on it (Shape tool).
    fn sync_overlay(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &AppState) {
        if self.stroke_gen == Some(st.stroke_gen) {
            return;
        }
        self.stroke_gen = Some(st.stroke_gen);
        let mut out: Vec<LineVertex> = vec![];
        if let (Some(stroke), Some(p), Some(m)) = (&st.stroke, &st.pipe, st.shown_mesh()) {
            let mut lines: Vec<Vec<[f32; 3]>> = vec![];
            for &f in &stroke.faces {
                let tri = m.tris[f as usize];
                for k in 0..3 {
                    let o = p.topo.adj[f as usize][k];
                    if o == rectify_core::NONE || !stroke.faces.contains(&o) {
                        lines.push(vec![m.pos[tri[k] as usize].as_vec3().to_array(), m.pos[tri[(k + 1) % 3] as usize].as_vec3().to_array()]);
                    }
                }
            }
            line_vertices(&lines, [255, 176, 112, 255], 1.1, &mut out);
            if let Some(q) = &stroke.prim {
                line_vertices(&shape_wire(q, &stroke.points), [242, 140, 58, 255], 1.6, &mut out);
            }
        }
        self.r.upload_overlay(device, queue, &out);
    }

    /// Each region's colour and state (cheap: compared every frame).
    fn sync_flags(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, st: &AppState) {
        if st.pipe.is_none() {
            return;
        }
        let style = st.style;
        let seg = st.shown_seg();
        if style == Style::Diff {
            let flags = diff_colors();
            if flags != self.flags {
                self.r.upload_faces(device, queue, &flags);
                self.flags = flags;
            }
            return;
        }
        // colours: 0 clay, 1 free, 2.. the regions
        let mut flags = vec![FaceGpu { color: linear(CLAY), flags: 0, metal: 0.0, rough: 0.55, pad: [0.0; 2] }];
        flags.push(FaceGpu { color: linear(if style == Style::Regions { FREE } else { CLAY }), flags: 0, metal: 0.0, rough: 0.55, pad: [0.0; 2] });
        // the regions a merge stroke has touched so far
        let touched: &[u32] = match (&st.stroke, st.tool) {
            (Some(s), Tool::Brush(BrushMode::Merge)) => &s.regions,
            _ => &[],
        };
        if let Some(s) = seg {
            for (i, r) in s.regions.iter().enumerate() {
                let color = if style == Style::Regions { region_color(i as u32, r.prim.kind()) } else { CLAY };
                let sel = st.selected == Some(i as u32);
                let f = if sel { FACE_SELECTED } else if touched.contains(&(i as u32)) { FACE_HOVERED } else { 0 };
                flags.push(FaceGpu { color: linear(color), flags: f, metal: 0.0, rough: 0.55, pad: [0.0; 2] });
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
        self.sync_overlay(device, queue, st);
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

/// Diff map colours: 0 untouched (clay), 1..=DIFF_STEPS moved (to orange,
/// 0.4 mm and more), then red: rougher than the AI mesh.
pub const DIFF_STEPS: u32 = 10;
pub const DIFF_RED: u32 = DIFF_STEPS + 1;
/// The move (mm) at which the diff map is fully orange.
pub const DIFF_FULL: f32 = 0.4;

fn diff_bin(mv: f32) -> u32 {
    if mv < 0.01 {
        0
    } else {
        1 + ((mv / DIFF_FULL).min(1.0) * (DIFF_STEPS - 1) as f32).round() as u32
    }
}

/// The colour of a diff bin (sRGB), for the legend too.
pub fn diff_color(bin: u32) -> [f32; 3] {
    if bin == 0 {
        return [0.46, 0.47, 0.49];
    }
    if bin >= DIFF_RED {
        return [0.86, 0.16, 0.14];
    }
    let t = (bin - 1) as f32 / (DIFF_STEPS - 1) as f32;
    let a = [0.62, 0.62, 0.58];
    let b = [0.98, 0.58, 0.18];
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn diff_colors() -> Vec<FaceGpu> {
    (0..=DIFF_RED).map(|b| FaceGpu { color: linear(diff_color(b)), flags: 0, metal: 0.0, rough: 0.6, pad: [0.0; 2] }).collect()
}

/// Lines that show a shape around the stroke that fitted it: a disc for a
/// plane, rings and lines for a cylinder, three great circles for a sphere.
pub fn shape_wire(q: &Prim, pts: &[DVec3]) -> Vec<Vec<[f32; 3]>> {
    let n = pts.len().max(1) as f64;
    let mean = pts.iter().copied().sum::<DVec3>() / n;
    let circle = |c: DVec3, u: DVec3, v: DVec3, r: f64| -> Vec<[f32; 3]> {
        (0..=64)
            .map(|i| {
                let t = i as f64 / 64.0 * std::f64::consts::TAU;
                (c + (u * t.cos() + v * t.sin()) * r).as_vec3().to_array()
            })
            .collect()
    };
    match *q {
        Prim::Plane { n, .. } => {
            let c = q.project(mean);
            let r = pts.iter().map(|p| (q.project(*p) - c).length()).fold(0.0, f64::max).max(2.0) * 1.3;
            let u = n.any_orthonormal_vector();
            let v = n.cross(u);
            vec![
                circle(c, u, v, r),
                vec![(c - u * r).as_vec3().to_array(), (c + u * r).as_vec3().to_array()],
                vec![(c - v * r).as_vec3().to_array(), (c + v * r).as_vec3().to_array()],
            ]
        }
        Prim::Cylinder { p, a, r } => {
            let ts: Vec<f64> = pts.iter().map(|x| a.dot(*x - p)).collect();
            let (mut t0, mut t1) = ts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| (lo.min(*t), hi.max(*t)));
            if !t0.is_finite() {
                t0 = 0.0;
                t1 = 0.0;
            }
            let extra = (r * 1.5 - (t1 - t0) * 0.5).max(r * 0.5);
            t0 -= extra;
            t1 += extra;
            let u = a.any_orthonormal_vector();
            let v = a.cross(u);
            let mut out = vec![circle(p + a * t0, u, v, r), circle(p + a * ((t0 + t1) * 0.5), u, v, r), circle(p + a * t1, u, v, r)];
            for k in 0..8 {
                let ang = k as f64 / 8.0 * std::f64::consts::TAU;
                let d = (u * ang.cos() + v * ang.sin()) * r;
                out.push(vec![(p + a * t0 + d).as_vec3().to_array(), (p + a * t1 + d).as_vec3().to_array()]);
            }
            out
        }
        Prim::Sphere { c, r } => vec![circle(c, DVec3::X, DVec3::Y, r), circle(c, DVec3::Y, DVec3::Z, r), circle(c, DVec3::Z, DVec3::X, r)],
    }
}
