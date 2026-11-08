//! Headless screenshots of the real app: drives it with synthetic input events
//! (Pencil, fingers, taps, drags) and renders frames with the same code as iPad.

use egui::{pos2, Event, Modifiers, PointerButton, Pos2, RawInput, Rect};
use forme_app::gfx::FrameRenderer;
use forme_app::state::{AppState, PointerKind, Tool};
use forme_app::{theme, AppState as _S};
use forme_core::{demo, CombineMode, NodeId, PrimKind};
use forme_render::headless::Gpu;
use glam::Vec3;
use std::time::Instant;

struct Harness {
    gpu: Gpu,
    fr: FrameRenderer,
    ctx: egui::Context,
    st: AppState,
    t: f64,
    size: [f32; 2],
    ppp: f32,
    pending: Vec<Event>,
    out_dir: String,
}

const FMT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

impl Harness {
    fn new(out_dir: &str) -> Self {
        let gpu = Gpu::new().expect("gpu");
        eprintln!("GPU : {}", gpu.adapter_name());
        let mut fr = FrameRenderer::new(&gpu.device, FMT);
        fr.full_scale = std::env::var("SHOT_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(0.6);
        let ctx = egui::Context::default();
        theme::install(&ctx);
        Harness {
            gpu,
            fr,
            ctx,
            st: AppState::default(),
            t: 0.0,
            size: [1194.0, 834.0],
            ppp: 2.0,
            pending: vec![],
            out_dir: out_dir.to_string(),
        }
    }

    fn reset(&mut self, st: AppState) {
        self.st = st;
        self.st.autosave = false;
        self.st.screen = self.size;
    }

    fn raw(&mut self) -> RawInput {
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(self.size[0], self.size[1]))),
            time: Some(self.t),
            predicted_dt: 1.0 / 60.0,
            events: std::mem::take(&mut self.pending),
            focused: true,
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(self.ppp);
        raw
    }

    /// Runs one UI frame (logic only, no GPU drawing).
    fn step(&mut self) {
        self.t += 1.0 / 60.0;
        let raw = self.raw();
        self.st.tick(self.t);
        let st = &mut self.st;
        let mut out = self.ctx.run_pass(raw, |ctx| forme_app::run(ctx, st));
        self.fr.apply_textures(&self.gpu.device, &self.gpu.queue, &mut out.textures_delta);
    }

    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    fn press(&mut self, p: Pos2, down: bool) {
        self.pending.push(Event::PointerMoved(p));
        self.pending.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: down, modifiers: Modifiers::NONE });
        self.step();
    }

    fn move_to(&mut self, p: Pos2) {
        self.pending.push(Event::PointerMoved(p));
        self.step();
    }

    fn tap(&mut self, p: Pos2) {
        self.press(p, true);
        self.press(p, false);
        self.steps(2);
    }

    fn drag(&mut self, a: Pos2, b: Pos2, n: usize, release: bool) {
        self.press(a, true);
        for i in 1..=n {
            let t = i as f32 / n as f32;
            self.move_to(a + (b - a) * t);
        }
        if release {
            self.press(b, false);
            self.step();
        }
    }

    fn screen_of(&self, w: Vec3) -> Pos2 {
        let p = self.st.view().project(w).expect("in front");
        pos2(p[0], p[1])
    }

    /// Renders the current state at full quality and saves a PNG.
    fn shot(&mut self, name: &str) {
        self.t += 1.0 / 60.0;
        let raw = self.raw();
        self.st.tick(self.t);
        let st = &mut self.st;
        let mut out = self.ctx.run_pass(raw, |ctx| forme_app::run(ctx, st));
        let paint = self.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        let w = (self.size[0] * self.ppp) as u32;
        let h = (self.size[1] * self.ppp) as u32;
        let tex = self.gpu.target(w, h, FMT);
        let view = tex.create_view(&Default::default());
        let t0 = Instant::now();
        self.fr.render(&self.gpu.device, &self.gpu.queue, &view, [w, h], self.ppp, &mut self.st, &paint, &mut out.textures_delta, true);
        let px = self.gpu.read_rgba(&tex);
        let path = format!("{}/{}.png", self.out_dir, name);
        image::save_buffer(&path, &px, w, h, image::ExtendedColorType::Rgba8).expect("png");
        eprintln!("{path}  ({:.1?})", t0.elapsed());
    }

    fn find(&self, name: &str) -> NodeId {
        self.st.scene.nodes.values().find(|n| n.name == name).map(|n| n.id).unwrap_or_else(|| panic!("node {name}"))
    }
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "shots".into());
    let only: Vec<String> = std::env::args().skip(2).collect();
    std::fs::create_dir_all(format!("{out}/docs")).ok();
    std::env::set_var("FORME_DOCS", format!("{out}/docs"));
    let mut h = Harness::new(&out);
    let want = |n: &str| only.is_empty() || only.iter().any(|o| n.starts_with(o.as_str()));

    if want("02") {
        // Main screen: the grenade, a boss picked by tapping it.
        h.reset(AppState::default());
        h.st.pointer_kind = PointerKind::Pencil;
        h.steps(3);
        let boss = h.screen_of(Vec3::new(4.6, 70.0, 33.2));
        h.tap(boss);
        let sel = h.st.selection.map(|id| h.st.scene.node(id).unwrap().name.clone());
        eprintln!("tap → sélection : {sel:?}");
        h.steps(2);
        h.shot("02-ecran-principal");
    }

    if want("03") {
        // Gesture creation in Carve: thumb held, Pencil sets the base then the depth.
        h.reset(AppState::default());
        h.st.pointer_kind = PointerKind::Pencil;
        h.st.camera.target = Vec3::new(2.0, 60.0, 10.0);
        h.st.camera.dist = 175.0;
        h.st.camera.yaw = 0.22;
        h.st.camera.pitch = 0.10;
        h.st.tool = Tool::Place(PrimKind::Cylinder);
        h.st.carve_held = true;
        h.steps(2);
        let a = h.screen_of(Vec3::new(0.0, 58.0, 26.0));
        h.drag(a, a + egui::vec2(34.0, 0.0), 8, true);
        let b = a + egui::vec2(34.0, 0.0);
        h.drag(b, b + egui::vec2(0.0, -120.0), 10, false);
        eprintln!("hint : {}", h.st.hint());
        h.shot("03-creation-carve");
        h.press(b + egui::vec2(0.0, -120.0), false);
        h.steps(2);
    }

    if want("04") || want("05") {
        // Profile mode: redraw the lever with the Pencil in the front orthographic view.
        h.reset(AppState::default());
        h.st.pointer_kind = PointerKind::Pencil;
        let lame = h.find("Lame");
        h.st.scene.node_mut(lame).unwrap().visible = false;
        h.st.changed();
        h.st.camera.preset(forme_app::camera::ViewPreset::Front);
        h.st.camera.target = Vec3::new(18.0, 84.0, 0.0);
        h.st.camera.dist = 300.0;
        h.st.tool = Tool::Profile;
        h.steps(2);
        h.st.ensure_profile();
        let pts = demo::lever_profile().points;
        let n_first = 14;
        for pt in pts.iter().take(n_first) {
            let p = h.screen_of(Vec3::new(pt.pos.x, pt.pos.y, 0.0));
            if pt.smooth {
                h.drag(p + egui::vec2(-16.0, 10.0), p, 6, true);
            } else {
                h.tap(p);
            }
        }
        let next = pts[n_first].pos;
        let hover = h.screen_of(Vec3::new(next.x, next.y, 0.0));
        h.move_to(hover);
        eprintln!("profil : {} points", h.st.profile.as_ref().map(|d| d.points.len()).unwrap_or(0));
        if want("04") {
            h.shot("04-mode-profil");
        }
        for pt in pts.iter().skip(n_first) {
            let p = h.screen_of(Vec3::new(pt.pos.x, pt.pos.y, 0.0));
            if pt.smooth {
                h.drag(p + egui::vec2(-16.0, 10.0), p, 6, true);
            } else {
                h.tap(p);
            }
        }
        let first = h.screen_of(Vec3::new(pts[0].pos.x, pts[0].pos.y, 0.0));
        h.tap(first);
        let sel = h.st.selection.map(|id| h.st.scene.node(id).unwrap().name.clone());
        eprintln!("contour fermé → {sel:?}");
        h.st.camera.preset(forme_app::camera::ViewPreset::Perspective);
        h.st.frame_all();
        h.steps(2);
        if want("05") {
            h.shot("05-profil-extrude");
        }
    }

    if want("06") {
        // A new part from scratch, only with gestures.
        h.reset(AppState::new(forme_core::Scene::new(), "Boîtier"));
        h.st.pointer_kind = PointerKind::Pencil;
        h.st.camera.target = Vec3::new(0.0, 16.0, 0.0);
        h.st.camera.dist = 300.0;
        h.st.camera.yaw = 0.62;
        h.st.camera.pitch = 0.48;
        h.steps(2);
        // 1. block on the ground: base, then height
        h.st.tool = Tool::Place(PrimKind::Box);
        let a = h.screen_of(Vec3::ZERO);
        let b = h.screen_of(Vec3::new(40.0, 0.0, 26.0));
        h.drag(a, b, 10, true);
        h.drag(b, b + egui::vec2(0.0, -40.0), 8, true);
        let block = h.st.selection.unwrap();
        let bn = h.st.scene.node(block).unwrap();
        let top = bn.transform.pos.y + bn.shape().unwrap().size.y;
        h.st.edit("Rondeur", block, |n| n.shape_mut().unwrap().round = 4.0);
        eprintln!("bloc : {:?} (dessus à {top:.1} mm)", h.st.scene.node(block).unwrap().shape().unwrap().size);
        // 2. hole in Carve (thumb held)
        h.st.tool = Tool::Place(PrimKind::Cylinder);
        h.st.carve_held = true;
        let c = h.screen_of(Vec3::new(-8.0, top, 0.0));
        let c2 = h.screen_of(Vec3::new(4.0, top, 0.0));
        h.drag(c, c2, 8, true);
        h.drag(c2, c2 + egui::vec2(0.0, -90.0), 8, true);
        h.st.carve_held = false;
        // 3. a circular groove around the hole
        h.st.mode = CombineMode::Groove;
        let g = h.screen_of(Vec3::new(-4.0, top, 0.0));
        let g2 = h.screen_of(Vec3::new(14.0, top, 0.0));
        h.drag(g, g2, 8, true);
        eprintln!("rainure : {:?}", h.st.placement.as_ref().map(|p| (p.snapped, p.normal)));
        h.drag(g2, g2 + egui::vec2(0.0, -60.0), 6, true);
        h.st.mode = CombineMode::Add;
        // 4. a screw from the kit, mirrored on X and Z → four corners
        h.st.tool = Tool::Kit(forme_app::kit::KitPiece::Screw);
        let s1 = h.screen_of(Vec3::new(31.0, top + 0.5, 17.0));
        h.tap(s1);
        let screw = h.st.selection.unwrap();
        h.st.edit("Symétrie", screw, |n| {
            n.mirror.x = true;
            n.mirror.z = true;
        });
        // 5. a hex boss on the front face
        h.st.tool = Tool::Place(PrimKind::Prism);
        let fy = top * 0.5;
        let f = h.screen_of(Vec3::new(14.0, fy, 26.0));
        let f2 = h.screen_of(Vec3::new(21.0, fy, 26.0));
        h.drag(f, f2, 6, true);
        h.drag(f2, f2 + egui::vec2(0.0, -30.0), 6, true);
        h.st.tool = Tool::Select;
        h.steps(2);
        let names: Vec<String> = h.st.scene.rows().iter().map(|(id, _)| h.st.scene.node(*id).unwrap().name.clone()).collect();
        eprintln!("arbre : {names:?}");
        std::fs::write(format!("{}/docs/boitier.json", h.out_dir), h.st.scene.to_json()).ok();
        h.shot("06-nouvel-objet");
    }

    if want("07") || want("08") {
        h.reset(AppState::default());
        h.st.export_open = true;
        h.st.export_res = forme_app::state::ExportRes::Normal;
        h.steps(40);
        if want("07") {
            h.shot("07-export");
        }
        let t0 = Instant::now();
        h.st.start_export();
        while h.st.export_job.is_some() {
            std::thread::sleep(std::time::Duration::from_millis(200));
            h.step();
        }
        eprintln!("export : {:.1?} → {}", t0.elapsed(), h.st.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default());
        if want("08") {
            let prog = forme_core::compile(&h.st.scene);
            let (lo, hi) = h.st.scene.bounds();
            let cell = (hi - lo).max_element() / 200.0;
            let mesh = forme_core::mesh::dual_contour(&prog, lo, hi, cell);
            let path = format!("{}/08-maillage-export.png", h.out_dir);
            raster_mesh(&mesh, &h.st, 1600, 1120, &path);
        }
    }
    let _ = (Tool::Select, demo::starter);
    let _: Option<_S> = None;
}

/// Minimal software rasteriser: flat shading + wireframe, to show the exported topology.
fn raster_mesh(mesh: &forme_core::mesh::Mesh, st: &AppState, w: u32, h: u32, path: &str) {
    let mut cam = st.camera;
    let (lo, hi) = st.scene.bounds();
    cam.frame(lo, hi);
    cam.dist *= 0.8;
    cam.target.y += 4.0;
    cam.target.x += 4.0;
    let view = cam.view([w as f32, h as f32], [w as f32 * 0.5, h as f32 * 0.5]);
    let (fwd, _, _) = view.basis();
    let mut color = vec![[23u8, 24, 27]; (w * h) as usize];
    let mut depth = vec![f32::MAX; (w * h) as usize];
    let proj: Vec<Option<([f32; 2], f32)>> =
        mesh.positions.iter().map(|p| view.project(*p).map(|s| (s, (*p - view.eye).dot(fwd)))).collect();
    let light = Vec3::new(-0.5, 0.8, 0.6).normalize();
    let mats = &st.scene.materials;
    let mut edges: Vec<(usize, usize)> = vec![];
    for t in mesh.indices.chunks(3) {
        let (ia, ib, ic) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let (Some(a), Some(b), Some(c)) = (proj[ia], proj[ib], proj[ic]) else { continue };
        let n = (mesh.positions[ib] - mesh.positions[ia]).cross(mesh.positions[ic] - mesh.positions[ia]).normalize_or_zero();
        if n.dot(fwd) > 0.0 {
            continue;
        }
        let m = mats.get(mesh.materials[ia] as usize).map(|m| m.color).unwrap_or([0.6; 3]);
        let k = 0.28 + 0.72 * n.dot(light).max(0.0);
        let col = [(m[0] * k * 255.0) as u8, (m[1] * k * 255.0) as u8, (m[2] * k * 255.0) as u8];
        let minx = a.0[0].min(b.0[0]).min(c.0[0]).floor().max(0.0) as i32;
        let maxx = a.0[0].max(b.0[0]).max(c.0[0]).ceil().min(w as f32 - 1.0) as i32;
        let miny = a.0[1].min(b.0[1]).min(c.0[1]).floor().max(0.0) as i32;
        let maxy = a.0[1].max(b.0[1]).max(c.0[1]).ceil().min(h as f32 - 1.0) as i32;
        let area = (b.0[0] - a.0[0]) * (c.0[1] - a.0[1]) - (b.0[1] - a.0[1]) * (c.0[0] - a.0[0]);
        if area.abs() < 1e-6 {
            continue;
        }
        for y in miny..=maxy {
            for x in minx..=maxx {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((b.0[0] - px) * (c.0[1] - py) - (b.0[1] - py) * (c.0[0] - px)) / area;
                let w1 = ((c.0[0] - px) * (a.0[1] - py) - (c.0[1] - py) * (a.0[0] - px)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * a.1 + w1 * b.1 + w2 * c.1;
                let i = (y as u32 * w + x as u32) as usize;
                if z < depth[i] {
                    depth[i] = z;
                    color[i] = col;
                }
            }
        }
        edges.extend_from_slice(&[(ia, ib), (ib, ic), (ic, ia)]);
    }
    for (ia, ib) in edges {
        let (Some(a), Some(b)) = (proj[ia], proj[ib]) else { continue };
        let len = ((b.0[0] - a.0[0]).hypot(b.0[1] - a.0[1])).ceil().max(1.0) as i32;
        for s in 0..=len {
            let t = s as f32 / len as f32;
            let x = a.0[0] + (b.0[0] - a.0[0]) * t;
            let y = a.0[1] + (b.0[1] - a.0[1]) * t;
            if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
                continue;
            }
            let i = (y as u32 * w + x as u32) as usize;
            let z = a.1 + (b.1 - a.1) * t;
            if z <= depth[i] + 0.15 {
                let c = &mut color[i];
                *c = [(c[0] as f32 * 0.55) as u8, (c[1] as f32 * 0.55) as u8, (c[2] as f32 * 0.55) as u8];
            }
        }
    }
    let px: Vec<u8> = color.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect();
    image::save_buffer(path, &px, w, h, image::ExtendedColorType::Rgba8).expect("png");
    eprintln!("{path}  ({} triangles)", mesh.triangle_count());
}
