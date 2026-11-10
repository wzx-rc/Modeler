//! Headless screenshots of the real app: drives it with synthetic input events
//! (Pencil, fingers, taps, drags) and renders frames with the same code as iPad.

use egui::{pos2, Event, Modifiers, PointerButton, Pos2, RawInput, Rect};
use forme_app::gfx::FrameRenderer;
use forme_app::settings::UiMode;
use forme_app::state::{AppState, Handle, PickMode, PointerKind, Tool};
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
            // IPAD_ZOOM: the device's UI zoom (layout in more points, fewer pixels per point)
            size: {
                let z: f32 = std::env::var("IPAD_ZOOM").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
                [1194.0 / z, 834.0 / z]
            },
            ppp: 2.0 * std::env::var("IPAD_ZOOM").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0),
            pending: vec![],
            out_dir: out_dir.to_string(),
        }
    }

    fn reset(&mut self, st: AppState) {
        self.st = st;
        self.st.autosave = false;
        self.st.screen = self.size;
    }

    /// PC window size and interface, for the desktop scenarios.
    fn pc(&mut self, st: AppState, mode: UiMode) {
        self.size = [1600.0, 900.0];
        self.reset(st);
        self.st.settings.ui_mode = mode;
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
        let scale = self.fr.full_scale;
        self.shot_at(name, 2.0, scale);
    }

    fn shot_at(&mut self, name: &str, ppp: f32, scale: f32) {
        let (old_ppp, old_scale) = (self.ppp, self.fr.full_scale);
        self.ppp = ppp;
        self.fr.full_scale = scale;
        self.render_png(name);
        self.ppp = old_ppp;
        self.fr.full_scale = old_scale;
    }

    fn render_png(&mut self, name: &str) {
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
        if let Some(dir) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(dir).ok();
        }
        image::save_buffer(&path, &px, w, h, image::ExtendedColorType::Rgba8).expect("png");
        eprintln!("{path}  ({:.1?})", t0.elapsed());
    }

    /// One app frame through the interactive path (progressive refinement),
    /// as on iPad; returns the wall time and saves the image if `name`.
    fn frame_live(&mut self, name: Option<&str>) -> f32 {
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
        self.fr.render(&self.gpu.device, &self.gpu.queue, &view, [w, h], self.ppp, &mut self.st, &paint, &mut out.textures_delta, false);
        let px = self.gpu.read_rgba(&tex);
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        if let Some(name) = name {
            let path = format!("{}/{}.png", self.out_dir, name);
            image::save_buffer(&path, &px, w, h, image::ExtendedColorType::Rgba8).expect("png");
        }
        ms
    }

    /// One interactive frame, timed until the GPU is done (no readback).
    fn frame_timed(&mut self) -> f32 {
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
        self.fr.render(&self.gpu.device, &self.gpu.queue, &view, [w, h], self.ppp, &mut self.st, &paint, &mut out.textures_delta, false);
        let _ = self.gpu.device.poll(wgpu::PollType::wait_indefinitely());
        t0.elapsed().as_secs_f32() * 1000.0
    }

    /// Orbits the camera for `n` frames at full resolution: (mean, worst) ms.
    fn orbit_bench(&mut self, n: usize) -> (f32, f32) {
        let _ = self.frame_timed();
        let mut times = vec![];
        for _ in 0..n {
            self.st.camera.orbit(6.0, 0.0);
            self.st.view_changed();
            times.push(self.frame_timed());
        }
        let mean = times.iter().sum::<f32>() / n as f32;
        let worst = times.iter().cloned().fold(0.0, f32::max);
        (mean, worst)
    }

    fn cam(&mut self, target: Vec3, dist: f32, yaw: f32, pitch: f32) {
        self.st.camera.ortho = false;
        self.st.camera.target = target;
        self.st.camera.dist = dist;
        self.st.camera.yaw = yaw;
        self.st.camera.pitch = pitch;
        self.st.view_changed();
        self.steps(2);
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
    if want("10") {
        bake_grenade(&mut h);
    }
    if want("19") {
        bake_cube(&mut h);
    }
    if want("max") {
        // 3ds Max style creation at the finger: base drag, release, height drag
        h.reset(AppState::new(forme_core::Scene::new(), "Sans titre"));
        h.cam(Vec3::new(0.0, 0.0, 0.0), 120.0, 0.7, 0.55);
        h.st.pointer_kind = forme_app::state::PointerKind::Finger;
        h.st.tool = forme_app::state::Tool::Place(PrimKind::Box);
        h.steps(3);
        let a = h.screen_of(Vec3::new(-15.0, 0.0, -10.0));
        let b = h.screen_of(Vec3::new(15.0, 0.0, 12.0));
        h.drag(a, b, 10, false);
        h.steps(2);
        h.shot("max-1-base");
        h.press(b, false);
        h.step();
        let p = h.st.placement.clone();
        eprintln!("après la base : {:?}", p.as_ref().map(|p| (p.phase, p.hx, p.hz, p.height)));
        h.shot("max-2-attente-hauteur");
        h.drag(b, b - egui::vec2(0.0, 160.0), 10, false);
        h.steps(2);
        h.shot("max-3-hauteur");
        h.press(b - egui::vec2(0.0, 160.0), false);
        h.steps(3);
        let id = h.st.scene.nodes.values().find(|n| n.shape().is_some()).map(|n| n.id);
        eprintln!(
            "boîte : {:?} · outil {:?}",
            id.and_then(|i| h.st.scene.node(i)).and_then(|n| n.shape()).map(|s| s.size),
            h.st.tool
        );
        // then a cylinder carved into its top, by tap (default size) then a height drag
        h.st.tool = forme_app::state::Tool::Place(PrimKind::Cylinder);
        h.st.carve_held = true;
        let top = h.screen_of(Vec3::new(0.0, 999.0, 0.0).min(Vec3::new(0.0, 20.0, 0.0)));
        let _ = top;
        h.steps(2);
        h.shot("max-4-fini");
    }
    if want("pc-atelier") || want("pc-studio") || want("pc-creation") || want("pc-perf") || want("pc-snap") || want("pc-menu") || want("pc-marquee") || want("pc-prims") || want("pc-nav") || want("pc-gizmo") || want("pc-ui") || want("bake-diag") {
        pc_shots(&mut h, &want);
    }
    if want("ui-ipad") {
        // the layout as seen on the device (run with IPAD_ZOOM=0.84)
        h.reset(AppState::default());
        let id = h.find("Bossage");
        h.st.select(Some(id));
        h.steps(6);
        h.shot("ui-ipad-selection");
        h.st.tool = forme_app::state::Tool::Place(PrimKind::Box);
        h.st.select(None);
        h.steps(6);
        h.shot("ui-ipad-placement");
    }
    if want("live") {
        // the iPad path: pixel budgets, quick image while moving, refinement in bands
        h.reset(AppState::default());
        h.fr.full_px = 2.4e6;
        h.fr.interactive_px = 4.5e5;
        h.fr.band_px = 3.0e5;
        let mut n = 0;
        let mut times = vec![];
        while n < 200 {
            times.push(h.frame_live(None));
            n += 1;
            if !h.fr.needs_refine() {
                break;
            }
        }
        eprintln!("immobile : image complète après {n} frames, {:?} ms", times.iter().map(|t| *t as u32).collect::<Vec<_>>());
        h.frame_live(Some("live-immobile"));
        // orbit: quick image every frame
        let c = egui::pos2(600.0, 420.0);
        h.st.pointer_kind = forme_app::state::PointerKind::Finger;
        let mut times = vec![];
        h.st.interacting = true;
        for i in 0..12 {
            h.st.camera.yaw += 0.03;
            h.st.view_changed();
            times.push(h.frame_live(None));
            let _ = (i, c);
        }
        h.st.interacting = false;
        eprintln!("en mouvement : {:?} ms", times.iter().map(|t| *t as u32).collect::<Vec<_>>());
        let mut n = 0;
        while h.fr.needs_refine() && n < 200 {
            h.frame_live(None);
            n += 1;
        }
        eprintln!("raffinage après mouvement : {n} frames");
        h.frame_live(Some("live-apres"));
        // a blank document (first launch)
        h.reset(AppState::new(forme_core::Scene::new(), "Sans titre"));
        h.frame_live(None);
        for _ in 0..20 {
            if !h.fr.needs_refine() {
                break;
            }
            h.frame_live(None);
        }
        h.frame_live(Some("live-vierge"));
    }
    if want("22") {
        edges_cube(&mut h);
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

/// Desktop test part: a pillow block made only of cubes and cylinders,
/// with the three basic modes.
/// Cube with a cylinder carved at one vertical corner (no fillet).
fn coin_cylindre() -> forme_core::Scene {
    use forme_core::scene::{NodeSpec, Scene, Shape};
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::new(20.0, 20.0, 20.0);
    s.add(root, NodeSpec::shape("Cube", b).at(Vec3::new(0.0, 20.0, 0.0)));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(9.0, 22.0, 0.0);
    s.add(root, NodeSpec::shape("Cylindre", c).at(Vec3::new(20.0, 20.0, 20.0)).mode(CombineMode::Carve));
    s
}

/// Plate with a round hole through its middle (no fillet).
fn plaque_trou() -> forme_core::Scene {
    use forme_core::scene::{NodeSpec, Scene, Shape};
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::new(20.0, 10.0, 20.0);
    s.add(root, NodeSpec::shape("Plaque", b).at(Vec3::new(0.0, 10.0, 0.0)));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(6.0, 14.0, 0.0);
    s.add(root, NodeSpec::shape("Trou", c).at(Vec3::new(0.0, 10.0, 0.0)).mode(CombineMode::Carve));
    s
}

fn support_axe() -> forme_core::Scene {
    use forme_core::scene::{Mirror, NodeSpec, Scene, Shape};
    let mut s = Scene::new();
    let root = s.root;
    let lying = glam::Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::new(26.0, 5.0, 16.0);
    b.round = 1.5;
    s.add(root, NodeSpec::shape("Socle", b).at(Vec3::new(0.0, 5.0, 0.0)));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(11.0, 14.0, 0.0);
    c.round = 1.0;
    s.add(root, NodeSpec::shape("Palier", c).at(Vec3::new(0.0, 19.0, 0.0)).rot(lying).blend(2.0));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(6.0, 20.0, 0.0);
    s.add(root, NodeSpec::shape("Alésage", c).at(Vec3::new(0.0, 19.0, 0.0)).rot(lying).mode(CombineMode::Carve).blend(0.6));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(2.6, 8.0, 0.0);
    s.add(
        root,
        NodeSpec::shape("Trous de fixation", c)
            .at(Vec3::new(19.0, 5.0, 10.0))
            .mode(CombineMode::Carve)
            .blend(0.4)
            .mirror(Mirror { x: true, y: false, z: true }),
    );
    s
}

/// The two desktop interfaces (1600 × 900) on the test part, and the settings.
fn pc_shots(h: &mut Harness, want: &dyn Fn(&str) -> bool) {
    let part = || AppState::new(support_axe(), "Support d'axe");
    if want("pc-creation") {
        // 3ds Max style: base on the ground plane, then up = Add, down = Carve,
        // Ctrl swaps; mouse hover drives the height after the release
        h.pc(part(), UiMode::Atelier);
        h.st.pointer_kind = PointerKind::Mouse;
        h.steps(2);
        h.st.frame_all();
        h.st.tool = Tool::Place(PrimKind::Box);
        h.steps(4);
        let a = h.screen_of(Vec3::new(-20.0, 0.0, 30.0));
        let b = h.screen_of(Vec3::new(20.0, 0.0, 50.0));
        h.press(a, true);
        for i in 1..=12 {
            h.move_to(a + (b - a) * (i as f32 / 12.0));
        }
        h.shot("pc-creation-1-base");
        h.press(b, false);
        h.step();
        let report = |h: &Harness, what: &str| {
            let p = h.st.placement.as_ref().unwrap();
            eprintln!("{what} : hauteur {:.1} mm · {}", p.height, p.mode.label());
        };
        h.move_to(b + egui::vec2(0.0, -2.0));
        for i in 1..=12 {
            h.move_to(b + egui::vec2(0.0, -2.0 - 12.0 * i as f32));
        }
        report(h, "monte");
        h.shot("pc-creation-2-monte-add");
        h.st.ctrl_held = true;
        h.steps(2);
        report(h, "monte + Ctrl");
        h.shot("pc-creation-3-ctrl-carve");
        h.st.ctrl_held = false;
        for i in 1..=24 {
            h.move_to(b + egui::vec2(0.0, -146.0 + 12.0 * i as f32));
        }
        report(h, "descend");
        h.shot("pc-creation-4-descend-carve");
        h.st.display_open = true;
        h.st.display_anchor = [520.0, 56.0];
        h.steps(4);
        h.shot("pc-affichage");
        h.st.display_open = false;
    }
    if want("pc-marquee") {
        // Select tool + mouse: left drag draws a selection rectangle
        for (tag, mode) in [("atelier", UiMode::Atelier), ("studio", UiMode::Studio)] {
            h.pc(part(), mode);
            h.st.pointer_kind = PointerKind::Mouse;
            h.steps(2);
            h.st.frame_all();
            h.steps(2);
            let (lo, hi) = h.st.scene.bounds();
            let mut sb = egui::Rect::NOTHING;
            for i in 0..8 {
                let c = Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
                sb.extend_with(h.screen_of(c));
            }
            let a = sb.left_top() + egui::vec2(-24.0, -24.0);
            let b = sb.center() + egui::vec2(sb.width() * 0.05, sb.height() * 0.12);
            h.drag(a, b, 10, false);
            h.steps(2);
            h.shot(&format!("pc-marquee-1-rectangle-{tag}"));
            let t = std::time::Instant::now();
            h.press(b, false);
            eprintln!("rectangle ({tag}) : {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
            h.steps(3);
            let names: Vec<String> = h.st.selected_ids().iter().filter_map(|id| h.st.scene.node(*id).map(|n| n.name.clone())).collect();
            eprintln!("rectangle ({tag}) : {} formes : {}", names.len(), names.join(", "));
            h.shot(&format!("pc-marquee-2-selection-{tag}"));
        }
        // edge mode: the edges seen inside the rectangle
        h.st.set_pick_mode(PickMode::Edge);
        h.steps(2);
        let (lo, hi) = h.st.scene.bounds();
        let mut sb = egui::Rect::NOTHING;
        for i in 0..8 {
            let c = Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
            sb.extend_with(h.screen_of(c));
        }
        // the upper half: bearing, its bore, the fixing holes
        let a = sb.left_top() + egui::vec2(-20.0, -20.0);
        let b = sb.right_center() + egui::vec2(20.0, 30.0);
        h.drag(a, b, 10, false);
        h.steps(2);
        h.shot("pc-marquee-3-aretes-rectangle");
        let t = std::time::Instant::now();
        h.press(b, false);
        eprintln!("rectangle (arêtes) : {} arêtes en {:.0} ms", h.st.edge_sel.len(), t.elapsed().as_secs_f64() * 1000.0);
        h.steps(3);
        h.shot("pc-marquee-4-aretes");
    }
    if want("pc-menu") {
        // marking menu: right button held still on the palier opens it,
        // release on "Passer en Carve" (east) runs it; then frozen captures
        h.pc(part(), UiMode::Atelier);
        h.st.pointer_kind = PointerKind::Mouse;
        h.steps(2);
        h.st.frame_all();
        h.steps(2);
        let c = h.screen_of(Vec3::new(0.0, 30.0, 0.0));
        let right = |h: &mut Harness, p: Pos2, down: bool| {
            h.pending.push(Event::PointerMoved(p));
            h.pending.push(Event::PointerButton { pos: p, button: PointerButton::Secondary, pressed: down, modifiers: Modifiers::NONE });
            h.step();
        };
        right(h, c, true);
        h.steps(20); // 0.33 s still
        eprintln!("menu : ouvert {} · contexte {:?}", h.st.marking.is_open(), h.st.marking.menu.as_ref().map(|m| m.ctx));
        h.move_to(c + egui::vec2(60.0, 0.0));
        right(h, c + egui::vec2(60.0, 0.0), false);
        h.steps(2);
        let id = h.find("Palier");
        eprintln!("menu : Palier en {}", h.st.scene.node(id).unwrap().combine.mode.label());
        h.st.undo();
        h.steps(2);
        // frozen captures: on a shape, then in empty space
        use forme_app::marking::{Ctx, Menu};
        let empty = h.screen_of(Vec3::new(-55.0, 0.0, 30.0));
        for (name, at, ctx, arc) in [("pc-menu-forme", c, Ctx::Shape(id), false), ("pc-menu-matiere", c, Ctx::Shape(id), true), ("pc-menu-scene", empty, Ctx::Empty, false)] {
            if let Ctx::Shape(id) = ctx {
                h.st.select(Some(id));
            } else {
                h.st.select(None);
            }
            h.st.marking.demo = true;
            let mut m = Menu::new([at.x, at.y], ctx, false, 0.0);
            m.pointer = [at.x + 58.0, at.y - 4.0];
            m.hover = Some(2);
            if arc {
                m.arc = Some(0.0);
                m.swatch = Some(2);
                m.hover = Some(0);
                m.pointer = [at.x + 20.0, at.y - 125.0];
            }
            h.st.marking.menu = Some(m);
            h.steps(3);
            h.shot(name);
            h.st.marking.menu = None;
            h.st.marking.demo = false;
        }
        // shortcut letters and the creation popover, both interfaces
        for (name, mode, anchor) in [("pc-creation-options", UiMode::Atelier, [84.0, 248.0]), ("pc-creation-options-studio", UiMode::Studio, [230.0, 92.0])] {
            h.pc(part(), mode);
            h.steps(2);
            h.st.frame_all();
            h.st.tool = Tool::Place(PrimKind::Box);
            h.st.creation_open = true;
            h.st.creation_anchor = anchor;
            h.steps(4);
            h.shot(name);
            h.st.creation_open = false;
        }
        // direct creation: one click drops a cube, selected with its handles
        h.pc(part(), UiMode::Atelier);
        h.st.pointer_kind = PointerKind::Mouse;
        h.st.settings.creation = forme_app::settings::Creation::Direct;
        h.steps(2);
        h.st.frame_all();
        h.st.tool = Tool::Place(PrimKind::Box);
        h.steps(2);
        let n0 = h.st.scene.nodes.len();
        h.tap(h.screen_of(Vec3::new(-45.0, 0.0, 25.0)));
        eprintln!("création directe : {} forme(s) ajoutée(s), outil {:?}, sélection {:?}", h.st.scene.nodes.len() - n0, h.st.tool, h.st.selection.and_then(|id| h.st.scene.node(id)).map(|n| n.name.clone()));
        h.st.settings.creation = forme_app::settings::Creation::Interactive;
    }
    if want("pc-prims") {
        // the five primitives created on the ground (base drag, then height)
        h.pc(AppState::new(forme_core::Scene::new(), "Primitives"), UiMode::Atelier);
        h.st.pointer_kind = PointerKind::Mouse;
        h.st.snap = false;
        h.steps(2);
        for (i, k) in forme_app::state::PRIMS.iter().enumerate() {
            h.st.tool = Tool::Place(*k);
            h.steps(2);
            let c = Vec3::new(-48.0 + 24.0 * i as f32, 0.0, -30.0);
            let a = h.screen_of(c);
            let b = h.screen_of(c + Vec3::new(8.0, 0.0, 6.0));
            h.press(a, true);
            for s in 1..=10 {
                h.move_to(a + (b - a) * (s as f32 / 10.0));
            }
            h.press(b, false);
            h.step();
            if h.st.placement.is_some() {
                for s in 1..=10 {
                    h.move_to(b + egui::vec2(0.0, -5.0 * s as f32));
                }
                h.st.commit_placement();
            }
            h.steps(2);
            let id = h.st.selection.unwrap();
            let n = h.st.scene.node(id).unwrap();
            let sh = n.shape().unwrap();
            let pos = h.st.scene.world_matrix(id).transform_point3(Vec3::ZERO);
            let lo = h.st.scene.bounds().0.y;
            eprintln!("{:?} : taille {:.1?} · centre y {:.2} · bas de la scène {:.2} · {}", k, sh.size, pos.y, lo, n.combine.mode.label());
        }
        h.st.select(None);
        h.st.frame_all();
        h.steps(4);
        h.shot("pc-prims");
        // regression: a torus started very close (tiny base) used to panic
        // in clamp(min > max)
        h.st.camera.dist = 1.5;
        h.st.view_changed();
        h.st.tool = Tool::Place(PrimKind::Torus);
        h.steps(2);
        let c = pos2(700.0, 450.0);
        h.press(c, true);
        for s in 1..=6 {
            h.move_to(c + egui::vec2(s as f32, 0.0));
        }
        h.press(c + egui::vec2(6.0, 0.0), false);
        h.step();
        h.st.commit_placement();
        h.steps(2);
        eprintln!("tore très zoomé : pas de plantage");
    }    if want("pc-ui") {
        // the two interfaces of the redesign, same scene and same moments:
        // nothing selected, the Palier selected, a creation tool armed
        for (tag, mode) in [("etabli", UiMode::Atelier), ("studio", UiMode::Studio)] {
            h.pc(AppState::new(support_axe(), "Support d'axe"), mode);
            h.st.pointer_kind = PointerKind::Mouse;
            h.steps(2);
            h.st.frame_all();
            h.steps(4);
            h.shot(&format!("pc-ui-{tag}-1-scene"));
            let id = h.find("Palier");
            h.st.select(Some(id));
            h.steps(4);
            h.shot(&format!("pc-ui-{tag}-2-selection"));
            h.st.select(None);
            h.st.tool = Tool::Place(PrimKind::Box);
            h.steps(4);
            h.shot(&format!("pc-ui-{tag}-3-creation"));
            h.st.tool = Tool::Select;
            // the marking menu of the Palier (radial pie in Établi)
            h.st.select(Some(id));
            h.st.marking.demo = true;
            let at = h.screen_of(Vec3::new(-30.0, 30.0, 0.0));
            let mut m = forme_app::marking::Menu::new([at.x, at.y], forme_app::marking::Ctx::Shape(id), true, 0.0);
            m.pointer = [at.x + 70.0, at.y - 60.0];
            m.hover = Some(1);
            m.shown = true;
            h.st.marking.menu = Some(m);
            h.steps(3);
            h.shot(&format!("pc-ui-{tag}-3b-menu"));
            h.st.marking.menu = None;
            h.st.marking.demo = false;
            h.st.select(None);            if mode == UiMode::Studio {
                let key = |h: &mut Harness, k: egui::Key| {
                    h.pending.push(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
                    h.step();
                    h.pending.push(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
                    h.step();
                };
                // Espace: the command palette, « cyl », Entrée arms the cylinder
                h.move_to(pos2(700.0, 450.0));
                key(h, egui::Key::Space);
                h.steps(2);
                h.pending.push(Event::Text("cyl".into()));
                h.steps(3);
                h.shot("pc-ui-studio-4-palette");
                key(h, egui::Key::Enter);
                h.steps(3);
                eprintln!("palette : outil {:?}", h.st.tool);
                // the sizes panel: Entrée creates a cylinder of the typed size
                let n0 = h.st.scene.shape_count();
                h.steps(2);
                h.pending.push(Event::Text("12".into()));
                h.steps(2);
                h.shot("pc-ui-studio-5-cotes");
                key(h, egui::Key::Enter);
                h.steps(3);
                let sz = h.st.selection.and_then(|id| h.st.scene.node(id)).and_then(|n| n.shape()).map(|s| s.size);
                eprintln!("cotes : {} → {} formes, taille {:?}, outil {:?}", n0, h.st.scene.shape_count(), sz, h.st.tool);
                h.steps(4);
                h.shot("pc-ui-studio-6-cree");
            }
        }
    }
    if want("pc-gizmo") {
        // transform tools: équerres far from the centre, pre-selection
        // highlight, size handles anchored on the opposite face (Ctrl: centre)
        h.pc(AppState::new(forme_core::Scene::new(), "Gizmo"), UiMode::Atelier);
        h.st.pointer_kind = PointerKind::Mouse;
        h.st.snap = false;
        h.steps(2);
        h.st.tool = Tool::Place(PrimKind::Box);
        h.steps(2);
        let a = h.screen_of(Vec3::new(-10.0, 0.0, -8.0));
        let b = h.screen_of(Vec3::new(10.0, 0.0, 8.0));
        h.drag(a, b, 10, true);
        if h.st.placement.is_some() {
            for s in 1..=10 {
                h.move_to(b + egui::vec2(0.0, -6.0 * s as f32));
            }
            h.st.commit_placement();
        }
        h.steps(2);
        h.st.frame_selection();
        h.st.camera.pitch = 0.42;
        h.st.camera.dist *= 1.6;
        h.st.view_changed();
        h.steps(4);
        let id = h.st.selection.unwrap();
        let faces = |h: &Harness| {
            let sh = h.st.scene.node(id).unwrap().shape().unwrap().clone();
            let c = h.st.scene.world_matrix(id).transform_point3(Vec3::ZERO);
            (c.x - sh.size.x, c.x + sh.size.x)
        };
        let at = |h: &Harness, k: Handle| h.st.handles().into_iter().find(|i| i.handle == k).map(|i| h.screen_of(i.world)).unwrap();
        let pl = at(h, Handle::PlaneXZ);
        h.move_to(pl);
        h.steps(2);
        eprintln!("survol équerre : {:?}", h.st.handle_hover);
        h.shot("pc-gizmo-1-equerre");
        let mx = at(h, Handle::MoveX);
        h.move_to(mx);
        h.steps(2);
        eprintln!("survol flèche : {:?}", h.st.handle_hover);
        h.shot("pc-gizmo-2-fleche");
        let r = at(h, Handle::Radius);
        h.move_to(r);
        h.steps(2);
        eprintln!("survol taille : {:?}", h.st.handle_hover);
        h.shot("pc-gizmo-3-taille");
        let f0 = faces(h);
        h.drag(r, r + egui::vec2(90.0, 0.0), 10, false);
        let f1 = faces(h);
        h.shot("pc-gizmo-4-ancre");
        h.press(r + egui::vec2(90.0, 0.0), false);
        h.steps(2);
        eprintln!("ancre : faces x {:.2?} → {:.2?} (la face −X doit rester)", f0, f1);
        h.st.ctrl_held = true;
        let r = at(h, Handle::Radius);
        let f0 = faces(h);
        h.drag(r, r + egui::vec2(60.0, 0.0), 10, true);
        h.st.ctrl_held = false;
        h.steps(2);
        eprintln!("Ctrl symétrique : faces x {:.2?} → {:.2?} (centre fixe)", f0, faces(h));
    }    if want("pc-nav") {
        // Tripo3D navigation: left drag in empty space orbits, right drag pans
        // (no marking menu), Shift + left drag draws the selection rectangle
        h.pc(part(), UiMode::Atelier);
        h.st.pointer_kind = PointerKind::Mouse;
        h.st.settings.navigation = forme_app::settings::Navigation::Tripo;
        h.steps(2);
        h.st.frame_all();
        h.steps(2);
        let empty = pos2(400.0, 250.0);
        let (yaw0, t0) = (h.st.camera.yaw, h.st.camera.target);
        h.drag(empty, empty + egui::vec2(80.0, 0.0), 8, true);
        eprintln!("tripo gauche : lacet {:.2} → {:.2}, rectangle {}", yaw0, h.st.camera.yaw, h.st.marquee.is_some());
        let right = |h: &mut Harness, p: Pos2, down: bool| {
            h.pending.push(Event::PointerMoved(p));
            h.pending.push(Event::PointerButton { pos: p, button: PointerButton::Secondary, pressed: down, modifiers: Modifiers::NONE });
            h.step();
        };
        right(h, empty, true);
        for s in 1..=8 {
            h.move_to(empty + egui::vec2(0.0, 10.0 * s as f32));
        }
        right(h, empty + egui::vec2(0.0, 80.0), false);
        h.steps(2);
        eprintln!("tripo droit : cible {:.1?} → {:.1?}, menu ouvert {}", t0, h.st.camera.target, h.st.marking.is_open());
        // a right click without moving opens the menu, which stays open
        let t1 = h.st.camera.target;
        right(h, empty, true);
        right(h, empty, false);
        h.steps(3);
        eprintln!("tripo clic droit : menu ouvert {} (caméra immobile {})", h.st.marking.is_open(), (h.st.camera.target - t1).length() < 1e-3);
        h.shot("pc-nav-menu");
        h.press(empty + egui::vec2(0.0, -200.0), true);
        h.press(empty + egui::vec2(0.0, -200.0), false);
        h.steps(2);
        eprintln!("tripo clic suivant : menu ouvert {}, geste {:?}", h.st.marking.is_open(), h.st.gesture);
        // F: the camera glides to the framing instead of jumping
        h.st.select(h.st.scene.nodes.keys().next().copied());
        h.st.camera.dist *= 4.0;
        h.st.view_changed();
        h.step();
        let d0 = h.st.camera.dist;
        h.st.frame_selection();
        let mut ds = vec![];
        for _ in 0..32 {
            h.step();
            ds.push(h.st.camera.dist);
        }
        eprintln!("F : distance {:.0} → {:.0?} (animation {})", d0, ds.iter().step_by(4).collect::<Vec<_>>(), h.st.cam_anim.is_none());
        h.st.settings.navigation = forme_app::settings::Navigation::Maya;
    }    if want("pc-snap") {
        // base on the top face of the socle, far corner snapped to its edge
        h.pc(part(), UiMode::Atelier);
        h.st.pointer_kind = PointerKind::Mouse;
        h.steps(2);
        h.st.frame_all();
        h.st.tool = Tool::Place(PrimKind::Box);
        h.steps(4);
        let a = h.screen_of(Vec3::new(14.0, 10.0, 4.0));
        let b = h.screen_of(Vec3::new(25.8, 10.0, 15.6));
        h.press(a, true);
        for i in 1..=12 {
            h.move_to(a + (b - a) * (i as f32 / 12.0));
        }
        let p = h.st.placement.clone().unwrap();
        eprintln!("snap : début {:?} normale {:.2?} · fin {:?}", p.snapped, p.normal, p.snap_end.map(|(w, s)| (w, s)));
        h.shot("pc-snap-1-coin");
        h.press(b, false);
        h.step();
        for i in 1..=10 {
            h.move_to(b + egui::vec2(0.0, -6.0 * i as f32));
        }
        let p = h.st.placement.clone().unwrap();
        eprintln!("snap : hauteur {:.1} · {}", p.height, p.mode.label());
        h.shot("pc-snap-2-hauteur");
        h.st.commit_placement();
        // object mode: a click on the palier selects it, no edge
        h.st.set_pick_mode(PickMode::Object);
        let c = h.screen_of(Vec3::new(0.0, 30.0, 0.0));
        h.tap(c);
        eprintln!("mode objet : sélection {:?}, arêtes {}", h.st.selection.and_then(|id| h.st.scene.node(id)).map(|n| n.name.clone()), h.st.edge_sel.len());
        h.st.set_pick_mode(PickMode::Edge);
        h.tap(c);
        eprintln!("mode arête : arêtes {}", h.st.edge_sel.len());
        h.shot("pc-snap-3-mode-arete");
    }
    if want("pc-perf") {
        let scenes: [(&str, AppState); 2] = [("support", part()), ("grenade", AppState::default())];
        for (name, scene) in scenes {
            for mode in [UiMode::Atelier, UiMode::Studio] {
                h.pc(AppState::new(scene.scene.clone(), name), mode);
                h.ppp = 2.0;
                h.steps(2);
                h.st.frame_all();
                for (sh, ao) in [(true, true), (false, true), (true, false), (false, false)] {
                    h.st.settings.display.shadows = sh;
                    h.st.settings.display.ao = ao;
                    let (mean, worst) = h.orbit_bench(60);
                    eprintln!("perf {name} {} ombres {sh} AO {ao} : {:.0}×{:.0} px · orbite {:.2} ms en moyenne, {:.2} ms au pire", mode.label(), h.size[0] * h.ppp, h.size[1] * h.ppp, mean, worst);
                }
                if mode == UiMode::Studio {
                    break;
                }
            }
        }
    }
    if want("pc-atelier") {
        h.pc(part(), UiMode::Atelier);
        h.steps(2);
        h.st.frame_all();
        let id = h.find("Palier");
        h.st.select(Some(id));
        h.steps(8);
        h.shot("pc-atelier");
        h.st.settings_open = true;
        h.steps(8);
        h.shot("pc-reglages");
        h.st.settings_open = false;
        h.st.select(None);
        h.st.tool = Tool::Place(PrimKind::Cylinder);
        h.steps(6);
        h.shot("pc-atelier-creation");
    }
    if want("bake-diag") {
        // Romain's scene (box with a carved, filleted corner) and the test part:
        // construction lines, then the bake coloured by role, both interfaces
        let romain = forme_core::Scene::from_json(include_str!("../../../forme-core/examples/scenes/romain-coin.forme")).expect("scène");
        let only = std::env::var("BAKE_DIAG").ok();
        let aretes = forme_core::Scene::from_json(include_str!("../../../forme-core/examples/scenes/romain-aretes.forme")).expect("scène");
        let cylindre = forme_core::Scene::from_json(include_str!("../../../forme-core/examples/scenes/romain-cylindre.forme")).expect("scène");
        let bossage = forme_core::Scene::from_json(include_str!("../../../forme-core/examples/scenes/romain-bossage.forme")).expect("scène");
        for (name, scene, mode) in [
            ("romain", romain.clone(), UiMode::Atelier),
            ("romain-aretes", aretes.clone(), UiMode::Atelier),
            ("romain-cylindre", cylindre, UiMode::Atelier),
            ("romain-bossage", bossage, UiMode::Atelier),
            ("coin", coin_cylindre(), UiMode::Atelier),
            ("trou", plaque_trou(), UiMode::Atelier),
            ("support", support_axe(), UiMode::Atelier),
            ("romain-studio", romain, UiMode::Studio),
            ("romain-aretes-studio", aretes, UiMode::Studio),
            ("support-studio", support_axe(), UiMode::Studio),
        ] {
            if only.as_deref().is_some_and(|o| o != name) {
                continue;
            }
            h.pc(AppState::new(scene, name), mode);
            h.steps(2);
            h.st.frame_all();
            if name.starts_with("romain") {
                let t = h.st.camera.target;
                let d = h.st.camera.dist;
                h.cam(t, d, -2.4, 0.55);
            }
            if name == "coin" || name == "trou" {
                let t = h.st.camera.target;
                let d = h.st.camera.dist;
                h.cam(t, d, 0.75, 0.55);
            }
            let wait = |h: &mut Harness| {
                for _ in 0..600 {
                    h.step();
                    std::thread::sleep(std::time::Duration::from_millis(15));
                    let st = &h.st;
                    if let Some((lv, _)) = &st.bake.lines {
                        if st.bake.lines_vis.key == (st.view_version, *lv) {
                            break;
                        }
                    }
                }
                h.steps(4);
            };
            // the same view with the panel closed: opening it must not move
            // the model (the panel floats over the 3D view)
            h.steps(4);
            h.shot(&format!("bake-diag-{name}-ferme"));
            h.st.set_bake_mode(true);
            wait(h);
            h.shot(&format!("bake-diag-{name}-lignes"));
            h.st.start_bake();
            while h.st.bake.job.is_some() {
                std::thread::sleep(std::time::Duration::from_millis(30));
                h.step();
            }
            h.st.bake.diag.roles = true;
            h.st.toast = None;
            wait(h);
            h.shot(&format!("bake-diag-{name}-roles"));
            if name == "romain-bossage" {
                // closer, from above: the round and its square of loops
                // against the face's edge, Cube 3 and the notch
                let d = h.st.camera.dist;
                h.cam(Vec3::new(38.8, 39.4, 47.0), d * 0.5, -2.4, 1.15);
                wait(h);
                h.shot(&format!("bake-diag-{name}-dessus"));
            }
        }
    }
    if want("pc-studio") {
        h.pc(part(), UiMode::Studio);
        h.steps(2);
        h.st.frame_all();
        let id = h.find("Palier");
        h.st.select(Some(id));
        h.steps(8);
        h.shot("pc-studio");
        h.st.select(None);
        h.st.tool = Tool::Place(PrimKind::Box);
        h.steps(6);
        h.shot("pc-studio-creation");
        h.st.tool = Tool::Select;
        h.st.bake.quality = forme_core::bake::Quality::Standard;
        h.st.start_bake();
        while h.st.bake.job.is_some() {
            std::thread::sleep(std::time::Duration::from_millis(50));
            h.step();
        }
        report_bake(h, "support");
        h.st.toast = None;
        h.steps(10);
        h.shot("pc-studio-bake");
        // optional second pass from the Studio bake box
        h.st.set_flat_pass(true);
        while h.st.bake.pass.is_some() {
            std::thread::sleep(std::time::Duration::from_millis(20));
            h.step();
        }
        h.st.toast = None;
        h.steps(10);
        h.shot("pc-studio-bake-passe");
        h.st.set_flat_pass(false);
    }
}

/// Bake of the rebuilt grenade: dialog, progress, result, wireframe, zoom,
/// exports, and the same zoom on the old voxel export for comparison.
fn cube_test() -> forme_core::Scene {
    use forme_core::scene::{NodeSpec, Scene, Shape};
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::splat(10.0);
    b.round = 1.0;
    s.add(root, NodeSpec::shape("Cube", b));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(4.0, 14.0, 0.0);
    s.add(root, NodeSpec::shape("Trou", c).mode(CombineMode::Carve).blend(0.8));
    s
}

/// Select mode: hover highlights a junction or one edge, tap selects it,
/// CTRL held adds more, the handle (or the panel) sets their fillet.
fn edges_cube(h: &mut Harness) {
    let sel = |h: &Harness| -> Vec<(String, f32)> { h.st.edge_sel.iter().map(|e| (e.label.clone(), e.radius)).collect() };
    h.reset(AppState::new(cube_test(), "Cube test"));
    h.cam(Vec3::new(0.0, 2.0, 0.0), 64.0, 0.72, 0.62);
    h.steps(4);
    // hover the hole rim
    let rim = h.screen_of(Vec3::new(3.45, 10.0, 3.45));
    h.move_to(rim);
    h.steps(4);
    eprintln!("survol : {:?}", h.st.edge_hover.as_ref().map(|e| (&e.label, e.radius)));
    h.shot("22-arete-survol");
    h.tap(rim);
    h.steps(4);
    eprintln!("sélection : {:?}", sel(h));
    h.shot("23-arete-selection");
    // pull the fillet handle
    let hd = h.st.handles()[0].clone();
    let a = h.screen_of(hd.world);
    let o = h.screen_of(hd.origin);
    let dir = (a - o).normalized();
    h.drag(a, a + dir * 70.0, 12, true);
    h.move_to(a + dir * 70.0 + egui::vec2(160.0, 120.0));
    h.steps(6);
    eprintln!("fillet tiré : {:?}", sel(h));
    h.shot("24-fillet-tire");
    // one edge of the cube: only that one lights up
    let e1 = h.screen_of(Vec3::new(9.75, 9.75, -4.0));
    h.move_to(e1);
    h.steps(4);
    eprintln!("survol arête : {:?}", h.st.edge_hover.as_ref().map(|e| (&e.label, e.radius, e.curves.len())));
    h.shot("25-arete-cube-survol");
    h.tap(e1);
    // CTRL held: add two more edges
    h.st.ctrl_held = true;
    let e2 = h.screen_of(Vec3::new(-4.0, 9.75, 9.75));
    let e3 = h.screen_of(Vec3::new(9.75, 0.0, 9.75));
    h.move_to(e2);
    h.steps(3);
    h.shot("26-ctrl-survol");
    h.tap(e2);
    h.tap(e3);
    h.st.ctrl_held = false;
    eprintln!("CTRL + taps : {:?}", sel(h));
    h.st.set_fillet(3.0, false, true);
    h.move_to(e3 + egui::vec2(-260.0, 160.0));
    h.steps(6);
    eprintln!("3 arêtes à 3 mm : {:?}", sel(h));
    h.shot("27-trois-aretes-fillet");
    // a chamfer on another edge, alone
    let e4 = h.screen_of(Vec3::new(-8.9, 0.0, 10.0));
    h.tap(e4);
    eprintln!("e4 {:?} → {:?}", e4, sel(h));
    h.st.set_fillet(1.5, true, true);
    let cube = h.find("Cube");
    eprintln!("arêtes du cube : {:?}", h.st.scene.node(cube).and_then(|n| n.shape()).map(|s| s.edges.clone()));
    h.st.clear_edge();
    h.st.select(None);
    h.move_to(egui::pos2(900.0, 700.0));
    h.cam(Vec3::new(0.0, 0.0, 0.0), 70.0, 0.25, 0.45);
    h.steps(8);
    h.shot("28-fillets-par-arete");
    // and the bake
    h.st.bake.open = false;
    h.st.start_bake();
    while h.st.bake.job.is_some() {
        std::thread::sleep(std::time::Duration::from_millis(50));
        h.step();
    }
    report_bake(h, "bake");
    h.st.toast = None;
    h.steps(8);
    h.shot("29-fillets-par-arete-bake");
}

/// The bake's outcome in the log: the mesh's counts, or why there is none.
fn report_bake(h: &Harness, tag: &str) {
    match (&h.st.bake.result, &h.st.bake.error) {
        (_, Some((_, why))) => eprintln!("{tag} : bake impossible · {why}"),
        (Some(b), None) => {
            let st = &b.stats;
            eprintln!(
                "{tag} : {} faces ({} q {} t {} n) · {} patchs · ouvertes {} nm {} · écart max {:.4} · {:.3} s",
                b.faces.len(),
                st.quads,
                st.tris,
                st.ngons,
                st.patches,
                st.open_edges,
                st.nonmanifold_edges,
                st.max_dev,
                st.seconds
            );
        }
        (None, None) => eprintln!("{tag} : pas de bake"),
    }
}

/// The test cube (bevel 1 mm, through hole Ø8, fillet 0.8) baked in the app.
fn bake_cube(h: &mut Harness) {
    use forme_core::scene::{NodeSpec, Scene, Shape};
    let mut s = Scene::new();
    let root = s.root;
    let mut b = Shape::new(PrimKind::Box);
    b.size = Vec3::splat(10.0);
    b.round = 1.0;
    s.add(root, NodeSpec::shape("Cube", b));
    let mut c = Shape::new(PrimKind::Cylinder);
    c.size = Vec3::new(4.0, 14.0, 0.0);
    s.add(root, NodeSpec::shape("Trou", c).mode(CombineMode::Carve).blend(0.8));
    h.reset(AppState::new(s, "Cube test"));
    h.st.frame_all();
    h.steps(4);
    h.st.bake.quality = forme_core::bake::Quality::Standard;
    h.st.bake.open = true;
    h.steps(30);
    h.shot("19-cube-bake-dialogue");
    h.st.start_bake();
    while h.st.bake.job.is_some() {
        std::thread::sleep(std::time::Duration::from_millis(50));
        h.step();
    }
    report_bake(h, "cube");
    h.steps(30);
    h.shot("19-cube-bake-resultat");
    if h.st.bake.result.is_none() {
        // not handled yet: the dialog says why, nothing more to show
        return;
    }
    h.st.bake.open = false;
    h.st.toast = None;
    h.cam(Vec3::new(3.0, 0.0, 0.0), 78.0, 0.72, 0.52);
    h.steps(20);
    h.shot("20-cube-bake-filaire");
    h.cam(Vec3::new(2.0, 8.0, 2.0), 34.0, 0.62, 0.8);
    h.steps(10);
    h.shot("21-cube-bake-zoom");
    // optional second pass (off by default): flat loops dissolved
    h.st.frame_all();
    h.st.bake.open = true;
    h.st.set_flat_pass(true);
    while h.st.bake.pass.is_some() {
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.step();
    }
    h.steps(30);
    h.shot("19-cube-passe-loops-plates");
    h.st.bake.open = false;
    h.st.toast = None;
    h.cam(Vec3::new(3.0, 0.0, 0.0), 78.0, 0.72, 0.52);
    h.steps(20);
    h.shot("20-cube-passe-filaire");
    h.st.set_flat_pass(false);
}

fn bake_grenade(h: &mut Harness) {
    let path = format!("{}/replay/grenade-remontee.forme", h.out_dir);
    let scene = std::fs::read_to_string(&path).ok().and_then(|s| forme_core::Scene::from_json(&s).ok()).unwrap_or_else(forme_core::demo::grenade);
    h.reset(AppState::new(scene, "Grenade (remontage)"));
    h.st.frame_all();
    h.steps(4);
    h.st.bake.quality = forme_core::bake::Quality::Max;
    h.st.bake.open = true;
    h.steps(40);
    h.shot("10-bake-dialogue");
    let t0 = Instant::now();
    h.st.start_bake();
    let mut mid = false;
    while h.st.bake.job.is_some() {
        std::thread::sleep(std::time::Duration::from_millis(150));
        h.step();
        let f = h.st.bake.job.as_ref().map(|j| j.progress.fraction()).unwrap_or(1.0);
        if !mid && f > 0.4 && h.st.bake.job.is_some() {
            mid = true;
            h.shot("11-bake-calcul");
        }
    }
    eprintln!("bake : {:.1?}", t0.elapsed());
    report_bake(h, "grenade");
    h.steps(40);
    h.shot("12-bake-resultat");
    if h.st.bake.result.is_none() {
        return;
    }
    h.st.bake.open = false;
    h.steps(20);
    h.shot("13-bake-filaire");
    // close-up: bosses, lobes and fillets
    h.cam(Vec3::new(2.0, 56.0, 26.0), 95.0, 0.32, 0.14);
    h.st.toast = None;
    h.steps(6);
    h.shot("14-bake-zoom");
    let zoom_bake = format!("{}/14-bake-zoom.png", h.out_dir);
    h.st.export_bake(true, true);
    eprintln!("{}", h.st.bake.last_export.clone().unwrap_or_default());
    // same view on the voxel export (dual contouring) for comparison
    let prog = forme_core::compile(&h.st.scene);
    let (lo, hi) = h.st.scene.bounds();
    let cell = (hi - lo).max_element() / forme_app::state::ExportRes::Normal.cells();
    let dc = forme_core::mesh::dual_contour(&prog, lo, hi, cell);
    let mut fake = forme_core::bake::Baked::default();
    fake.positions = dc.positions.iter().map(|p| p.as_dvec3()).collect();
    for t in dc.indices.chunks(3) {
        let n: Vec<glam::DVec3> = t.iter().map(|&i| dc.normals[i as usize].as_dvec3()).collect();
        fake.faces.push(forme_core::bake::BFace { verts: t.to_vec(), normals: n, mat: dc.materials[t[0] as usize], patch: 0 });
    }
    let real = h.st.bake.result.clone();
    h.st.bake.result = Some(std::sync::Arc::new(fake));
    h.st.bake.version += 1;
    h.st.bake.show = true;
    h.st.toast = None;
    h.steps(6);
    h.shot("tmp-dc-zoom");
    let dc_path = format!("{}/tmp-dc-zoom.png", h.out_dir);
    if let (Ok(a), Ok(bimg)) = (image::open(&dc_path), image::open(&zoom_bake)) {
        let (a, bimg) = (a.to_rgba8(), bimg.to_rgba8());
        let (w, hh) = (a.width(), a.height());
        // the free viewport area (between rail, panels, top bar and mode bar)
        let (x0, y0) = (w * 9 / 100, hh * 15 / 100);
        let (cw, ch) = (w * 62 / 100, hh * 72 / 100);
        let mut out = image::RgbaImage::new(cw * 2 + 12, ch);
        for p in out.pixels_mut() {
            *p = image::Rgba([23, 24, 27, 255]);
        }
        image::imageops::overlay(&mut out, &image::imageops::crop_imm(&a, x0, y0, cw, ch).to_image(), 0, 0);
        image::imageops::overlay(&mut out, &image::imageops::crop_imm(&bimg, x0, y0, cw, ch).to_image(), (cw + 12) as i64, 0);
        out.save(format!("{}/15-voxel-vs-bake.png", h.out_dir)).ok();
        std::fs::remove_file(&dc_path).ok();
    }
    eprintln!("voxel export (dual contouring) : {} triangles", dc.triangle_count());
    h.st.bake.result = real;
    h.st.bake.version += 1;
}
