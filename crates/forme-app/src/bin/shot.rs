//! Headless screenshots of the real app: drives it with synthetic input events
//! (Pencil, fingers, taps, drags) and renders frames with the same code as iPad.

use egui::{pos2, Event, Modifiers, PointerButton, Pos2, RawInput, Rect};
use forme_app::gfx::FrameRenderer;
use forme_app::settings::UiMode;
use forme_app::state::{AppState, PickMode, PointerKind, Tool};
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
        // the iPad scenarios show every primitive and mode
        self.st.settings.advanced = true;
    }

    /// PC window size and interface, for the desktop scenarios.
    fn pc(&mut self, st: AppState, mode: UiMode) {
        self.size = [1600.0, 900.0];
        self.reset(st);
        self.st.settings.advanced = false;
        self.st.settings.ui_mode = mode;
        self.st.enforce_basics();
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
    if want("09") {
        replay_grenade(&mut h);
    }
    if want("10") {
        bake_grenade(&mut h);
    }
    if want("19") {
        bake_cube(&mut h);
    }
    if want("user") {
        // like Romain's test on the iPad: a box, a profile blade, four kit screws, then Bake
        use forme_core::scene::{NodeSpec, Profile, ProfileMode, ProfilePoint, Scene, Shape};
        let mut s = Scene::new();
        let root = s.root;
        let mut b = Shape::new(PrimKind::Box);
        b.size = Vec3::new(14.0, 16.0, 12.0);
        s.add(root, NodeSpec::shape("Boîte 1", b));
        let mut prof = Shape::new(PrimKind::Profile);
        prof.size = Vec3::new(0.0, 0.0, 1.5);
        let pts = [(-2.0, -1.0), (22.0, 1.5), (22.0, 4.0), (-2.0, 3.0)];
        prof.profile = Some(Profile { points: pts.iter().map(|(x, y)| ProfilePoint { pos: glam::Vec2::new(*x, *y), smooth: false }).collect(), mode: ProfileMode::Extrude });
        s.add(root, NodeSpec::shape("Profil 1", prof).at(Vec3::new(12.0, 8.0, 0.0)));
        for at in [Vec3::new(-6.0, 4.0, 12.0), Vec3::new(3.0, 6.0, 12.0), Vec3::new(0.0, -2.0, 12.0), Vec3::new(-4.0, -6.0, 12.0)] {
            forme_app::kit::insert(&mut s, root, forme_app::kit::KitPiece::Screw, at, glam::Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), 1.0);
        }
        h.reset(AppState::new(s, "Sans titre"));
        h.st.frame_all();
        h.steps(4);
        h.shot("user-1-scene");
        h.st.bake.quality = forme_core::bake::Quality::Standard;
        h.st.start_bake();
        while h.st.bake.job.is_some() {
            std::thread::sleep(std::time::Duration::from_millis(50));
            h.step();
        }
        report_bake(&h, "bake");
        h.st.toast = None;
        h.steps(6);
        h.shot("user-2-bake");
        h.cam(Vec3::new(-2.0, 1.0, 12.0), 34.0, 0.25, 0.25);
        h.steps(6);
        h.shot("user-3-bake-vis");
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
    if want("pc-atelier") || want("pc-studio") || want("pc-creation") || want("pc-perf") || want("pc-snap") || want("pc-menu") || want("pc-marquee") || want("bake-diag") {
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


// ------------------------------------------------------------------------
// Full rebuild of the grenade from an empty scene, with the app's tools only.
// Camera moves stand for finger gestures; numeric fields stand for the panel.
// ------------------------------------------------------------------------

use forme_app::state::Handle;
use forme_core::{Axis, NodeKind, Repeat};

struct Replay {
    journal: Vec<String>,
    frame: usize,
    step: usize,
    /// Journal index where the current step's notes start.
    mark: usize,
    /// Timelapse frames: (file, seconds on screen).
    frames: Vec<(String, f32)>,
}

impl Harness {
    fn note(&mut self, r: &mut Replay, text: &str) {
        r.journal.push(format!("- {text}"));
        eprintln!("  · {text}");
        self.st.toast = Some(forme_app::state::Toast { text: text.to_string(), until: self.t + 100.0, warn: false });
        self.steps(12);
        r.frame += 1;
        let name = format!("replay/frames/f{:03}", r.frame);
        self.shot_at(&name, 1.0, 0.7);
        r.frames.push((name, 0.55));
    }

    fn step_frame(&mut self, r: &mut Replay, title: &str) {
        r.step += 1;
        r.journal.insert(r.mark, format!("\n**Étape {} · {title}**", r.step));
        r.mark = r.journal.len();
        eprintln!("Étape {} · {title}", r.step);
        self.st.toast = Some(forme_app::state::Toast { text: format!("Étape {} · {title}", r.step), until: self.t + 100.0, warn: false });
        self.steps(30); // let the toast fade in
        r.frame += 1;
        let name = format!("replay/frames/f{:03}", r.frame);
        self.shot_at(&name, 1.0, 0.7);
        r.frames.push((name, 1.8));
    }

    fn tool(&mut self, r: &mut Replay, t: Tool) {
        self.st.commit_placement();
        self.st.tool = t;
        self.steps(2);
        let what = match t {
            Tool::Place(k) => format!("outil {}", k.label()),
            Tool::Kit(k) => format!("kit {}", k.label()),
            Tool::Profile => "outil Profil".into(),
            other => format!("{other:?}"),
        };
        self.note(r, &format!("rail : {what}"));
    }

    /// Pencil: base on a surface (or the ground), stretched while reading the live
    /// measure until `size` (radius, or half width × half depth for boxes), then the height.
    fn place(&mut self, r: &mut Replay, base: Vec3, size: (f32, f32), height: Option<f32>) -> NodeId {
        let a = self.screen_of(base);
        self.press(a, true);
        for i in 1..=3 {
            self.move_to(a + egui::vec2(3.0 * i as f32, 0.0));
        }
        let p = self.st.placement.clone().expect("placement started");
        let is_box = p.kind == PrimKind::Box;
        let target = p.base + p.rot * Vec3::X * size.0 + if is_box { p.rot * Vec3::Z * size.1 } else { Vec3::ZERO };
        let b = self.screen_of(target);
        let a2 = a + egui::vec2(9.0, 0.0);
        for i in 1..=10 {
            self.move_to(a2 + (b - a2) * (i as f32 / 10.0));
        }
        self.press(b, false);
        self.step();
        let id = self.st.selection.expect("shape created");
        let msg = format!(
            "Pencil : base{} (normale {:.2}, {:.2}, {:.2})",
            p.snapped.map(|s| format!(" aimantée sur {s}")).unwrap_or_default(),
            p.normal.x,
            p.normal.y,
            p.normal.z
        );
        if let Some(hgt) = height {
            let p = self.st.placement.clone().expect("awaiting height");
            let px = self.st.view().pixel_size_at((p.base - self.st.camera.eye()).length());
            let dy = (hgt - p.height) / px;
            self.drag(b, b + egui::vec2(0.0, -dy), 12, true);
        }
        let n = self.st.scene.node(id).unwrap();
        let sh = n.shape().unwrap();
        self.note(r, &format!("{msg} → {} {} · taille {:.1} / {:.1} / {:.1} mm", n.name, n.combine.mode.label(), sh.size.x, sh.size.y, sh.size.z));
        id
    }

    fn select_node(&mut self, r: &mut Replay, id: NodeId) {
        self.st.tool = Tool::Select;
        self.st.select(Some(id));
        self.steps(2);
        let name = self.st.scene.node(id).unwrap().name.clone();
        self.note(r, &format!("arbre : sélection de « {name} »"));
    }

    /// Drags a gizmo handle of the selection by a world offset.
    fn gizmo(&mut self, r: &mut Replay, handle: Handle, delta: Vec3) {
        self.st.tool = Tool::Select;
        self.steps(2);
        let info = self.st.handles().into_iter().find(|h| h.handle == handle).expect("handle");
        let from = self.screen_of(info.world);
        let to = self.screen_of(info.world + delta);
        self.drag(from, to, 14, true);
        let id = self.st.selection.unwrap();
        let pos = self.st.scene.world_matrix(id).transform_point3(Vec3::ZERO);
        self.note(r, &format!("gizmo {handle:?} → position {:.1}, {:.1}, {:.1}", pos.x, pos.y, pos.z));
    }

    fn panel<F: FnOnce(&mut forme_core::Node)>(&mut self, r: &mut Replay, what: &str, f: F) {
        let id = self.st.selection.unwrap();
        self.st.edit(what, id, f);
        self.steps(2);
        self.note(r, &format!("panneau : {what}"));
    }

    fn rename(&mut self, r: &mut Replay, name: &str) {
        let n = name.to_string();
        self.panel(r, &format!("nom « {name} »"), move |node| node.name = n);
    }

    fn shape_edit<F: FnOnce(&mut forme_core::Shape)>(&mut self, r: &mut Replay, what: &str, f: F) {
        self.panel(r, what, move |n| {
            if let NodeKind::Shape(s) = &mut n.kind {
                f(s)
            }
        });
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
}

fn replay_grenade(h: &mut Harness) {
    use forme_app::camera::ViewPreset;
    let mut r = Replay { journal: vec!["# Remontage de la grenade, scène vide → objet complet".into()], frame: 0, step: 0, mark: 1, frames: Vec::new() };
    h.reset(AppState::new(forme_core::Scene::new(), "Grenade (remontage)"));
    h.st.pointer_kind = PointerKind::Pencil;
    h.cam(Vec3::new(0.0, 40.0, 0.0), 330.0, 0.42, 0.3);
    h.step_frame(&mut r, "Scène vide");

    // 1. body
    h.tool(&mut r, Tool::Place(PrimKind::Cylinder));
    let fut = h.place(&mut r, Vec3::ZERO, (27.0, 27.0), Some(68.0));
    h.gizmo(&mut r, Handle::MoveY, Vec3::new(0.0, 24.6, 0.0));
    h.rename(&mut r, "Fût");
    h.shape_edit(&mut r, "Rondeur 10 mm, Taper 0,04", |s| {
        s.round = 10.0;
        s.taper = 0.04;
    });
    h.panel(&mut r, "Matière Vert militaire", |n| n.material = 1);
    h.step_frame(&mut r, "Fût : cylindre au sol, monté au gizmo, arrondi");

    // 2. lobes
    h.cam(Vec3::new(0.0, 58.0, 0.0), 260.0, 0.42, 0.2);
    h.tool(&mut r, Tool::Place(PrimKind::Sphere));
    let lobe = h.place(&mut r, Vec3::new(0.0, 46.0, 26.8), (14.0, 14.0), None);
    h.select_node(&mut r, lobe);
    h.shape_edit(&mut r, "Rayons 14 / 15,5 / 12 mm", |s| s.size = Vec3::new(14.0, 15.5, 12.0));
    let lz = h.st.scene.world_matrix(lobe).transform_point3(Vec3::ZERO).z;
    h.gizmo(&mut r, Handle::MoveZ, Vec3::new(0.0, 0.0, 19.0 - lz));
    h.panel(&mut r, "Blend 5 mm, Vert, Répétition radiale ×4 (axe Y)", |n| {
        n.combine.blend = 5.0;
        n.material = 1;
        n.repeat = Repeat::Radial { count: 4, axis: Axis::Y };
        n.name = "Lobe".into();
    });
    h.st.group_selected();
    h.note(&mut r, "arbre : Grouper");
    h.panel(&mut r, "groupe « Lobes », Répétition linéaire ×2, pas 24 mm en Y", |n| {
        n.name = "Lobes".into();
        n.repeat = Repeat::Linear { count: 2, step: Vec3::new(0.0, 24.0, 0.0) };
    });
    h.step_frame(&mut r, "Lobes : sphère au Pencil, radiale ×4, groupe ×2");

    // 3. bosses + kit screws
    h.cam(Vec3::new(0.0, 52.0, 10.0), 200.0, 0.3, 0.12);
    h.tool(&mut r, Tool::Place(PrimKind::Cylinder));
    let boss = h.place(&mut r, Vec3::new(0.0, 46.0, 31.0), (6.5, 6.5), Some(6.0));
    h.select_node(&mut r, boss);
    h.panel(&mut r, "rotation 90° / 0 / 0 (axe exactement en Z)", |n| n.transform.rot = glam::Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
    let bp = h.st.scene.world_matrix(boss).transform_point3(Vec3::ZERO);
    h.gizmo(&mut r, Handle::PlaneXY, Vec3::new(-bp.x, 46.0 - bp.y, 0.0));
    let bz = h.st.scene.world_matrix(boss).transform_point3(Vec3::ZERO).z;
    h.gizmo(&mut r, Handle::MoveZ, Vec3::new(0.0, 0.0, 30.0 - bz));
    h.rename(&mut r, "Bossage");
    h.shape_edit(&mut r, "Rondeur 1,2 mm", |s| s.round = 1.2);
    h.panel(&mut r, "Blend 1,5 mm, Vert", |n| {
        n.combine.blend = 1.5;
        n.material = 1;
    });
    h.st.group_selected();
    let g1 = h.st.selection.unwrap();
    h.note(&mut r, "arbre : Grouper (bossage + vis)");
    h.select_node(&mut r, boss);
    h.tool(&mut r, Tool::Kit(forme_app::kit::KitPiece::Screw));
    let top = h.screen_of(Vec3::new(0.0, 46.0, 33.0));
    h.tap(top);
    h.note(&mut r, "kit : vis à fente posée au centre du bossage");
    h.select_node(&mut r, g1);
    h.panel(&mut r, "Répétition radiale ×4", |n| {
        n.name = "Bossage + vis".into();
        n.repeat = Repeat::Radial { count: 4, axis: Axis::Y };
    });
    h.st.group_selected();
    h.note(&mut r, "arbre : Grouper");
    h.panel(&mut r, "groupe « Bossages », Répétition linéaire ×2, pas 24 mm en Y", |n| {
        n.name = "Bossages".into();
        n.repeat = Repeat::Linear { count: 2, step: Vec3::new(0.0, 24.0, 0.0) };
    });
    h.step_frame(&mut r, "Bossages : cylindre + vis du kit, radiale ×4, groupe ×2");

    // 4. lower ring and base
    h.cam(Vec3::new(0.0, 30.0, 0.0), 300.0, 0.42, 0.25);
    h.tool(&mut r, Tool::Place(PrimKind::Cylinder));
    let ring = h.place(&mut r, Vec3::ZERO, (28.5, 28.5), Some(5.0));
    let ry = h.st.scene.world_matrix(ring).transform_point3(Vec3::ZERO).y;
    h.gizmo(&mut r, Handle::MoveY, Vec3::new(0.0, 27.0 - ry, 0.0));
    h.rename(&mut r, "Bague basse");
    h.shape_edit(&mut r, "Rondeur 1,2 mm", |s| s.round = 1.2);
    h.panel(&mut r, "Matière Orange", |n| n.material = 2);
    h.step_frame(&mut r, "Bague basse : anneau orange");

    h.tool(&mut r, Tool::Place(PrimKind::Cylinder));
    let base = h.place(&mut r, Vec3::ZERO, (23.0, 23.0), Some(14.0));
    let by = h.st.scene.world_matrix(base).transform_point3(Vec3::ZERO).y;
    h.gizmo(&mut r, Handle::MoveY, Vec3::new(0.0, 15.0 - by, 0.0));
    h.rename(&mut r, "Base");
    h.shape_edit(&mut r, "Rondeur 5 mm", |s| s.round = 5.0);
    h.panel(&mut r, "Blend 2 mm, Vert", |n| {
        n.combine.blend = 2.0;
        n.material = 1;
    });
    h.cam(Vec3::new(0.0, 14.0, 10.0), 200.0, 0.3, 0.05);
    h.tool(&mut r, Tool::Place(PrimKind::Sphere));
    let foot = h.place(&mut r, Vec3::new(0.0, 12.0, 23.5), (6.0, 6.0), None);
    h.select_node(&mut r, foot);
    h.shape_edit(&mut r, "Rayons 6 / 4 / 6 mm", |s| s.size = Vec3::new(6.0, 4.0, 6.0));
    let fp = h.st.scene.world_matrix(foot).transform_point3(Vec3::ZERO);
    h.gizmo(&mut r, Handle::PlaneYZ, Vec3::new(0.0, 8.5 - fp.y, 15.0 - fp.z));
    h.panel(&mut r, "Blend 2 mm, Vert, Répétition radiale ×6", |n| {
        n.name = "Pieds".into();
        n.combine.blend = 2.0;
        n.material = 1;
        n.repeat = Repeat::Radial { count: 6, axis: Axis::Y };
    });
    h.step_frame(&mut r, "Base et pieds (équerre YZ)");

    // 5. neck
    h.cam(Vec3::new(0.0, 100.0, 0.0), 260.0, 0.42, 0.35);
    h.tool(&mut r, Tool::Place(PrimKind::Cylinder));
    let core = h.place(&mut r, Vec3::new(0.0, 92.0, 0.0), (14.0, 14.0), Some(18.0));
    h.select_node(&mut r, core);
    h.rename(&mut r, "Noyau du col");
    h.panel(&mut r, "Matière Métal sombre", |n| n.material = 3);
    h.tool(&mut r, Tool::Place(PrimKind::Torus));
    let thread = h.place(&mut r, Vec3::new(0.0, 109.0, 0.0), (14.5, 14.5), Some(3.0));
    let ty = h.st.scene.world_matrix(thread).transform_point3(Vec3::ZERO).y;
    h.gizmo(&mut r, Handle::MoveY, Vec3::new(0.0, 93.5 - ty, 0.0));
    h.panel(&mut r, "« Filetage », Acier, Répétition linéaire ×5, pas 3,5 mm", |n| {
        n.name = "Filetage".into();
        n.material = 4;
        n.repeat = Repeat::Linear { count: 5, step: Vec3::new(0.0, 3.5, 0.0) };
    });
    h.tool(&mut r, Tool::Place(PrimKind::Cylinder));
    let collar = h.place(&mut r, Vec3::new(0.0, 109.0, 0.0), (19.0, 19.0), Some(8.0));
    h.select_node(&mut r, collar);
    h.rename(&mut r, "Collier");
    h.shape_edit(&mut r, "Rondeur 1,5 mm", |s| s.round = 1.5);
    h.panel(&mut r, "Matière Orange", |n| n.material = 2);
    h.step_frame(&mut r, "Col : noyau aimanté au centre, filetage ×5, collier");

    // 6. head block and hinge
    h.cam(Vec3::new(4.0, 125.0, 0.0), 230.0, 0.42, 0.3);
    h.tool(&mut r, Tool::Place(PrimKind::Box));
    let block = h.place(&mut r, Vec3::new(0.0, 116.0, 0.0), (16.0, 13.0), Some(16.0));
    h.select_node(&mut r, block);
    h.rename(&mut r, "Bloc");
    h.shape_edit(&mut r, "Rondeur 3,5 mm", |s| s.round = 3.5);
    h.panel(&mut r, "Matière Métal sombre", |n| n.material = 3);
    h.tool(&mut r, Tool::Place(PrimKind::Box));
    let hinge = h.place(&mut r, Vec3::new(16.0, 126.0, 0.0), (7.0, 8.5), Some(16.0));
    h.select_node(&mut r, hinge);
    let hx = h.st.scene.world_matrix(hinge).transform_point3(Vec3::ZERO).x;
    h.gizmo(&mut r, Handle::MoveX, Vec3::new(13.0 - hx, 0.0, 0.0));
    h.rename(&mut r, "Charnière");
    h.shape_edit(&mut r, "Rondeur 3 mm", |s| s.round = 3.0);
    h.panel(&mut r, "Blend 1 mm, Orange", |n| {
        n.combine.blend = 1.0;
        n.material = 2;
    });
    h.step_frame(&mut r, "Tête : bloc sur le collier, charnière sur sa face");

    // 7. hinge axis: carve then pin
    h.cam(Vec3::new(14.0, 126.0, 6.0), 150.0, 0.35, 0.15);
    h.tool(&mut r, Tool::Place(PrimKind::Cylinder));
    h.st.carve_held = true;
    h.note(&mut r, "pouce : Carve maintenu");
    let hole = h.place(&mut r, Vec3::new(15.0, 126.0, 8.5), (2.4, 2.4), Some(20.0));
    h.st.carve_held = false;
    h.select_node(&mut r, hole);
    h.panel(&mut r, "« Trou d'axe », blend 0,3 mm, Métal sombre", |n| {
        n.name = "Trou d'axe".into();
        n.combine.blend = 0.3;
        n.material = 3;
    });
    h.tool(&mut r, Tool::Place(PrimKind::Cylinder));
    let pin = h.place(&mut r, Vec3::new(15.0, 126.0, 7.0), (1.7, 1.7), Some(19.0));
    h.select_node(&mut r, pin);
    let pz = h.st.scene.world_matrix(pin).transform_point3(Vec3::ZERO).z;
    h.gizmo(&mut r, Handle::MoveZ, Vec3::new(0.0, 0.0, -pz));
    h.rename(&mut r, "Axe");
    h.shape_edit(&mut r, "Rondeur 0,4 mm", |s| s.round = 0.4);
    h.panel(&mut r, "Matière Acier", |n| n.material = 4);
    h.step_frame(&mut r, "Charnière : perçage en Carve, axe aimanté dans le trou");

    // 8. drum, groove, cap
    h.cam(Vec3::new(-10.0, 135.0, 0.0), 260.0, -0.5, 0.3);
    h.tool(&mut r, Tool::Place(PrimKind::Cylinder));
    let drum = h.place(&mut r, Vec3::new(-16.0, 124.0, 0.0), (12.0, 12.0), Some(38.0));
    h.select_node(&mut r, drum);
    let dp = h.st.scene.world_matrix(drum).transform_point3(Vec3::ZERO);
    h.gizmo(&mut r, Handle::PlaneXY, Vec3::new(-6.0 - dp.x, 142.0 - dp.y, 0.0));
    h.rename(&mut r, "Tambour");
    h.shape_edit(&mut r, "Rondeur 2 mm", |s| s.round = 2.0);
    h.panel(&mut r, "Matière Métal sombre", |n| n.material = 3);
    h.step_frame(&mut r, "Tambour : posé sur la face gauche, placé à l'équerre XY");

    h.st.select(None);
    h.tool(&mut r, Tool::Place(PrimKind::Box));
    h.st.set_mode(CombineMode::Groove);
    h.note(&mut r, "barre de modes : Groove");
    let groove = h.place(&mut r, Vec3::new(3.0, 154.0, 0.0), (0.5, 20.0), Some(40.0));
    h.st.set_mode(CombineMode::Add);
    h.select_node(&mut r, groove);
    h.shape_edit(&mut r, "Largeur 0,2 mm (plan de coupe)", |s| s.size.x = 0.1);
    h.panel(&mut r, "« Rainure », profondeur 1,2 mm, largeur 0,9 mm, Métal sombre", |n| {
        n.name = "Rainure".into();
        n.combine.depth = 1.2;
        n.combine.width = 0.9;
        n.material = 3;
    });
    h.cam(Vec3::new(-24.0, 142.0, 0.0), 170.0, -0.9, 0.15);
    h.tool(&mut r, Tool::Place(PrimKind::Cylinder));
    let cap = h.place(&mut r, Vec3::new(-25.0, 142.0, 0.0), (10.5, 10.5), Some(6.0));
    h.select_node(&mut r, cap);
    let cx = h.st.scene.world_matrix(cap).transform_point3(Vec3::ZERO).x;
    h.gizmo(&mut r, Handle::MoveX, Vec3::new(-25.5 - cx, 0.0, 0.0));
    h.rename(&mut r, "Capuchon");
    h.shape_edit(&mut r, "Rondeur 1,2 mm", |s| s.round = 1.2);
    h.panel(&mut r, "Matière Orange", |n| n.material = 2);
    h.tool(&mut r, Tool::Place(PrimKind::Cylinder));
    let plug = h.place(&mut r, Vec3::new(-28.5, 142.0, 0.0), (6.0, 6.0), Some(7.0));
    h.select_node(&mut r, plug);
    let px_ = h.st.scene.world_matrix(plug).transform_point3(Vec3::ZERO).x;
    h.gizmo(&mut r, Handle::MoveX, Vec3::new(-27.5 - px_, 0.0, 0.0));
    h.rename(&mut r, "Bouchon");
    h.shape_edit(&mut r, "Rondeur 0,8 mm", |s| s.round = 0.8);
    h.panel(&mut r, "Matière Métal sombre", |n| n.material = 3);
    h.step_frame(&mut r, "Rainure en Groove, capuchon et bouchon aimantés");

    // 9. lever drawn with the Pencil
    h.st.select(None);
    h.st.camera.preset(ViewPreset::Front);
    h.st.camera.target = Vec3::new(18.0, 84.0, 0.0);
    h.st.camera.dist = 300.0;
    h.st.view_changed();
    h.note(&mut r, "vue : Face (orthographique)");
    h.tool(&mut r, Tool::Profile);
    h.st.ensure_profile();
    let pts = forme_core::demo::lever_profile().points;
    for pt in &pts {
        let p = h.screen_of(Vec3::new(pt.pos.x, pt.pos.y, 0.0));
        if pt.smooth {
            h.drag(p + egui::vec2(-16.0, 10.0), p, 6, true);
        } else {
            h.tap(p);
        }
    }
    h.note(&mut r, &format!("Pencil : {} points (taps = angles, glissés = courbes)", pts.len()));
    h.step_frame(&mut r, "Levier : contour tracé au Pencil en vue Face");
    let first = h.screen_of(Vec3::new(pts[0].pos.x, pts[0].pos.y, 0.0));
    h.tap(first);
    let _lever = h.st.selection.unwrap();
    h.note(&mut r, "Pencil : tap sur le premier point → contour fermé, extrudé");
    h.shape_edit(&mut r, "Épaisseur 9 mm, rondeur 1,2 mm", |s| {
        s.size.z = 4.5;
        s.round = 1.2;
    });
    h.panel(&mut r, "« Lame », position Z −4 mm, Vert", |n| {
        n.name = "Lame".into();
        n.transform.pos.z = -4.0;
        n.material = 1;
    });

    // 10. pin and ring
    h.cam(Vec3::new(40.0, 110.0, 0.0), 200.0, 0.5, 0.15);
    h.tool(&mut r, Tool::Place(PrimKind::Capsule));
    let gp = h.place(&mut r, Vec3::new(48.6, 112.0, -4.0), (1.4, 1.4), Some(14.8));
    h.select_node(&mut r, gp);
    h.panel(&mut r, "« Goupille », Acier", |n| {
        n.name = "Goupille".into();
        n.material = 4;
    });
    h.st.select(None);
    h.st.camera.preset(ViewPreset::Front);
    h.st.camera.target = Vec3::new(40.0, 110.0, 0.0);
    h.st.camera.dist = 200.0;
    h.st.view_changed();
    h.tool(&mut r, Tool::Place(PrimKind::Torus));
    let ring2 = h.place(&mut r, Vec3::new(63.5, 112.0, 0.0), (6.5, 6.5), Some(2.6));
    h.select_node(&mut r, ring2);
    h.panel(&mut r, "« Anneau », position Z −4 mm, Acier", |n| {
        n.name = "Anneau".into();
        n.transform.pos.z = -4.0;
        n.material = 4;
    });
    h.st.camera.preset(ViewPreset::Perspective);
    h.st.frame_all();
    h.st.tool = Tool::Select;
    h.st.select(None);
    h.steps(2);
    h.step_frame(&mut r, "Goupille et anneau : grenade terminée");

    let shapes = h.st.scene.shape_count();
    r.journal.push(format!("\n{} formes dans l'arbre, {} nœuds.", shapes, h.st.scene.nodes.len() - 1));
    let rows: Vec<String> = h
        .st
        .scene
        .rows()
        .iter()
        .map(|(id, d)| format!("{}{}", "  ".repeat(*d), h.st.scene.node(*id).unwrap().name))
        .collect();
    r.journal.push(format!("\n```\n{}\n```", rows.join("\n")));
    std::fs::write(format!("{}/replay/journal.md", h.out_dir), r.journal.join("\n")).ok();
    // ffmpeg concat list for the timelapse (last frame repeated, as the demuxer requires).
    let mut list = String::new();
    for (f, d) in &r.frames {
        list += &format!("file '../{f}.png'\nduration {d}\n");
    }
    if let Some((f, _)) = r.frames.last() {
        list += &format!("file '../{f}.png'\n");
    }
    std::fs::write(format!("{}/replay/frames.txt", h.out_dir), list).ok();
    std::fs::write(format!("{}/replay/grenade-remontee.forme", h.out_dir), h.st.scene.to_json()).ok();
    h.st.toast = None;
    h.steps(2);
    h.shot("09-grenade-remontee");
    let _ = fut;
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
    if want("pc-snap") {
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
        for (name, scene, mode) in [
            ("romain", romain.clone(), UiMode::Atelier),
            ("romain-aretes", aretes.clone(), UiMode::Atelier),
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
            h.st.bake.open = true;
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
