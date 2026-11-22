#![allow(deprecated)]
//! forme-nurbs-shot: runs the real app headless (egui + wgpu, a software
//! GPU is fine) with the kernel process, plays scenarios with pointer
//! events and saves PNGs.
//!
//! cargo run --release -p nurbs-app --bin forme-nurbs-shot -- OUT_DIR [scenario prefix…]
//! (FORME_NURBS_KERNEL=path/to/forme-nurbs-kernel when it is not next to it)

use egui::{pos2, vec2, Event, Modifiers, PointerButton, Pos2, RawInput, Rect};
use glam::{DQuat, DVec3, Vec3};
use nurbs_app::draw::Drawer;
use nurbs_app::state::{AppState, PushKind, SelMode, Tool};
use nurbs_app::theme;
use nurbs_kernel::client::{kernel_exe, Kernel};
use nurbs_kernel::doc::PrimKind;
use nurbs_kernel::{Doc, EdgeRef, FaceTag, Op, Prim};
use std::time::{Duration, Instant};

const FMT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl Gpu {
    fn new() -> Gpu {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() })).expect("adapter");
        let i = adapter.get_info();
        eprintln!("GPU: {} ({:?})", i.name, i.backend);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("shot"), required_limits: adapter.limits(), ..Default::default() })).expect("device");
        Gpu { device, queue }
    }

    fn target(&self, w: u32, h: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shot"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FMT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    fn read_rgba(&self, tex: &wgpu::Texture) -> Vec<u8> {
        let (w, h) = (tex.width(), tex.height());
        let padded = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (padded * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        buf.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let data = buf.slice(..).get_mapped_range().expect("mapped");
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h {
            let s = (row * padded) as usize;
            out.extend_from_slice(&data[s..s + (w * 4) as usize]);
        }
        drop(data);
        buf.unmap();
        out
    }
}

struct Harness {
    gpu: Gpu,
    drawer: Drawer,
    ctx: egui::Context,
    st: AppState,
    t: f64,
    size: [f32; 2],
    ppp: f32,
    pending: Vec<Event>,
    out: String,
    mods: Modifiers,
    /// egui textures not drawn yet (applied with the next image).
    pending_tex: egui::TexturesDelta,
}

impl Harness {
    fn new(out: &str) -> Harness {
        let gpu = Gpu::new();
        let drawer = Drawer::new(&gpu.device, FMT);
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let st = AppState::new(Doc::default(), "Untitled", Kernel::start(None));
        Harness { gpu, drawer, ctx, st, t: 0.0, size: [1600.0, 900.0], ppp: 2.0, pending: vec![], out: out.to_string(), mods: Modifiers::NONE, pending_tex: Default::default() }
    }

    /// A new app state on this document (its own kernel process).
    fn reset(&mut self, doc: Doc, name: &str) {
        let exe = kernel_exe();
        if exe.is_none() {
            eprintln!("kernel not found: set FORME_NURBS_KERNEL");
        }
        self.st = AppState::new(doc, name, Kernel::start(exe));
        // the change of tab drawn at once (scenario `tabanim` turns its
        // animation on), so the shots after a tab key do not catch it
        self.st.settings.tab_anim = false;
        self.ctx = egui::Context::default();
        theme::install(&self.ctx);
        self.pending_tex = Default::default();
        self.drawer = Drawer::new(&self.gpu.device, FMT);
        self.steps(2);
        self.settle();
        self.st.frame_all();
        self.steps(2);
        // the camera's flight ended (a scenario's own camera set next is
        // not taken back by it)
        self.settle();
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
        // iPad: the touches, as main.rs hands them over
        if self.st.touch.on {
            nurbs_app::touch::tick(&mut self.st, self.t);
            self.st.touch.apply(&mut raw);
        }
        raw
    }

    /// A touch (iPad), in screen points; `pencil`: the Apple Pencil.
    fn touch(&mut self, id: u64, phase: nurbs_app::touch::Phase, p: Pos2, pencil: bool) {
        nurbs_app::touch::on_touch(&self.ctx, &mut self.st, id, phase, p, pencil, self.t);
        self.step();
    }

    /// Touches moved together from `a` to `b` (one per start point).
    fn touch_drag(&mut self, a: &[(u64, Pos2)], b: &[Pos2], n: usize, release: bool) {
        use nurbs_app::touch::Phase;
        for (id, p) in a {
            nurbs_app::touch::on_touch(&self.ctx, &mut self.st, *id, Phase::Start, *p, false, self.t);
        }
        self.step();
        for i in 1..=n {
            let t = i as f32 / n as f32;
            for (k, (id, p)) in a.iter().enumerate() {
                nurbs_app::touch::on_touch(&self.ctx, &mut self.st, *id, Phase::Move, *p + (b[k] - *p) * t, false, self.t);
            }
            self.step();
        }
        if release {
            for (k, (id, _)) in a.iter().enumerate() {
                nurbs_app::touch::on_touch(&self.ctx, &mut self.st, *id, Phase::End, b[k], false, self.t);
            }
            self.steps(2);
        }
    }

    fn step(&mut self) {
        self.t += 1.0 / 60.0;
        let raw = self.raw();
        self.st.tick(self.t);
        let st = &mut self.st;
        let out = self.ctx.run_pass(raw, |ctx| nurbs_app::run(ctx, st));
        // textures are applied with the next image
        self.pending_tex.append(out.textures_delta);
    }

    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Frames until the kernel has answered the latest document.
    fn settle(&mut self) {
        let end = Instant::now() + Duration::from_secs(20);
        while Instant::now() < end {
            self.step();
            if !self.st.building() && self.st.cam_anim.is_none() {
                self.steps(2);
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        eprintln!("kernel: no answer in 20 s");
    }

    fn press(&mut self, p: Pos2, down: bool) {
        self.pending.push(Event::PointerMoved(p));
        self.pending.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: down, modifiers: self.mods });
        self.step();
    }

    /// The middle button.
    fn mpress(&mut self, p: Pos2, down: bool) {
        self.pending.push(Event::PointerMoved(p));
        self.pending.push(Event::PointerButton { pos: p, button: PointerButton::Middle, pressed: down, modifiers: self.mods });
        self.step();
    }

    /// Text typed (into the focused field).
    fn type_text(&mut self, t: &str) {
        self.pending.push(Event::Text(t.to_string()));
        self.step();
    }

    /// The right button (the marking menu).
    fn rpress(&mut self, p: Pos2, down: bool) {
        self.pending.push(Event::PointerMoved(p));
        self.pending.push(Event::PointerButton { pos: p, button: PointerButton::Secondary, pressed: down, modifiers: self.mods });
        self.step();
    }

    fn move_to(&mut self, p: Pos2) {
        self.pending.push(Event::PointerMoved(p));
        self.step();
    }

    /// Shift, Ctrl… held from now on.
    fn hold(&mut self, m: Modifiers) {
        self.mods = m;
        self.pending.push(Event::ModifiersChanged(m));
        self.step();
    }

    /// A key pressed (true) or let go (false), held in between.
    fn key_hold(&mut self, k: egui::Key, pressed: bool) {
        self.pending.push(Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers: self.mods });
        self.step();
    }

    fn key(&mut self, k: egui::Key) {
        for pressed in [true, false] {
            self.pending.push(Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers: self.mods });
            self.step();
        }
    }

    fn click(&mut self, p: Pos2) {
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

    /// One frame as the app makes it, drawn but not read back: the time of
    /// the tick (the kernel's answers taken), of the interface and of the
    /// drawing (ms). `perf` uses it.
    fn frame_timed(&mut self) -> [f64; 3] {
        self.t += 1.0 / 60.0;
        let raw = self.raw();
        let t0 = Instant::now();
        self.st.tick(self.t);
        let t1 = Instant::now();
        let st = &mut self.st;
        let mut out = self.ctx.run_pass(raw, |ctx| nurbs_app::run(ctx, st));
        let t2 = Instant::now();
        let paint = self.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        let mut tex = std::mem::take(&mut self.pending_tex);
        tex.append(out.textures_delta);
        let (w, h) = ((self.size[0] * self.ppp) as u32, (self.size[1] * self.ppp) as u32);
        let target = self.gpu.target(w, h);
        let view = target.create_view(&Default::default());
        self.drawer.render(&self.gpu.device, &self.gpu.queue, &view, [w, h], self.ppp, &self.st, &paint, &mut tex);
        let t3 = Instant::now();
        [(t1 - t0).as_secs_f64() * 1e3, (t2 - t1).as_secs_f64() * 1e3, (t3 - t2).as_secs_f64() * 1e3]
    }

    fn screen_of(&self, w: Vec3) -> Pos2 {
        let p = self.st.view().project(w).expect("in front");
        pos2(p[0], p[1])
    }

    fn shot(&mut self, name: &str) {
        self.t += 1.0 / 60.0;
        let raw = self.raw();
        self.st.tick(self.t);
        let st = &mut self.st;
        let mut out = self.ctx.run_pass(raw, |ctx| nurbs_app::run(ctx, st));
        let paint = self.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        let mut tex = std::mem::take(&mut self.pending_tex);
        tex.append(out.textures_delta);
        let (w, h) = ((self.size[0] * self.ppp) as u32, (self.size[1] * self.ppp) as u32);
        let target = self.gpu.target(w, h);
        let view = target.create_view(&Default::default());
        self.drawer.render(&self.gpu.device, &self.gpu.queue, &view, [w, h], self.ppp, &self.st, &paint, &mut tex);
        let px = self.gpu.read_rgba(&target);
        let path = format!("{}/{}.png", self.out, name);
        if let Some(dir) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(dir).ok();
        }
        image::save_buffer(&path, &px, w, h, image::ExtendedColorType::Rgba8).expect("png");
        let m = self.st.model.as_ref();
        eprintln!("{path}  ({} faces, {} edges, errors {:?})", m.map(|m| m.faces.len()).unwrap_or(0), m.map(|m| m.edges.len()).unwrap_or(0), m.map(|m| m.errors.clone()).unwrap_or_default());
    }
}

/// The test part: a 40 mm block, a hole through it, a boss on top, a slot.
fn test_part() -> Doc {
    let mut d = Doc::default();
    let block = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
    d.feature_mut(block).unwrap().name = "Block".into();
    let hole = d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-10.0, -5.0, 0.0), DQuat::IDENTITY);
    d.feature_mut(hole).unwrap().name = "Hole".into();
    let boss = d.add(Op::Add, Prim::Cylinder { radius: 7.0, height: 10.0 }, DVec3::new(10.0, 20.0, 0.0), DQuat::IDENTITY);
    d.feature_mut(boss).unwrap().name = "Boss".into();
    let slot = d.add(Op::Carve, Prim::Box { x: 8.0, y: 8.0, z: 40.0 }, DVec3::new(0.0, 14.0, 0.0), DQuat::IDENTITY);
    d.feature_mut(slot).unwrap().name = "Slot".into();
    d
}

/// `--serve [mesh.obj]`: the app headless, its link with Claude open
/// (`remote.rs`, port FORME_NURBS_PORT or 47830), until killed: the MCP
/// server `forme-nurbs-mcp` can drive it (tests without a screen).
fn serve(mesh: Option<&str>) {
    let mut h = Harness::new("/tmp");
    h.reset(Doc::default(), "Untitled");
    if let Some(m) = mesh {
        h.st.import_reference(m);
    }
    match nurbs_app::remote::start(&mut h.st, std::sync::Arc::new(|| {})) {
        Ok(p) => eprintln!("serving on 127.0.0.1:{p}"),
        Err(e) => {
            eprintln!("no link: {e}");
            return;
        }
    }
    loop {
        h.step();
        nurbs_app::remote::poll(&mut h.st);
        if !h.st.link.captures.is_empty() {
            let (gpu, drawer) = (&h.gpu, &mut h.drawer);
            nurbs_app::remote::render_captures(&mut h.st, |st, w, hh| {
                let tex = gpu.target(w, hh);
                let view = tex.create_view(&Default::default());
                drawer.render(&gpu.device, &gpu.queue, &view, [w, hh], 1.0, st, &[], &mut egui::TexturesDelta::default());
                gpu.read_rgba(&tex)
            });
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a == "--serve").unwrap_or(false) {
        serve(args.get(1).map(|s| s.as_str()));
        return;
    }
    let out = args.first().cloned().unwrap_or_else(|| "shots".into());
    let only: Vec<String> = args.iter().skip(1).cloned().collect();
    let want = |name: &str| only.is_empty() || only.iter().any(|o| name.starts_with(o.as_str()));
    let mut h = Harness::new(&out);

    if want("multi") {
        multi(&mut h);
    }
    if want("sdf") {
        sdf_tab(&mut h);
    }
    if want("sdfx") {
        sdf_more(&mut h);
    }
    if want("commonm") {
        common_meshes(&mut h);
    }
    if want("common-") || only.iter().any(|o| o == "common") {
        common_base(&mut h);
    }
    if want("pbevel") {
        poly_bevel(&mut h);
    }
    if want("pcut") {
        poly_cut(&mut h);
    }
    if want("buildup") {
        buildup(&mut h);
    }
    if want("pangle") {
        poly_angle(&mut h);
    }
    if want("tabanim") {
        tab_anim(&mut h);
    }
    if want("lsurf") {
        linked(&mut h);
    }
    if want("lsurfx") {
        linked_complex(&mut h);
    }
    if want("lobj") {
        linked_objects(&mut h);
    }

    if want("empty") {
        h.reset(Doc::default(), "Untitled");
        h.shot("zero-empty");
    }

    if want("part") {
        h.reset(test_part(), "Test part");
        h.shot("zero-part");
        // pre-selection, then selection by a click on the block's front face
        let front = h.screen_of(Vec3::new(14.0, 6.0, 15.0));
        h.move_to(front);
        h.steps(2);
        h.shot("zero-part-hover");
        h.click(front);
        h.settle();
        h.shot("zero-part-select");
        // the hole's far wall, seen through its opening: selects the hole
        // (a cutter, shown by its outline)
        h.st.selection.clear();
        let wall = h.screen_of(Vec3::new(-13.5, 17.0, -3.5));
        h.click(wall);
        h.steps(2);
        h.shot("zero-part-select-hole");
    }

    if want("move") {
        h.reset(test_part(), "Test part");
        // select the boss, drag its X arrow: the kernel rebuilds live
        let boss = h.screen_of(Vec3::new(10.0, 30.0, 0.0));
        h.click(boss);
        h.steps(2);
        let pivot = h.st.pivot().unwrap();
        let v = h.st.view();
        let k = v.pixel_size_at((pivot.as_vec3() - v.eye).length()) * 78.0;
        let tip = h.screen_of(pivot.as_vec3() + Vec3::X * k * 0.7);
        let to = h.screen_of(pivot.as_vec3() + Vec3::new(-14.0, 0.0, 0.0) + Vec3::X * k * 0.7);
        h.move_to(tip);
        h.steps(2);
        h.shot("zero-move-1-hot");
        h.drag(tip, to, 12, false);
        h.settle();
        h.shot("zero-move-2-live");
        h.press(to, false);
        h.settle();
        h.shot("zero-move-3-done");
        eprintln!("boss now at {:?}", h.st.doc.features[2].origin);
    }

    if want("create") {
        h.reset(test_part(), "Test part");
        // a box on the block's top face, Add: base, then height
        h.st.tool = Tool::Create(PrimKind::Box);
        let a = h.screen_of(Vec3::new(-16.0, 20.0, -12.0));
        let b = h.screen_of(Vec3::new(-6.0, 20.0, -4.0));
        h.move_to(a);
        h.steps(2);
        h.shot("zero-create-1-armed");
        h.drag(a, b, 10, true);
        h.settle();
        h.shot("zero-create-2-base");
        let up = h.screen_of(Vec3::new(-11.0, 32.0, -8.0));
        h.move_to(up);
        h.settle();
        h.shot("zero-create-3-height");
        h.click(up);
        h.settle();
        h.shot("zero-create-4-done");
        // a cylinder in Carve, into the block's front face
        h.st.mode = Op::Carve;
        h.st.tool = Tool::Create(PrimKind::Cylinder);
        let c = h.screen_of(Vec3::new(12.0, 8.0, 15.0));
        let r = h.screen_of(Vec3::new(16.0, 8.0, 15.0));
        h.drag(c, r, 8, true);
        let down = h.screen_of(Vec3::new(12.0, 8.0, 5.0));
        h.move_to(down);
        h.settle();
        h.shot("zero-create-5-carve");
        h.click(down);
        h.settle();
        h.st.tool = Tool::Select;
        h.st.selection.clear();
        h.steps(2);
        h.shot("zero-create-6-done");
        for f in &h.st.doc.features {
            eprintln!("  {} {:?} {:?} at {:?}", f.name, f.op, f.prim, f.origin);
        }
    }

    if want("quick") {
        // quick creation: the tool's key twice, then stacked, then drilled
        h.reset(Doc::default(), "Quick");
        h.key(egui::Key::B);
        h.key(egui::Key::B);
        h.key(egui::Key::C);
        h.key(egui::Key::C);
        h.settle();
        h.st.frame_all();
        h.steps(2);
        h.shot("zero-quick-1-stacked");
        // Carve for new shapes (X would also switch the selected cylinder)
        h.st.mode = Op::Carve;
        h.hold(Modifiers::SHIFT);
        h.key(egui::Key::C);
        h.hold(Modifiers::NONE);
        h.settle();
        h.shot("zero-quick-2-drilled");
        for f in &h.st.doc.features {
            eprintln!("  {} {:?} {:?} at {:?}", f.name, f.op, f.prim, f.origin);
        }
        // materials: orange cube, steel cylinder, the section open
        let ids: Vec<u32> = h.st.doc.features.iter().map(|f| f.id).collect();
        h.st.selection = vec![ids[0]];
        h.st.set_material(2);
        h.st.selection = vec![ids[1]];
        h.st.set_material(4);
        h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("material-open"), true));
        h.settle();
        h.shot("zero-quick-3-materials");
    }

    if want("sketch") {
        // the Sketch tool (L): an outline on the ground, Add, then one on the
        // block's front face, Carve
        h.reset(test_part(), "Test part");
        h.st.camera.frame(Vec3::new(-60.0, 0.0, -20.0), Vec3::new(25.0, 25.0, 35.0));
        h.steps(2);
        h.key(egui::Key::L);
        let view = h.st.view();
        let at = move |w: Vec3| {
            let p = view.project(w).expect("in front");
            pos2(p[0], p[1])
        };
        let g = |x: f32, z: f32| at(Vec3::new(x, 0.0, z));
        let (p0, p1, p2, p3, p4) = (g(-55.0, 30.0), g(-30.0, 30.0), g(-30.0, 20.0), g(-42.0, 12.0), g(-55.0, 12.0));
        h.click(p0);
        h.click(p1);
        h.click(p2);
        // a curve point: pressed, then dragged out
        h.drag(g(-40.0, 18.0), p3, 8, true);
        h.click(p4);
        h.move_to(g(-50.0, 22.0));
        h.steps(2);
        h.shot("zero-sketch-1-outline");
        // closing on the first point: on to the height
        h.move_to(p0);
        h.steps(2);
        h.shot("zero-sketch-2-close");
        h.click(p0);
        let up = at(Vec3::new(-42.0, 12.0, 21.0));
        h.move_to(up);
        h.settle();
        h.shot("zero-sketch-3-height");
        h.click(up);
        h.settle();
        // a D-shaped pocket in the block's front face, Carve
        h.st.mode = Op::Carve;
        let f = |x: f32, y: f32| at(Vec3::new(x, y, 15.0));
        let (a0, a1, a2, a3) = (f(-17.0, 3.0), f(-7.0, 3.0), f(-7.0, 13.0), f(-17.0, 13.0));
        h.click(a0);
        h.click(a1);
        h.drag(f(-6.0, 8.0), f(-3.0, 8.0), 6, true);
        h.click(a2);
        h.click(a3);
        h.click(a0);
        let inside = at(Vec3::new(-12.0, 8.0, 7.0));
        h.move_to(inside);
        h.settle();
        h.shot("zero-sketch-4-pocket");
        h.click(inside);
        h.settle();
        h.st.tool = Tool::Select;
        h.st.selection.clear();
        h.steps(2);
        h.shot("zero-sketch-5-done");
        for f in &h.st.doc.features {
            eprintln!("  {} {:?} {:?} at {:?} {:?}", f.name, f.op, f.prim, f.origin, f.profile.as_ref().map(|p| p.points.len()));
        }
    }

    if want("edges") {
        // the test part, with a smaller boss clear of the slot
        let mut d = test_part();
        if let Some(b) = d.features.iter_mut().find(|f| f.name == "Boss") {
            b.prim = Prim::Cylinder { radius: 4.0, height: 10.0 };
            b.origin = DVec3::new(12.0, 20.0, -4.0);
        }
        h.reset(d, "Test part");
        // Edges mode (2): the block's top front edge, right of the slot
        h.key(egui::Key::Num2);
        assert_eq!(h.st.sel_mode, SelMode::Edge);
        let e1 = h.screen_of(Vec3::new(12.0, 20.0, 15.0));
        h.move_to(e1);
        h.steps(2);
        h.shot("zero-edges-1-hover");
        h.click(e1);
        // Shift + click: the block's front right vertical edge
        h.hold(Modifiers::SHIFT);
        let e2 = h.screen_of(Vec3::new(20.0, 8.0, 15.0));
        h.click(e2);
        h.hold(Modifiers::NONE);
        h.move_to(h.screen_of(Vec3::new(0.0, 0.0, 40.0)));
        h.steps(2);
        eprintln!("picked {} edges", h.st.edge_sel.len());
        h.shot("zero-edges-2-picked");
        // a 2 mm fillet, as the inspector's "2" does
        h.st.set_round(2.0, false, true);
        h.st.end_live();
        h.settle();
        h.shot("zero-edges-3-fillet");
        // a click on the fillet's face selects it; Shift + click on the
        // boss's foot adds that edge to it
        let face = h.screen_of(Vec3::new(14.0, 19.4, 14.4));
        h.click(face);
        eprintln!("selected {:?}", h.st.selected_round().map(|f| f.name.clone()));
        h.hold(Modifiers::SHIFT);
        let foot = h.screen_of(Vec3::new(12.0, 20.0, 0.0));
        h.click(foot);
        h.hold(Modifiers::NONE);
        h.settle();
        h.shot("zero-edges-4-boss-foot");
        // as a chamfer
        h.st.set_round(1.5, true, true);
        h.st.end_live();
        h.settle();
        h.shot("zero-edges-5-chamfer");
        // the boss moved (Object mode): its foot is found again
        h.st.set_round(2.0, false, true);
        h.st.end_live();
        h.key(egui::Key::Q);
        let boss = h.st.doc.features.iter().find(|f| f.name == "Boss").map(|f| f.id).unwrap();
        h.st.selection = vec![boss];
        h.st.edit("Move", |d| d.feature_mut(boss).unwrap().origin.z = 3.0);
        h.st.selection.clear();
        h.settle();
        h.shot("zero-edges-6-moved");
        // too big: it stops at the largest size OpenCascade can make
        let r = h.st.doc.features.iter().find(|f| f.round.is_some()).map(|f| f.id).unwrap();
        h.st.selection = vec![r];
        h.st.set_round(14.0, false, true);
        h.st.end_live();
        h.settle();
        h.settle();
        h.shot("zero-edges-7-too-big");
        for f in &h.st.doc.features {
            eprintln!("  {} {:?}", f.name, f.round);
        }
    }

    if want("faces") {
        // the component modes (Face, 3) and the Inset / Extrude tools on the
        // test part
        h.reset(test_part(), "Test part");
        h.key(egui::Key::Num3);
        assert_eq!(h.st.sel_mode, SelMode::Face);
        // the block's front face (it has the slot's notch at the top)
        let front = h.screen_of(Vec3::new(-12.0, 7.0, 15.0));
        h.move_to(front);
        h.steps(2);
        h.shot("zero-faces-1-hover");
        h.click(front);
        h.move_to(front + egui::vec2(0.0, 2.0));
        h.steps(2);
        eprintln!("picked {} faces", h.st.face_sel.len());
        h.shot("zero-faces-2-picked");
        // I: a 2 mm inset at once, its dot and its arrow on screen
        h.key(egui::Key::I);
        h.settle();
        let hs = h.st.push_handles().expect("inset handles");
        let (_, w, knob) = hs.inset.expect("inset dot");
        h.move_to(h.screen_of(knob.as_vec3()));
        h.steps(2);
        h.shot("zero-faces-3-inset");
        // the dot dragged 1 mm further in: 3 mm
        h.drag(h.screen_of(knob.as_vec3()), h.screen_of((knob + w).as_vec3()), 8, true);
        h.settle();
        let hs = h.st.push_handles().expect("inset handles");
        // the arrow dragged 4 mm in: a pocket
        let grab = hs.base + (hs.tip - hs.base) * 0.6;
        let to = grab - hs.n * 4.0;
        h.drag(h.screen_of(grab.as_vec3()), h.screen_of(to.as_vec3()), 8, false);
        h.settle();
        h.shot("zero-faces-4-pocket");
        h.press(h.screen_of(to.as_vec3()), false);
        h.steps(2);
        h.settle();
        h.move_to(h.screen_of(Vec3::new(0.0, -10.0, 40.0)));
        h.steps(2);
        let made = h.st.doc.features.iter().filter_map(|f| f.push.as_ref().map(|p| (f.name.clone(), p.inset, p.depth))).collect::<Vec<_>>();
        eprintln!("made {made:?}");
        h.shot("zero-faces-5-done");
        // E, then a press on the block's left top face (it has the hole) and
        // a drag up: 6 mm
        h.key(egui::Key::E);
        let top = DVec3::new(-16.0, 20.0, 10.0);
        let a = h.screen_of(top.as_vec3());
        let b = h.screen_of((top + DVec3::Y * 6.0).as_vec3());
        h.move_to(a);
        h.steps(2);
        h.shot("zero-faces-6-extrude-armed");
        h.drag(a, b, 8, false);
        h.settle();
        h.shot("zero-faces-7-extrude-drag");
        h.press(b, false);
        h.steps(2);
        h.settle();
        h.move_to(h.screen_of(Vec3::new(0.0, -10.0, 40.0)));
        h.steps(2);
        h.shot("zero-faces-8-extruded");
        // the inset raised instead of sunk, from the panel
        let inset = h.st.doc.features.iter().find(|f| f.name == "Inset").map(|f| f.id).unwrap();
        h.st.set_sel_mode(SelMode::Face);
        h.st.selection = vec![inset];
        h.st.set_push(3.0, 2.0, true);
        h.st.end_live();
        h.settle();
        h.shot("zero-faces-9-raised");
        // Vertex mode (1): the corners, one under the pointer
        h.key(egui::Key::Num1);
        h.move_to(h.screen_of(Vec3::new(20.0, 20.0, 15.0)));
        h.steps(2);
        h.shot("zero-faces-10-vertex");
        h.key(egui::Key::Num4);
        assert_eq!(h.st.sel_mode, SelMode::Object);
    }

    if want("blend") {
        // three fillets (3, 3 and 6 mm) made one after the other on the
        // three edges of a block's corner: open (each on its own), then
        // blended
        let tag = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
        let mk = |blend: bool| {
            let mut d = Doc::default();
            let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            d.feature_mut(b).unwrap().name = "Block".into();
            let edges = [(2u16, 4u16, DVec3::new(0.0, 20.0, 15.0), 3.0), (0, 4, DVec3::new(20.0, 10.0, 15.0), 3.0), (2, 0, DVec3::new(20.0, 20.0, 0.0), 6.0)];
            for (a, c, at, size) in edges {
                let r = d.add_round(size, false, vec![EdgeRef { faces: [tag(b, a), tag(b, c)], at }]);
                d.feature_mut(r).unwrap().round.as_mut().unwrap().blend = blend;
            }
            d
        };
        for (blend, name) in [(false, "zero-blend-1-open"), (true, "zero-blend-2-blended")] {
            let d = mk(blend);
            let last = d.features.last().unwrap().id;
            h.reset(d, "Corner");
            h.st.camera.frame(Vec3::new(5.0, 5.0, 0.0), Vec3::new(25.0, 22.0, 18.0));
            h.st.selection = vec![last];
            h.steps(2);
            h.settle();
            h.shot(name);
        }
    }

    if want("remesh") {
        // the « Export converter » (field-aligned remesh), its own view
        // apart from the face-by-face Quads view
        let tag = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
        let mut d = test_part();
        let block = d.features.iter().find(|f| f.name == "Block").map(|f| f.id).unwrap();
        d.add_round(3.0, false, vec![EdgeRef { faces: [tag(block, 0), tag(block, 4)], at: DVec3::new(20.0, 10.0, 15.0) }]);
        h.reset(d, "Remesh part");
        h.st.set_remesh_view(true);
        h.settle();
        let t0 = std::time::Instant::now();
        while h.st.remeshing() && t0.elapsed().as_secs() < 120 {
            std::thread::sleep(Duration::from_millis(50));
            h.steps(1);
        }
        h.steps(2);
        match h.st.remesh.as_ref().map(|r| (&r.result, r.ms)) {
            Some((Ok((q, rep)), ms)) => eprintln!("  remesh {} quads, quad {:.2} mm, {:.0} ms, closed {}", q.quads.len(), rep.quad_size, ms, nurbs_kernel::remesh::is_closed(q)),
            Some((Err(e), _)) => eprintln!("  remesh failed: {e}"),
            None => eprintln!("  no remesh"),
        }
        h.shot("zero-remesh-1");
        std::env::set_var("FORME_DOCS", &out);
        h.st.export_obj();
        h.steps(1);
        // the Quads view turns it off: the two stay apart
        h.st.set_quads_view(true);
        h.settle();
        eprintln!("  quads view: remesh_on {} quads {}", h.st.remesh_on, h.st.quad_mesh().map(|q| q.quads.len()).unwrap_or(0));
        h.shot("zero-remesh-2-faces-view");
    }

    if want("quads") {
        let tag = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
        // a 20 mm cube, its top front edge filleted at 4 mm
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Box { x: 20.0, y: 20.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(c).unwrap().name = "Cube".into();
        d.add_round(4.0, false, vec![EdgeRef { faces: [tag(c, 2), tag(c, 4)], at: DVec3::new(0.0, 20.0, 10.0) }]);
        h.reset(d, "Cube one fillet");
        h.shot("zero-quads-0-off");
        h.st.quads = true;
        h.steps(2);
        h.settle();
        h.shot("zero-quads-1-one-fillet");
        std::env::set_var("FORME_DOCS", &out);
        h.st.export_obj();
        h.steps(1);
        h.shot("zero-quads-2-exported");

        // the four upright edges filleted
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Box { x: 20.0, y: 20.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(c).unwrap().name = "Cube".into();
        let ups = [(0u16, 4u16, 10.0, 10.0), (0, 5, 10.0, -10.0), (1, 4, -10.0, 10.0), (1, 5, -10.0, -10.0)];
        d.add_round(4.0, false, ups.iter().map(|(a, b, x, z)| EdgeRef { faces: [tag(c, *a), tag(c, *b)], at: DVec3::new(*x, 10.0, *z) }).collect());
        h.reset(d, "Cube four fillets");
        h.st.quads = true;
        h.steps(2);
        h.settle();
        h.shot("zero-quads-3-four-fillets");
        h.st.export_obj();

        // the test part with a fillet: every face in quads (the faces with
        // holes too)
        let mut d = test_part();
        let block = d.features.iter().find(|f| f.name == "Block").map(|f| f.id).unwrap();
        d.add_round(3.0, false, vec![EdgeRef { faces: [tag(block, 0), tag(block, 4)], at: DVec3::new(20.0, 10.0, 15.0) }]);
        h.reset(d, "Test part");
        h.st.quads = true;
        h.steps(2);
        h.settle();
        h.shot("zero-quads-4-part");
        h.st.export_obj();
    }

    if want("quads-corner") {
        // fillets meeting in corners: three edges of a 20 mm cube's corner
        // in one fillet (a sphere's eighth closes it), then every edge
        let tag = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
        let cases: [(&str, f64, bool); 5] = [("corner-r2", 2.0, false), ("corner-r4", 4.0, false), ("corner-r8", 8.0, false), ("all-r4", 4.0, true), ("all-r8", 8.0, true)];
        for (name, r, all) in cases {
            let mut d = Doc::default();
            let c = d.add(Op::Add, Prim::Box { x: 20.0, y: 20.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
            d.feature_mut(c).unwrap().name = "Cube".into();
            let mut edges = vec![];
            for (a, b, at) in [(2u16, 4u16, DVec3::new(0.0, 20.0, 10.0)), (0, 4, DVec3::new(10.0, 10.0, 10.0)), (0, 2, DVec3::new(10.0, 20.0, 0.0))] {
                edges.push(EdgeRef { faces: [tag(c, a), tag(c, b)], at });
            }
            if all {
                edges.clear();
                for (x, sx) in [(0u16, 10.0), (1, -10.0)] {
                    for (z, sz) in [(4u16, 10.0), (5, -10.0)] {
                        edges.push(EdgeRef { faces: [tag(c, x), tag(c, z)], at: DVec3::new(sx, 10.0, sz) });
                    }
                    for (y, sy) in [(2u16, 20.0), (3, 0.0)] {
                        edges.push(EdgeRef { faces: [tag(c, x), tag(c, y)], at: DVec3::new(sx, sy, 0.0) });
                    }
                }
                for (z, sz) in [(4u16, 10.0), (5, -10.0)] {
                    for (y, sy) in [(2u16, 20.0), (3, 0.0)] {
                        edges.push(EdgeRef { faces: [tag(c, z), tag(c, y)], at: DVec3::new(0.0, sy, sz) });
                    }
                }
            }
            d.add_round(r, false, edges);
            h.reset(d, name);
            h.st.quads = true;
            h.st.camera.frame(Vec3::new(-6.0, 4.0, -6.0), Vec3::new(10.0, 20.0, 10.0));
            h.steps(2);
            h.settle();
            h.shot(&format!("zero-quads-{name}"));
            if let Some(q) = h.st.quad_mesh() {
                eprintln!("  {name}: {} quads, left out {:?}", q.quads.len(), q.skipped);
            }
        }
        // the test part, its front right corner rounded large (6 mm, three
        // edges in one fillet)
        let mut d = test_part();
        let block = d.features.iter().find(|f| f.name == "Block").map(|f| f.id).unwrap();
        let edges = [(2u16, 4u16, DVec3::new(12.0, 20.0, 15.0)), (0, 4, DVec3::new(20.0, 10.0, 15.0)), (0, 2, DVec3::new(20.0, 20.0, 0.0))];
        d.add_round(6.0, false, edges.iter().map(|(a, b, at)| EdgeRef { faces: [tag(block, *a), tag(block, *b)], at: *at }).collect());
        h.reset(d, "Test part, large fillet");
        h.st.quads = true;
        h.steps(2);
        h.settle();
        h.shot("zero-quads-part-large-fillet");
        if let Some(q) = h.st.quad_mesh() {
            eprintln!("  part-large-fillet: {} quads, left out {:?}", q.quads.len(), q.skipped);
        }
        std::env::set_var("FORME_DOCS", &out);
        h.st.export_obj();

        // a hole 1 mm from the edge of a plate
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 10.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(14.0, -5.0, 0.0), DQuat::IDENTITY);
        h.reset(d, "Hole near an edge");
        h.st.quads = true;
        h.st.camera.frame(Vec3::new(-20.0, 0.0, -15.0), Vec3::new(20.0, 10.0, 15.0));
        h.steps(2);
        h.settle();
        h.shot("zero-quads-hole-edge");
    }

    if want("comp") {
        // component moves (Romain 18 nov.: "Manipulation de component"): a
        // face, an edge and a vertex of a block moved with the gizmo; then a
        // corner of a thin plate rounded (vertex fillet)
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Block");
        h.st.camera.frame(Vec3::new(-25.0, 0.0, -20.0), Vec3::new(30.0, 30.0, 20.0));
        h.steps(2);
        // drags the gizmo's arrow `axis` by `mm`
        let pull = |h: &mut Harness, axis: usize, mm: f32| {
            let c = h.st.pivot().expect("a gizmo").as_vec3();
            let s0 = h.screen_of(c);
            let dir = (h.screen_of(c + Vec3::AXES[axis]) - s0).normalized();
            let grab = s0 + dir * 55.0;
            let by = h.screen_of(c + Vec3::AXES[axis] * mm) - s0;
            h.drag(grab, grab + by, 10, true);
            h.settle();
        };
        // Face mode: the +X face, out 8 mm
        h.key(egui::Key::Num3);
        let side = h.screen_of(Vec3::new(20.0, 10.0, 5.0));
        h.click(side);
        h.move_to(side + egui::vec2(0.0, 2.0));
        h.steps(2);
        h.shot("zero-comp-1-face-picked");
        pull(&mut h, 0, 8.0);
        h.shot("zero-comp-2-face-moved");
        // Edge mode: the top front edge, up 6 mm (the top and the front tilt)
        h.key(egui::Key::Num2);
        let edge = h.screen_of(Vec3::new(0.0, 20.0, 15.0));
        h.click(edge);
        h.steps(2);
        pull(&mut h, 1, 6.0);
        h.shot("zero-comp-3-edge-moved");
        // Vertex mode: the front left top corner (raised with the edge), up
        // 8 mm more: the top and the left side bend
        h.key(egui::Key::Num1);
        let corner = h.screen_of(Vec3::new(-20.0, 26.0, 15.0));
        h.click(corner);
        h.steps(2);
        h.shot("zero-comp-4a-vertex-picked");
        pull(&mut h, 1, 8.0);
        h.shot("zero-comp-4-vertex-moved");
        let made = h.st.doc.features.iter().filter_map(|f| f.tweak.as_ref().map(|t| (f.name.clone(), t.delta))).collect::<Vec<_>>();
        eprintln!("  moves {made:?}, errors {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
        h.st.quads = true;
        h.steps(2);
        h.settle();
        h.shot("zero-comp-5-quads");
        h.st.quads = false;

        // a thin plate: its front right corner rounded at 5 mm
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 3.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Plate");
        h.st.camera.frame(Vec3::new(-25.0, 0.0, -20.0), Vec3::new(25.0, 10.0, 20.0));
        h.steps(2);
        h.key(egui::Key::Num1);
        let corner = h.screen_of(Vec3::new(20.0, 3.0, 15.0));
        h.click(corner);
        h.steps(2);
        h.shot("zero-comp-6-corner-picked");
        h.st.round_corners(5.0);
        h.st.end_live();
        h.settle();
        // two more corners, picked together
        h.click(h.screen_of(Vec3::new(-20.0, 3.0, 15.0)));
        h.hold(egui::Modifiers::SHIFT);
        h.click(h.screen_of(Vec3::new(20.0, 3.0, -15.0)));
        h.hold(egui::Modifiers::NONE);
        h.st.round_corners(8.0);
        h.st.end_live();
        h.settle();
        h.move_to(h.screen_of(Vec3::new(0.0, -10.0, 40.0)));
        h.steps(2);
        h.shot("zero-comp-7-corners-rounded");
        eprintln!("  plate: {:?}", h.st.doc.features.iter().map(|f| (f.name.clone(), f.round.as_ref().map(|r| (r.size, r.edges.len())))).collect::<Vec<_>>());

        // a sketch (an L outline, 6 mm high): corners cut in its outline,
        // fillets and a chamfer (Romain 18 nov.: "Bevel fillet sur un path")
        let mut d = Doc::default();
        let id = d.add(Op::Add, Prim::Profile { x: 40.0, y: 6.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(id).unwrap().profile = Some(nurbs_kernel::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.0], [0.0, 0.0], [0.0, 0.5], [-0.5, 0.5]], smooth: vec![false; 6], corner: vec![], tension: vec![], open: false, spline: false });
        h.reset(d, "Sketch");
        h.st.camera.frame(Vec3::new(-25.0, 0.0, -20.0), Vec3::new(25.0, 10.0, 20.0));
        h.steps(2);
        h.key(egui::Key::Num1);
        // local (x, z) = the points scaled; the top at y = 6
        let at = |x: f32, z: f32| Vec3::new(x, 6.0, z);
        h.click(h.screen_of(at(20.0, -15.0)));
        h.hold(egui::Modifiers::SHIFT);
        h.click(h.screen_of(at(-20.0, 15.0)));
        h.hold(egui::Modifiers::NONE);
        h.steps(2);
        h.shot("zero-comp-8-sketch-corners-picked");
        h.st.set_profile_corners(6.0, false, true);
        h.st.end_live();
        h.settle();
        // the inner corner rounded, the outer one beside it bevelled
        h.click(h.screen_of(at(0.0, 0.0)));
        h.st.set_profile_corners(4.0, false, true);
        h.st.end_live();
        h.settle();
        h.click(h.screen_of(at(20.0, 0.0)));
        h.st.set_profile_corners(3.0, true, true);
        h.st.end_live();
        h.settle();
        h.steps(2);
        h.shot("zero-comp-9-sketch-corners-cut");
        eprintln!("  sketch corners {:?}, errors {:?}", h.st.doc.feature(id).and_then(|f| f.profile.as_ref()).map(|p| p.corner.clone()), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.st.quads = true;
        h.steps(2);
        h.settle();
        h.shot("zero-comp-10-sketch-quads");
        h.st.quads = false;
    }

    if want("path") {
        // the Path tool (P), Romain 18 nov.: "un path sans épaisseur": an
        // open path on the ground in front of the block (a curve point in
        // it), a closed one beside it, then fillets and a bevel on corners
        h.reset(test_part(), "Test part");
        h.st.camera.frame(Vec3::new(-60.0, 0.0, -20.0), Vec3::new(40.0, 25.0, 45.0));
        h.steps(2);
        h.key(egui::Key::P);
        let g = |h: &Harness, x: f32, z: f32| h.screen_of(Vec3::new(x, 0.0, z));
        h.click(g(&h, -35.0, 25.0));
        h.click(g(&h, -5.0, 25.0));
        h.click(g(&h, 5.0, 40.0));
        h.click(g(&h, 30.0, 40.0));
        h.drag(g(&h, 35.0, 30.0), g(&h, 40.0, 30.0), 8, true);
        h.click(g(&h, 30.0, 20.0));
        h.move_to(g(&h, 15.0, 15.0));
        h.steps(2);
        h.shot("zero-path-1-drawing");
        h.key(egui::Key::Enter);
        h.settle();
        h.shot("zero-path-2-open");
        // a closed one: back on its first point
        h.key(egui::Key::P);
        let first = g(&h, -55.0, -15.0);
        h.click(first);
        h.click(g(&h, -30.0, -15.0));
        h.click(g(&h, -30.0, 10.0));
        h.click(g(&h, -55.0, 10.0));
        h.move_to(first);
        h.steps(2);
        h.shot("zero-path-3-closing");
        h.click(first);
        h.settle();
        h.shot("zero-path-4-closed");
        for f in h.st.doc.features.iter().filter(|f| f.is_path()) {
            eprintln!("  {} {:?} open {:?} {:?}", f.name, f.prim, f.profile.as_ref().map(|p| p.open), f.profile.as_ref().map(|p| p.points.len()));
        }
        // Vertex mode: the open path's two sharp corners and a corner of
        // the closed one picked, rounded at 6 mm; then two more corners of
        // the closed one bevelled (the fourth is behind the block)
        h.st.camera.frame(Vec3::new(-58.0, 0.0, -17.0), Vec3::new(42.0, 0.0, 42.0));
        h.st.camera.pitch = 0.8;
        h.st.camera.target = Vec3::new(-8.0, 0.0, 12.0);
        h.st.camera.dist *= 0.62;
        h.steps(2);
        h.key(egui::Key::Num1);
        h.click(g(&h, -5.0, 25.0));
        h.hold(egui::Modifiers::SHIFT);
        h.click(g(&h, 5.0, 40.0));
        h.click(g(&h, -30.0, 10.0));
        h.hold(egui::Modifiers::NONE);
        h.steps(2);
        h.shot("zero-path-5-corners-picked");
        eprintln!("  picked corners {:?}", h.st.picked_profile_corners());
        h.st.set_profile_corners(6.0, false, true);
        h.st.end_live();
        h.settle();
        h.steps(2);
        h.shot("zero-path-6-filleted");
        h.click(g(&h, -55.0, -15.0));
        h.st.set_profile_corners(4.0, true, true);
        h.st.end_live();
        h.settle();
        h.click(g(&h, -55.0, 10.0));
        h.st.set_profile_corners(8.0, true, true);
        h.st.end_live();
        h.settle();
        h.steps(2);
        h.shot("zero-path-7-bevelled");
        for f in h.st.doc.features.iter().filter(|f| f.is_path()) {
            eprintln!("  {} corners {:?}", f.name, f.profile.as_ref().map(|p| p.corner.clone()));
        }
        // Object mode: a click on the open path selects it
        h.key(egui::Key::Num4);
        h.click(g(&h, -20.0, 25.0));
        h.steps(2);
        h.shot("zero-path-8-selected");
        eprintln!("  selected {:?}, model errors {:?}", h.st.selection, h.st.model.as_ref().map(|m| m.errors.clone()));
    }

    if want("pen") {
        // from Forme Rectify's Pen: Shift locks a side to 0°, 45°, 90°,
        // Ctrl + drag from a point placed sets its fillet
        h.reset(test_part(), "Test part");
        h.st.camera.frame(Vec3::new(-60.0, 0.0, 10.0), Vec3::new(20.0, 0.0, 50.0));
        h.steps(2);
        h.key(egui::Key::P);
        let g = |h: &Harness, x: f32, z: f32| h.screen_of(Vec3::new(x, 0.0, z));
        h.click(g(&h, -50.0, 20.0));
        h.click(g(&h, -20.0, 20.0));
        // a side drawn a little off the diagonal, Shift held: 45°
        h.hold(Modifiers::SHIFT);
        h.move_to(g(&h, -5.0, 37.0));
        h.click(g(&h, -5.0, 37.0));
        h.hold(Modifiers::NONE);
        h.click(g(&h, 15.0, 37.0));
        // Ctrl + drag from the second point: a 6 mm fillet
        h.hold(Modifiers::CTRL);
        h.drag(g(&h, -20.0, 20.0), g(&h, -20.0, 26.0), 8, false);
        h.steps(2);
        h.shot("zero-pen-1-fillet-drag");
        h.press(g(&h, -20.0, 26.0), false);
        h.hold(Modifiers::NONE);
        h.steps(2);
        if let Some(k) = &h.st.sketch {
            eprintln!("  pen points {:?} corners {:?}", k.pts, k.corner);
        }
        h.key(egui::Key::Enter);
        h.settle();
        h.shot("zero-pen-2-done");
        for f in h.st.doc.features.iter().filter(|f| f.is_path()) {
            eprintln!("  {} corners {:?}", f.name, f.profile.as_ref().map(|p| p.corner.clone()));
        }
    }

    if want("surface") {
        // the Surface tool: an outline filled with no thickness, a corner
        // filleted by Ctrl + drag, Ctrl + Z takes the last point off only
        h.reset(test_part(), "Test part");
        h.st.camera.frame(Vec3::new(-80.0, 0.0, -10.0), Vec3::new(20.0, 0.0, 60.0));
        h.steps(2);
        h.key(egui::Key::U);
        let g = |h: &Harness, x: f32, z: f32| h.screen_of(Vec3::new(x, 0.0, z));
        h.click(g(&h, -70.0, 10.0));
        h.click(g(&h, -30.0, 10.0));
        h.click(g(&h, -30.0, 50.0));
        h.click(g(&h, -60.0, 55.0));
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::Z);
        h.hold(Modifiers::NONE);
        eprintln!("  after Ctrl+Z: {:?} points", h.st.sketch.as_ref().map(|k| k.pts.len()));
        h.click(g(&h, -70.0, 50.0));
        h.hold(Modifiers::CTRL);
        h.drag(g(&h, -30.0, 10.0), g(&h, -30.0, 18.0), 8, false);
        h.steps(2);
        h.press(g(&h, -30.0, 18.0), false);
        h.hold(Modifiers::NONE);
        h.steps(2);
        if let Some(k) = &h.st.sketch {
            eprintln!("  surface points {:?} corners {:?}", k.pts.len(), k.corner);
        }
        h.shot("zero-surface-1-drawing");
        h.key(egui::Key::Enter);
        h.settle();
        h.shot("zero-surface-2-done");
        for f in h.st.doc.features.iter().filter(|f| f.is_sheet()) {
            eprintln!("  {} sheet points {}", f.name, f.sheet.as_ref().map(|s| s.pts.len()).unwrap_or(0));
        }
        eprintln!("  model errors {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
    }

    if want("surface-scan") {
        // on the gun's grip, on the scan (Tab switches in the middle of
        // the outline: two points on the plane), one corner filleted
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        if std::path::Path::new(&path).exists() {
            let mut d = Doc::default();
            d.reference = Some(nurbs_kernel::RefMesh { path: path.clone() });
            h.reset(d, "Gun scan");
            h.step();
            let end = Instant::now() + Duration::from_secs(180);
            while h.st.ref_busy() && Instant::now() < end {
                h.step();
                std::thread::sleep(Duration::from_millis(20));
            }
            h.settle();
            h.st.toast = None;
            let refm = h.st.reference.clone().expect("reference");
            let side = |x: f64, y: f64| -> Vec3 { refm.hit(DVec3::new(x, y, 100.0), DVec3::new(0.0, 0.0, -1.0)).map(|q| q.0.as_vec3()).unwrap_or(Vec3::new(x as f32, y as f32, 12.0)) };
            h.st.camera.yaw = 0.35;
            h.st.camera.pitch = 0.25;
            h.st.camera.frame(Vec3::new(-60.0, 5.0, 0.0), Vec3::new(-15.0, 40.0, 20.0));
            h.steps(2);
            h.key(egui::Key::U);
            h.key(egui::Key::Tab);
            eprintln!("  on scan: {}", h.st.surface_on_scan);
            for (x, y) in [(-48.0, 14.0), (-27.0, 14.0), (-27.0, 30.0)] {
                let p = h.screen_of(side(x, y));
                h.move_to(p);
                h.click(p);
            }
            h.key(egui::Key::Tab);
            let p = h.screen_of(side(-48.0, 30.0));
            h.move_to(p);
            h.click(p);
            eprintln!("  points {:?} free {:?}", h.st.sketch.as_ref().map(|k| k.pts.len()), h.st.sketch.as_ref().map(|k| k.free.iter().map(|f| f.is_some()).collect::<Vec<_>>()));
            h.hold(Modifiers::CTRL);
            let a = h.screen_of(side(-27.0, 14.0));
            let b = h.screen_of(side(-27.0, 19.0));
            h.drag(a, b, 8, false);
            h.steps(2);
            h.press(b, false);
            h.hold(Modifiers::NONE);
            h.steps(2);
            h.shot("zero-surface-scan-1-drawing");
            let t = Instant::now();
            h.key(egui::Key::Enter);
            h.settle();
            eprintln!("  built in {:.1} s", t.elapsed().as_secs_f64());
            h.shot("zero-surface-scan-2-done");
            let f = h.st.doc.features.iter().find(|f| f.is_sheet()).cloned();
            eprintln!("  sheet {:?} grid {:?}", f.as_ref().map(|f| f.name.clone()), f.as_ref().and_then(|f| f.sheet.as_ref().map(|s| s.grid)));
            if let Ok(dir) = std::env::var("FORME_SHEET_DUMP") {
                let _ = std::fs::write(format!("{dir}/sheet-doc.json"), serde_json::to_string(&h.st.doc).unwrap_or_default());
            }
            eprintln!("  faces {:?} tris {:?} errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()), h.st.model.as_ref().map(|m| m.triangles.len()), h.st.model.as_ref().map(|m| m.errors.clone()));
            // selectable in Object mode
            h.key(egui::Key::Q);
            h.st.selection.clear();
            let c = h.screen_of(side(-37.0, 22.0));
            h.click(c);
            h.steps(2);
            eprintln!("  selected {:?}", h.st.selection);
            h.shot("zero-surface-scan-3-selected");
            h.st.ref_shown = false;
            h.steps(2);
            h.shot("zero-surface-scan-4-alone");
            h.st.ref_shown = true;
            // Curve mode (SplitIt's): five points on the scan, one sharp
            h.key(egui::Key::U);
            h.st.surface_curve = true;
            h.st.surface_on_scan = true;
            h.key(egui::Key::Delete);
            for (x, y) in [(-46.0, 16.0), (-37.0, 19.0), (-28.0, 15.0), (-30.0, 28.0), (-45.0, 29.0)] {
                let p = h.screen_of(side(x, y));
                h.move_to(p);
                h.click(p);
            }
            h.hold(Modifiers::SHIFT);
            let p = h.screen_of(side(-28.0, 15.0));
            h.move_to(p);
            h.click(p);
            h.hold(Modifiers::NONE);
            eprintln!("  curve: sharp {:?}", h.st.sketch.as_ref().map(|k| k.sharp.clone()));
            h.shot("zero-surface-scan-5-curve");
            let t = Instant::now();
            h.key(egui::Key::Enter);
            h.settle();
            eprintln!("  curve built in {:.1} s, faces {:?}, errors {:?}", t.elapsed().as_secs_f64(), h.st.model.as_ref().map(|m| m.faces.len()), h.st.model.as_ref().map(|m| m.errors.clone()));
            h.shot("zero-surface-scan-6-curve-done");
        }
    }

    if want("rdrag") {
        // from Forme Rectify: a drag on an edge makes its fillet, the top
        // bar's option a chamfer, Ctrl + B, a drag on a sketch corner
        h.reset(test_part(), "Test part");
        h.key(egui::Key::Num2);
        let a = h.screen_of(Vec3::new(12.0, 20.0, 15.0));
        h.drag(a, a + egui::vec2(0.0, -40.0), 10, true);
        h.settle();
        h.shot("zero-rdrag-1-edge");
        for f in h.st.doc.features.iter().filter(|f| f.round.is_some()) {
            eprintln!("  {} size {:.3} chamfer {}", f.name, f.round.as_ref().unwrap().size, f.round.as_ref().unwrap().chamfer);
        }
        // a drag on the fillet's face: bigger
        let face = h.screen_of(Vec3::new(14.0, 19.6, 14.6));
        h.drag(face, face + egui::vec2(30.0, 0.0), 10, true);
        h.settle();
        for f in h.st.doc.features.iter().filter(|f| f.round.is_some()) {
            eprintln!("  after face drag: {} size {:.3}", f.name, f.round.as_ref().unwrap().size);
        }
        // the option on Chamfer: another edge
        h.st.round_chamfer = true;
        let b = h.screen_of(Vec3::new(20.0, 8.0, 15.0));
        h.drag(b, b + egui::vec2(30.0, 0.0), 10, true);
        h.settle();
        h.shot("zero-rdrag-2-chamfer");
        // Ctrl + B on a picked edge
        h.st.round_chamfer = false;
        h.key(egui::Key::Escape);
        h.key(egui::Key::Escape);
        let c = h.screen_of(Vec3::new(-20.0, 20.0, 0.0));
        h.click(c);
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::B);
        h.hold(Modifiers::NONE);
        h.settle();
        for f in h.st.doc.features.iter().filter(|f| f.round.is_some()) {
            eprintln!("  {} size {:.3} chamfer {}", f.name, f.round.as_ref().unwrap().size, f.round.as_ref().unwrap().chamfer);
        }
        h.shot("zero-rdrag-3-ctrl-b");
        // a sketch: its corner rounded by a drag in Vertex mode
        h.key(egui::Key::Num4);
        h.st.camera.frame(Vec3::new(-60.0, 0.0, 10.0), Vec3::new(20.0, 0.0, 50.0));
        h.steps(2);
        h.key(egui::Key::L);
        let g = |h: &Harness, x: f32, z: f32| h.screen_of(Vec3::new(x, 0.0, z));
        for (x, z) in [(-50.0, 20.0), (-20.0, 20.0), (-20.0, 45.0), (-50.0, 45.0), (-50.0, 20.0)] {
            h.click(g(&h, x, z));
        }
        h.click(g(&h, -30.0, 30.0) + egui::vec2(0.0, -40.0));
        h.settle();
        let sk = h.st.doc.features.iter().find(|f| f.profile.is_some() && !f.is_path()).map(|f| f.id);
        eprintln!("  sketch {:?}", sk);
        h.st.round_chamfer = false;
        h.key(egui::Key::Num1);
        h.steps(2);
        if let Some(id) = sk {
            let top = h.st.doc.feature(id).map(|f| f.prim.height()).unwrap_or(0.0);
            if let Some(v) = h.st.corner_vertex(id, 1, top) {
                let s = h.screen_of(v.as_vec3());
                h.drag(s, s + egui::vec2(-25.0, 25.0), 10, true);
                h.settle();
                eprintln!("  corners {:?} vert_sel {}", h.st.doc.feature(id).and_then(|f| f.profile.as_ref()).map(|p| p.corner.clone()), h.st.vert_sel.len());
                // the same corner dragged again, back towards its point: smaller
                if let Some(v) = h.st.vert_sel.first().copied() {
                    let s = h.screen_of(v.as_vec3());
                    h.drag(s, s + egui::vec2(10.0, -10.0), 10, true);
                    h.settle();
                    eprintln!("  smaller {:?}", h.st.doc.feature(id).and_then(|f| f.profile.as_ref()).map(|p| p.corner.clone()));
                }
            }
        }
        h.shot("zero-rdrag-4-corner");
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
    }

    if want("marquee") {
        // Maya: a rectangle picks the components the eye sees inside it
        // (F9 vertex, F10 edge, F11 face; Shift adds, Ctrl takes away)
        h.reset(test_part(), "Test part");
        let lo = h.screen_of(Vec3::new(-24.0, -2.0, 18.0));
        let hi = h.screen_of(Vec3::new(24.0, 32.0, -18.0));
        let (a, b) = (pos2(lo.x.min(hi.x) - 30.0, lo.y.max(hi.y) + 30.0), pos2(lo.x.max(hi.x) + 30.0, lo.y.min(hi.y) - 30.0));
        h.key(egui::Key::F10);
        let mid = h.screen_of(Vec3::new(4.0, 10.0, 15.0));
        h.drag(a, pos2(mid.x, b.y), 10, false);
        h.steps(2);
        h.shot("zero-marquee-1-edges-drawing");
        h.press(pos2(mid.x, b.y), false);
        h.settle();
        eprintln!("  edges picked {:?}", h.st.edge_sel);
        h.shot("zero-marquee-2-edges");
        h.key(egui::Key::F9);
        h.drag(a, b, 10, true);
        h.settle();
        eprintln!("  vertices picked {} of {} seen", h.st.vert_sel.len(), h.st.visible_vertices().len());
        h.shot("zero-marquee-3-vertices");
        h.key(egui::Key::F11);
        h.drag(a, b, 10, true);
        h.settle();
        eprintln!("  faces picked {}", h.st.face_sel.len());
        // Ctrl: the left half taken away
        h.hold(Modifiers::CTRL);
        h.drag(a, pos2(mid.x, b.y), 10, true);
        h.hold(Modifiers::NONE);
        h.settle();
        eprintln!("  faces after Ctrl {}", h.st.face_sel.len());
        h.shot("zero-marquee-4-faces");
        // a thin wall (2 mm): only what the camera sees, its far side never
        // (Romain 21 nov., 0 h 11: "un rectangle de sélection sélectionne
        // les faces arrières"); in X-ray the same. A box seen from a corner
        // shows 3 faces, 9 edges and 7 vertices.
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 60.0, y: 40.0, z: 2.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Thin wall");
        h.settle();
        let (a, b) = (pos2(420.0, 120.0), pos2(1500.0, 800.0));
        for xray in [false, true] {
            if h.st.xray != xray {
                h.hold(Modifiers::ALT);
                h.key(egui::Key::X);
                h.hold(Modifiers::NONE);
                h.settle();
            }
            h.key(egui::Key::F11);
            h.drag(a, b, 10, true);
            h.settle();
            let faces = h.st.face_sel.len();
            if !xray {
                h.shot("zero-marquee-5-thin-faces");
            }
            h.key(egui::Key::F10);
            h.drag(a, b, 10, true);
            h.settle();
            let edges = h.st.edge_sel.len();
            if xray {
                h.shot("zero-marquee-6-thin-xray-edges");
            }
            h.key(egui::Key::F9);
            h.drag(a, b, 10, true);
            h.settle();
            eprintln!("  thin wall, x-ray {}: {faces} faces, {edges} edges, {} vertices picked (seen: 3, 9, 7)", h.st.xray, h.st.vert_sel.len());
        }
        if h.st.xray {
            h.hold(Modifiers::ALT);
            h.key(egui::Key::X);
            h.hold(Modifiers::NONE);
        }
    }

    if want("direct") {
        // Shift + drag on a picked face: extrude; Ctrl + drag: inset;
        // Shift + drag on a picked edge: the edge pulled out
        h.reset(test_part(), "Test part");
        h.key(egui::Key::F11);
        let side = h.screen_of(Vec3::new(20.0, 8.0, 2.0));
        h.click(side);
        let out = h.screen_of(Vec3::new(26.0, 8.0, 2.0));
        h.hold(Modifiers::SHIFT);
        h.press(side, true);
        for i in 1..=10 {
            h.move_to(side + (out - side) * (i as f32 / 10.0));
        }
        h.steps(2);
        h.shot("zero-direct-1-extrude-drag");
        h.press(out, false);
        h.hold(Modifiers::NONE);
        h.settle();
        let pushes = |h: &Harness| h.st.doc.features.iter().filter_map(|f| f.push.as_ref().map(|p| (f.name.clone(), p.inset, p.depth))).collect::<Vec<_>>();
        eprintln!("  after Shift drag {:?} errors {:?}", pushes(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-direct-2-extruded");
        // the front face, Ctrl + drag: an inset
        h.key(egui::Key::Escape);
        h.key(egui::Key::F11);
        let front = h.screen_of(Vec3::new(10.0, 7.0, 15.0));
        h.click(front);
        h.hold(Modifiers::CTRL);
        h.press(front, true);
        for i in 1..=10 {
            h.move_to(front + egui::vec2(3.0 * i as f32, 0.0));
        }
        h.steps(2);
        h.shot("zero-direct-3-inset-drag");
        h.press(front + egui::vec2(30.0, 0.0), false);
        h.hold(Modifiers::NONE);
        h.settle();
        eprintln!("  after Ctrl drag {:?} errors {:?}", pushes(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-direct-4-inset");
        // an edge of a plain block (the faces round a moved edge must have
        // no hole): its top front edge, Shift + drag up
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Block");
        h.key(egui::Key::F10);
        let e = h.screen_of(Vec3::new(-14.0, 20.0, 15.0));
        h.click(e);
        eprintln!("  edges picked {:?}", h.st.edge_sel);
        h.hold(Modifiers::SHIFT);
        h.press(e, true);
        for i in 1..=10 {
            h.move_to(e + egui::vec2(0.0, -14.0 * i as f32));
        }
        h.steps(2);
        h.shot("zero-direct-5-edge-drag");
        h.press(e + egui::vec2(0.0, -140.0), false);
        h.hold(Modifiers::NONE);
        h.settle();
        eprintln!("  edge {:?} errors {:?}", h.st.doc.features.iter().filter_map(|f| f.tweak.as_ref().map(|t| (f.name.clone(), t.delta))).collect::<Vec<_>>(), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-direct-6-edge");
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
    }

    if want("quick-extrude") {
        // Romain 20 nov., 11 h 54: "Une Edge ou une Face > Shift + Drag"
        // (the move gizmo sits on the picked face: the press lands on it),
        // and "Q, W ou Entrée valide l'opération et sort du tool"
        h.reset(test_part(), "Test part");
        h.settle();
        h.key(egui::Key::W);
        h.key(egui::Key::F11);
        let side = h.screen_of(Vec3::new(20.0, 8.0, 2.0));
        h.click(side);
        h.settle();
        eprintln!("  picked {:?} gizmo {:?} hot {:?}", h.st.face_sel.len(), h.st.gizmo, h.st.hot_handle);
        let out = h.screen_of(Vec3::new(27.0, 8.0, 2.0));
        h.hold(Modifiers::SHIFT);
        h.drag(side, out, 10, false);
        h.steps(2);
        h.shot("zero-quick-1-shift-drag");
        h.press(out, false);
        h.hold(Modifiers::NONE);
        h.settle();
        let pushes = |h: &Harness| h.st.doc.features.iter().filter_map(|f| f.push.as_ref().map(|p| (f.name.clone(), (p.depth * 100.0).round() / 100.0))).collect::<Vec<_>>();
        let tweaks = |h: &Harness| h.st.doc.features.iter().filter_map(|f| f.tweak.as_ref().map(|t| (f.name.clone(), t.delta))).collect::<Vec<_>>();
        eprintln!("  Shift drag on the gizmo: pushes {:?} moves {:?}", pushes(&h), tweaks(&h));
        // Ctrl + E on the front face, then W: kept as it is, back to Select
        h.key(egui::Key::Escape);
        h.key(egui::Key::F11);
        let front = h.screen_of(Vec3::new(10.0, 7.0, 15.0));
        h.click(front);
        h.move_to(front);
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::E);
        h.hold(Modifiers::NONE);
        for i in 1..=8 {
            h.move_to(front + egui::vec2(0.0, 6.0 * i as f32));
        }
        h.steps(2);
        h.shot("zero-quick-2-extrude-live");
        h.key(egui::Key::W);
        h.settle();
        eprintln!("  W during the extrusion: tool {:?} push {} pushes {:?}", h.st.tool, h.st.push.is_some(), pushes(&h));
        h.shot("zero-quick-3-w-kept");
        // a Sketch of three points, then Q: closed and made (no height
        // yet: its base's smaller side)
        h.key(egui::Key::L);
        for at in [Vec3::new(-18.0, 20.0, -13.0), Vec3::new(-6.0, 20.0, -13.0), Vec3::new(-12.0, 20.0, -7.0)] {
            let p = h.screen_of(at);
            h.move_to(p);
            h.click(p);
        }
        eprintln!("  sketch before Q: tool {:?} points {:?}", h.st.tool, h.st.sketch.as_ref().map(|k| k.pts.len()));
        h.key(egui::Key::Q);
        h.settle();
        eprintln!("  Q in a sketch: tool {:?} sketch {} creation {} last {:?}", h.st.tool, h.st.sketch.is_some(), h.st.creation.is_some(), h.st.doc.features.last().map(|f| (f.name.clone(), f.prim.clone())));
        h.shot("zero-quick-4-sketch-q");
        // a Cube in its height phase, Enter: kept at that height
        h.key(egui::Key::Escape);
        h.key(egui::Key::B);
        let a = h.screen_of(Vec3::new(28.0, 0.0, -28.0));
        let b = h.screen_of(Vec3::new(42.0, 0.0, -14.0));
        h.drag(a, b, 8, true);
        for i in 1..=6 {
            h.move_to(b + egui::vec2(0.0, -8.0 * i as f32));
        }
        h.steps(2);
        eprintln!("  before Enter: tool {:?} creation {:?} a {a:?} b {b:?}", h.st.tool, h.st.creation.as_ref().map(|c| (c.phase, c.height)));
        h.key(egui::Key::Enter);
        h.settle();
        eprintln!("  Enter in the height: tool {:?} creation {} last {:?}", h.st.tool, h.st.creation.is_some(), h.st.doc.features.last().map(|f| (f.name.clone(), f.prim.clone())));
        h.shot("zero-quick-5-cube-enter");
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
    }

    if want("scale") {
        // R: the classic gizmo again (Romain 21 nov., 0 h 11: "oublie la box
        // à poignées et reviens au classique"): a cube per axis of the
        // shape's own frame, the centre for all three; then picked faces
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Add, Prim::Box { x: 10.0, y: 10.0, z: 10.0 }, DVec3::new(-45.0, 0.0, 0.0), DQuat::IDENTITY);
        h.reset(d, "Scale");
        h.settle();
        h.st.selection = vec![b];
        h.key(egui::Key::R);
        h.settle();
        let say = |h: &Harness, what: &str| {
            let f = h.st.doc.feature(b).unwrap();
            eprintln!("  {what}: {:?} at {:?} readout {:?}", f.prim, f.origin, h.st.gizmo_readout);
        };
        // where the gizmo's cube along an axis is on screen, and a point
        // further along it
        let tip = |h: &Harness, a: usize, far: f32| {
            let pivot = h.st.pivot().unwrap().as_vec3();
            let v = h.st.view();
            let k = v.pixel_size_at((pivot - v.eye).length()) * 78.0;
            (h.screen_of(pivot + Vec3::AXES[a] * k), h.screen_of(pivot + Vec3::AXES[a] * k * far))
        };
        let (g, out) = tip(&h, 2, 1.5);
        h.move_to(g);
        h.steps(2);
        eprintln!("  hot {:?}", h.st.hot_handle);
        h.shot("zero-scale-1-hover");
        h.drag(g, out, 10, false);
        h.steps(2);
        say(&h, "the Z cube dragged out");
        h.shot("zero-scale-2-axis");
        h.press(out, false);
        h.settle();
        say(&h, "let go");
        // the centre: to the right, bigger all round
        let c = h.screen_of(h.st.pivot().unwrap().as_vec3());
        h.move_to(c);
        h.steps(2);
        eprintln!("  hot {:?}", h.st.hot_handle);
        h.drag(c, c + vec2(60.0, 0.0), 10, false);
        h.steps(2);
        say(&h, "the centre dragged right");
        h.shot("zero-scale-3-all");
        h.press(c + vec2(60.0, 0.0), false);
        h.settle();
        say(&h, "let go");
        // Face mode: the block's top picked, R, its X cube pulled out: the
        // face scaled round its middle
        h.key(egui::Key::F11);
        let (_, hi) = h.st.doc.feature(b).unwrap().bounds();
        let top = h.screen_of(Vec3::new(hi.x as f32 * 0.3, hi.y as f32, hi.z as f32 * 0.3));
        h.click(top);
        h.key(egui::Key::R);
        h.settle();
        eprintln!("  faces {} gizmo {:?} cage drawn: no", h.st.face_sel.len(), h.st.gizmo);
        let (g, out) = tip(&h, 0, 0.6);
        h.move_to(g);
        h.steps(2);
        eprintln!("  hot {:?}", h.st.hot_handle);
        h.shot("zero-scale-4-face");
        h.drag(g, out, 10, false);
        h.steps(2);
        h.shot("zero-scale-5-face-drag");
        h.press(out, false);
        h.settle();
        eprintln!("  scale faces {:?} errors {:?}", h.st.doc.features.iter().filter_map(|f| f.tweak.as_ref().map(|t| (f.name.clone(), t.scale, t.delta, t.about))).collect::<Vec<_>>(), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-scale-6-face-done");
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
    }

    if want("knife-chain") {
        // Romain 20 nov., 12 h 09: a point (an edge's end or middle) catches
        // first within 12 points; the cut goes on from its last point
        h.reset(test_part(), "Test part");
        h.settle();
        h.key(egui::Key::S);
        let corner = h.screen_of(Vec3::new(-20.0, 20.0, 15.0));
        h.move_to(corner + egui::vec2(7.0, 6.0));
        h.steps(2);
        eprintln!("  9 points from the corner: hover {:?} point {}", h.st.knife_hover, h.st.knife_hover_point);
        let pp = corner + egui::vec2(7.0, 6.0);
        if let Some(m) = h.st.model.clone() {
            for (i, e) in m.edges.iter().enumerate() {
                let (a, b) = (Vec3::from(e.pts[0]), Vec3::from(*e.pts.last().unwrap()));
                if a.distance(Vec3::new(-20.0, 20.0, 15.0)) < 1.0 || b.distance(Vec3::new(-20.0, 20.0, 15.0)) < 1.0 {
                    eprintln!("    edge {i} {a:?} -> {b:?} seam {} n {} screen {:?} {:?}", e.seam, e.pts.len(), h.st.view().project(a), h.st.view().project(b));
                }
            }
        }
        eprintln!("  knife_point {:?} hidden {} corner on screen {:?} pointer {:?}", h.st.knife_point([pp.x, pp.y]), h.st.hidden(Vec3::new(-20.0, 20.0, 15.0)), corner, pp);
        h.shot("zero-knife-chain-1-catch");
        let pts = [Vec3::new(-20.0, 15.0, 15.0), Vec3::new(-12.0, 0.0, 15.0), Vec3::new(20.0, 6.0, 15.0)];
        for (i, at) in pts.iter().enumerate() {
            let p = h.screen_of(*at);
            h.move_to(p);
            h.steps(2);
            h.click(p);
            h.settle();
            let next = pts.get(i + 1).map(|q| h.screen_of(*q)).unwrap_or(p + egui::vec2(0.0, -60.0));
            h.move_to(p + (next - p) * 0.6);
            h.steps(2);
            eprintln!("  click {i}: knife {:?} last {:?} tool {:?}", h.st.knife, h.st.knife_last(), h.st.tool);
            h.shot(&format!("zero-knife-chain-{}-after-{i}", i + 2));
        }
        h.key(egui::Key::Enter);
        h.settle();
        for f in h.st.doc.features.iter().filter(|f| f.cut.is_some()) {
            let k = f.cut.as_ref().unwrap();
            eprintln!("  {} {} points {:?} lines", f.name, k.pts.len(), k.segs);
        }
        eprintln!("  Enter: tool {:?} faces {} errors {:?}", h.st.tool, h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-knife-chain-5-done");
    }

    if want("inset") {
        // insets that failed before 19 nov.: the top left of the hole (1 mm
        // from the slot), the top round the boss (the boss crosses its
        // border), a pocket; then one too wide that stops
        let mut d = test_part();
        let p1 = d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 1, sub: 2 }.pack(), at: DVec3::new(-15.0, 20.0, 10.0) }, inset: 2.0, depth: -2.0 });
        let p2 = d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 1, sub: 2 }.pack(), at: DVec3::new(15.0, 20.0, 10.0) }, inset: 2.0, depth: 1.5 });
        h.reset(d, "Insets");
        h.settle();
        eprintln!("  errors {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-inset-1-near-holes");
        h.st.selection = vec![p2];
        h.st.set_push(30.0, 1.5, true);
        h.st.end_live();
        h.settle();
        h.settle();
        eprintln!("  too wide: inset now {:?} (p1 {p1}) errors {:?}", h.st.doc.feature(p2).and_then(|f| f.push.as_ref()).map(|p| p.inset), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-inset-2-too-wide");
    }

    if want("help") {
        // the help box at the bottom right (Plasticity style), by context
        h.reset(test_part(), "Test part");
        h.shot("zero-help-1-object");
        h.key(egui::Key::F10);
        let e = h.screen_of(Vec3::new(-12.0, 20.0, 15.0));
        h.click(e);
        h.settle();
        h.shot("zero-help-2-edge");
        h.key(egui::Key::Escape);
        h.key(egui::Key::Escape);
        h.key(egui::Key::F8);
        h.key(egui::Key::L);
        h.steps(2);
        h.shot("zero-help-3-sketch");
        h.key(egui::Key::Escape);
        h.st.settings.help = false;
        h.steps(2);
        h.shot("zero-help-4-folded");
        h.st.settings.help = true;
    }

    if want("outliner") {
        // the outliner by kind, and the clicked element's operations in a
        // second column
        let mut d = test_part();
        d.add_round(2.0, false, vec![EdgeRef { faces: [FaceTag { feature: 1, sub: 2 }.pack(), FaceTag { feature: 1, sub: 4 }.pack()], at: DVec3::new(12.0, 20.0, 15.0) }]);
        d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 1, sub: 4 }.pack(), at: DVec3::new(-12.0, 8.0, 15.0) }, inset: 2.0, depth: -1.0 });
        d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 3, sub: 1 }.pack(), at: DVec3::new(10.0, 30.0, 0.0) }, inset: 2.0, depth: 2.0 });
        h.reset(d, "Test part");
        h.st.select(Some(1), false);
        h.settle();
        h.shot("zero-outliner-1-block");
        h.st.select(Some(7), false);
        h.steps(3);
        h.shot("zero-outliner-2-boss-step");
        for f in &h.st.doc.features {
            eprintln!("  {} {:?} owner {:?}", f.name, f.id, h.st.owner_of(f.id));
        }
    }

    if want("ipad") {
        ipad(&mut h);
    }

    if want("ipad-studio") {
        ipad_studio(&mut h);
    }

    if want("studio-lamp") {
        studio_lamp(&mut h);
    }

    if want("groups") {
        // Romain (21 nov., 12 h 37): groups, "le meilleur de Maya et de 3ds
        // Max": three objects grouped (closed: a click takes them all), the
        // group's own transform (moved, turned, scaled), opened to move one
        // shape alone, closed again, copied, ungrouped, deleted
        let tag = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
        let mut d = Doc::default();
        let block = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let hole = d.add(Op::Carve, Prim::Cylinder { radius: 6.0, height: 30.0 }, DVec3::new(-8.0, -5.0, 0.0), DQuat::IDENTITY);
        let post = d.add(Op::Add, Prim::Cylinder { radius: 9.0, height: 32.0 }, DVec3::new(42.0, 0.0, -4.0), DQuat::IDENTITY);
        let small = d.add(Op::Add, Prim::Box { x: 14.0, y: 14.0, z: 14.0 }, DVec3::new(30.0, 0.0, 34.0), DQuat::IDENTITY);
        let far = d.add(Op::Add, Prim::Box { x: 18.0, y: 10.0, z: 18.0 }, DVec3::new(-50.0, 0.0, 30.0), DQuat::IDENTITY);
        let fillet = d.add_round(4.0, false, vec![EdgeRef { faces: [tag(block, 0), tag(block, 2)], at: DVec3::new(20.0, 20.0, 0.0) }]);
        h.reset(d, "Groups");
        h.st.camera.frame(Vec3::new(-70.0, 0.0, -40.0), Vec3::new(120.0, 45.0, 60.0));
        h.steps(2);
        h.settle();
        h.key(egui::Key::F8);
        let say = |h: &Harness, what: &str| {
            let e = |q: DQuat| {
                let (x, y, z) = q.to_euler(glam::EulerRot::XYZ);
                format!("{:.1} {:.1} {:.1}", x.to_degrees(), y.to_degrees(), z.to_degrees())
            };
            eprintln!("  {what}: {} group(s), selection {:?}, selected group {:?}, errors {}, version {}", h.st.doc.groups.len(), h.st.selection, h.st.selected_group().map(|g| g.name.clone()), h.st.model.as_ref().map(|m| m.errors.len()).unwrap_or(99), h.st.version);
            for g in &h.st.doc.groups {
                eprintln!("    {} {:?} at ({:.2} {:.2} {:.2}) rot ({}) scale ({:.2} {:.2} {:.2}) {}", g.name, g.members, g.origin.x, g.origin.y, g.origin.z, e(g.rot), g.scale.x, g.scale.y, g.scale.z, if g.open { "open" } else { "closed" });
            }
            if let Some(t) = h.st.toast.as_ref() {
                eprintln!("    toast: {}", t.text);
            }
        };
        let at = |h: &Harness, id: u32| h.st.doc.feature(id).map(|f| format!("({:.2} {:.2} {:.2})", f.origin.x, f.origin.y, f.origin.z)).unwrap_or_default();
        let on = |h: &Harness, id: u32| {
            let f = h.st.doc.feature(id).unwrap();
            h.screen_of((f.origin + f.axis() * f.prim.height()).as_vec3())
        };
        // the block, then the hole's, the post and the small cube added
        h.click(on(&h, block));
        h.hold(Modifiers::SHIFT);
        h.click(on(&h, post));
        h.click(on(&h, small));
        h.hold(Modifiers::NONE);
        h.st.selection.push(hole);
        h.settle();
        h.shot("zero-groups-1-picked");
        // Ctrl + G
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::G);
        h.hold(Modifiers::NONE);
        h.settle();
        say(&h, "grouped");
        let gid = h.st.doc.groups[0].id;
        h.shot("zero-groups-2-grouped");
        // deselected; the pointer on the post lights the whole group
        h.key(egui::Key::Escape);
        h.steps(2);
        let p = on(&h, post);
        h.move_to(p);
        h.steps(3);
        eprintln!("  hover {:?} with {:?}", h.st.hover_feature, h.st.hover_also);
        h.shot("zero-groups-3-hover");
        // a click on one of its shapes takes the group
        h.click(p);
        h.settle();
        say(&h, "clicked the post");
        // moved by its arrow: every shape, the hole and the fillet with it
        let c = h.st.pivot().expect("a gizmo").as_vec3();
        let s0 = h.screen_of(c);
        let dir = (h.screen_of(c + Vec3::X) - s0).normalized();
        let grab = s0 + dir * 55.0;
        let to = h.screen_of(c + Vec3::X * 30.0) - s0;
        h.drag(grab, grab + to, 10, false);
        h.steps(3);
        h.shot("zero-groups-4-moving");
        h.press(grab + to, false);
        h.settle();
        say(&h, "moved 30 along X");
        eprintln!("    block {} hole {} post {} small {} far {} fillet at {:?}", at(&h, block), at(&h, hole), at(&h, post), at(&h, small), at(&h, far), h.st.doc.feature(fillet).and_then(|f| f.round.as_ref()).map(|r| r.edges[0].at));
        h.shot("zero-groups-5-moved");
        // E: turned round its pivot
        h.key(egui::Key::E);
        h.settle();
        let pivot = h.st.pivot().unwrap().as_vec3();
        let v = h.st.view();
        let k = v.pixel_size_at((pivot - v.eye).length()) * 78.0 * 0.9;
        let eye = v.eye - pivot;
        let ring = |t: f32| pivot + (Vec3::Z * t.cos() + Vec3::X * t.sin()) * k;
        let t0 = [45f32, 135.0, 225.0, 315.0].into_iter().map(f32::to_radians).max_by(|a, b| (ring(*a) - pivot).dot(eye).total_cmp(&(ring(*b) - pivot).dot(eye))).unwrap();
        let (a, b) = (h.screen_of(ring(t0)), h.screen_of(ring(t0 + 33f32.to_radians())));
        h.move_to(a);
        h.steps(2);
        h.drag(a, b, 12, true);
        h.settle();
        say(&h, "turned");
        eprintln!("    block {} post {} small {}", at(&h, block), at(&h, post), at(&h, small));
        h.shot("zero-groups-6-turned");
        // D + click: its pivot on the block's corner (kept in the document),
        // then back at the middle from the panel's "Centre pivot"
        h.key(egui::Key::W);
        h.settle();
        let corner = h.st.doc.feature(block).unwrap().corners()[1].as_vec3();
        h.key_hold(egui::Key::D, true);
        h.move_to(h.screen_of(corner) + vec2(3.0, -2.0));
        h.steps(2);
        h.click(h.screen_of(corner) + vec2(3.0, -2.0));
        h.key_hold(egui::Key::D, false);
        h.steps(2);
        say(&h, "its pivot put with D on the block's corner");
        eprintln!("    corner {corner:?} pivot {:?}", h.st.pivot());
        h.shot("zero-groups-6b-pivot");
        h.st.put_group_pivot(gid, None);
        h.steps(2);
        say(&h, "pivot centred again");
        // its transform properties: scale 1.25 from the panel
        let g = h.st.doc.group(gid).unwrap().clone();
        h.st.set_group_transform(gid, g.origin, g.rot, DVec3::splat(1.25), "Scale group", true);
        h.st.end_live();
        h.key(egui::Key::W);
        h.settle();
        say(&h, "scaled 1.25 from its properties");
        eprintln!("    block {:?} post {:?}", h.st.doc.feature(block).map(|f| f.prim), h.st.doc.feature(post).map(|f| f.prim));
        h.shot("zero-groups-7-scaled");
        // its bar: Closed, a click opens it
        let toggle = |h: &Harness| h.ctx.data(|d| d.get_temp::<Pos2>(egui::Id::new("group-bar-toggle"))).expect("the group's bar");
        let t = toggle(&h);
        h.move_to(t);
        h.steps(2);
        h.click(t);
        h.settle();
        say(&h, "opened from its bar");
        h.shot("zero-groups-8-open");
        // open: the small cube alone, moved up 12; the group's bar stays
        h.click(on(&h, small));
        h.settle();
        say(&h, "the small cube picked alone");
        let c = h.st.pivot().expect("a gizmo").as_vec3();
        let s0 = h.screen_of(c);
        let dir = (h.screen_of(c + Vec3::Y) - s0).normalized();
        let grab = s0 + dir * 55.0;
        let to = h.screen_of(c + Vec3::Y * 12.0) - s0;
        h.drag(grab, grab + to, 10, true);
        h.settle();
        say(&h, "the small cube moved up");
        eprintln!("    small {} block {}", at(&h, small), at(&h, block));
        h.shot("zero-groups-9-member-moved");
        // closed again from the bar: the group is one thing again, selected
        let t = toggle(&h);
        h.move_to(t);
        h.steps(2);
        h.click(t);
        h.settle();
        say(&h, "closed again");
        h.shot("zero-groups-10-closed");
        // a rectangle over the far cube and one shape of the group: the whole group comes
        h.key(egui::Key::Escape);
        h.steps(2);
        let (a, b) = (on(&h, far), on(&h, small));
        let lo = pos2(a.x.min(b.x) - 30.0, a.y.min(b.y) - 40.0);
        let hi = pos2(a.x.max(b.x) + 12.0, a.y.max(b.y) + 30.0);
        h.drag(lo, hi, 8, false);
        h.steps(2);
        h.shot("zero-groups-11-rectangle");
        h.press(hi, false);
        h.settle();
        say(&h, "rectangle over the far cube and the small one");
        // Ctrl + D on the group: a copy, a group too, moved aside
        h.key(egui::Key::Escape);
        h.steps(2);
        h.click(on(&h, post));
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::D);
        h.hold(Modifiers::NONE);
        h.settle();
        say(&h, "duplicated");
        let c = h.st.pivot().expect("a gizmo").as_vec3();
        let s0 = h.screen_of(c);
        let dir = (h.screen_of(c - Vec3::Z) - s0).normalized();
        let to = h.screen_of(c - Vec3::Z * 110.0) - s0;
        // the arrow points along +Z: grabbed there, pulled back
        let grab = s0 - dir * 55.0;
        h.move_to(grab);
        h.steps(2);
        h.drag(grab, grab + to, 10, true);
        h.settle();
        say(&h, "the copy moved along -Z");
        h.st.frame_all();
        h.steps(30);
        h.settle();
        h.shot("zero-groups-12-copy");
        // Ctrl + Shift + G: the copy ungrouped, its shapes stay
        let before = h.st.version;
        h.hold(Modifiers::CTRL | Modifiers::SHIFT);
        h.key(egui::Key::G);
        h.hold(Modifiers::NONE);
        h.settle();
        say(&h, "the copy ungrouped");
        h.shot("zero-groups-13-ungrouped");
        // undone: the group is back, the part not built again
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::Z);
        h.hold(Modifiers::NONE);
        h.settle();
        say(&h, "ungroup undone");
        eprintln!("    builds asked meanwhile: {}", h.st.version - before);
        // the first group selected by its row in the outliner, deleted, undone
        let row = h.ctx.data(|d| d.get_temp::<Pos2>(egui::Id::new(("group-row", gid)))).expect("the group's row");
        h.click(row);
        h.settle();
        say(&h, "its row clicked");
        h.shot("zero-groups-14-row");
        h.key(egui::Key::Delete);
        h.settle();
        say(&h, "deleted");
        eprintln!("    features left {}", h.st.doc.features.len());
        h.shot("zero-groups-15-deleted");
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::Z);
        h.hold(Modifiers::NONE);
        h.settle();
        say(&h, "delete undone");
        eprintln!("    features {}", h.st.doc.features.len());
    }

    if want("orphans") {
        // operations left by a shape deleted in a document from before
        // 20 nov. (FORME_DOC: a saved scene): the outliner's row takes them
        // away, one undo step
        let d = match std::env::var("FORME_DOC") {
            Ok(path) => nurbs_kernel::Doc::from_json(&std::fs::read_to_string(&path).expect("FORME_DOC")).expect("a document"),
            Err(_) => {
                let mut d = test_part();
                d.add_round(2.0, false, vec![EdgeRef { faces: [FaceTag { feature: 1, sub: 2 }.pack(), FaceTag { feature: 1, sub: 4 }.pack()], at: DVec3::new(12.0, 20.0, 15.0) }]);
                d.add_round(1.0, false, vec![EdgeRef { faces: [FaceTag { feature: 3, sub: 0 }.pack(), FaceTag { feature: 3, sub: 1 }.pack()], at: DVec3::new(17.0, 30.0, 0.0) }]);
                d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 3, sub: 1 }.pack(), at: DVec3::new(10.0, 30.0, 0.0) }, inset: 2.0, depth: 2.0 });
                // the boss gone, as an old Delete left it
                d.features.retain(|f| f.id != 3);
                d
            }
        };
        h.reset(d, "Leftovers");
        h.settle();
        let errors = |h: &Harness| h.st.model.as_ref().map(|m| m.errors.len()).unwrap_or(0);
        let row = h.ctx.data(|d| d.get_temp::<Pos2>(egui::Id::new("orphans-row"))).expect("the leftover row");
        h.move_to(row);
        h.steps(3);
        h.shot("zero-orphans-1-row");
        eprintln!("  {} features, {} leftover, {} errors", h.st.doc.features.len(), h.st.doc.orphans().len(), errors(&h));
        h.click(row);
        h.settle();
        h.move_to(pos2(800.0, 300.0));
        h.steps(3);
        h.shot("zero-orphans-2-removed");
        eprintln!("  {} features, {} leftover, {} errors, toast {:?}", h.st.doc.features.len(), h.st.doc.orphans().len(), errors(&h), h.st.toast.as_ref().map(|t| t.text.clone()));
        h.st.undo();
        h.settle();
        eprintln!("  undone: {} features, {} leftover, {} errors", h.st.doc.features.len(), h.st.doc.orphans().len(), errors(&h));
    }

    if want("history") {
        // the history strip: the Block's own history (its fillet and
        // inset), back to the fillet, scrubbed, the fillet turned off in
        // the pop-up; the fixed Sub Outliner; the scene's undo history
        let mut d = test_part();
        d.add_round(2.0, false, vec![EdgeRef { faces: [FaceTag { feature: 1, sub: 2 }.pack(), FaceTag { feature: 1, sub: 4 }.pack()], at: DVec3::new(12.0, 20.0, 15.0) }]);
        d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 1, sub: 4 }.pack(), at: DVec3::new(-12.0, 8.0, 15.0) }, inset: 2.0, depth: -1.0 });
        d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 3, sub: 1 }.pack(), at: DVec3::new(10.0, 30.0, 0.0) }, inset: 2.0, depth: 2.0 });
        h.reset(d, "Test part");
        let cell = |h: &Harness, i: usize| h.ctx.data(|d| d.get_temp::<Pos2>(egui::Id::new(("hist-cell", i)))).expect("cell drawn");
        let row = |h: &Harness, id: u64| h.ctx.data(|d| d.get_temp::<Pos2>(egui::Id::new(("row", id)))).expect("row drawn");
        h.st.select(Some(1), false);
        h.steps(3);
        h.move_to(cell(&h, 1));
        h.steps(3);
        h.shot("zero-history-1-block");
        h.click(cell(&h, 1));
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-history-2-back");
        eprintln!("  marker {} suspended {:?}", h.st.marker_of(1), h.st.doc.features.iter().filter(|f| f.suspended).map(|f| f.name.clone()).collect::<Vec<_>>());
        // held on the last icon and dragged to the first: the shape alone
        let (a, b) = (cell(&h, 2), cell(&h, 0));
        h.drag(a, b, 10, false);
        h.settle();
        h.shot("zero-history-3-scrub");
        eprintln!("  scrubbed to {}", h.st.marker_of(1));
        h.drag(b, a, 6, false);
        h.press(a, false);
        h.settle();
        eprintln!("  released at {} · undo {:?}", h.st.marker_of(1), h.st.undo_labels());
        // the fillet's pop-up and its tick: turned off, the inset still built
        h.move_to(cell(&h, 1));
        h.steps(3);
        let c = cell(&h, 1);
        h.move_to(c + vec2(0.0, -30.0));
        h.steps(2);
        let tick = h.ctx.data(|d| d.get_temp::<Pos2>(egui::Id::new("history-tick"))).expect("pop-up tick");
        h.move_to(tick);
        h.steps(2);
        h.click(tick);
        h.settle();
        h.move_to(tick);
        h.steps(3);
        h.shot("zero-history-4-off");
        eprintln!("  fillet visible {} errors {:?}", h.st.doc.features[4].visible, h.st.model.as_ref().map(|m| m.errors.clone()));
        // the fixed Sub Outliner, a right click on the inset's row
        h.st.settings.sub_fixed = true;
        h.steps(3);
        let r = row(&h, 6);
        h.rpress(r, true);
        h.rpress(r, false);
        h.steps(3);
        h.shot("zero-history-5-fixed-menu");
        h.key(egui::Key::Escape);
        // a new operation while the marker is back goes in at the marker
        h.st.set_marker(1, 1);
        h.settle();
        h.st.edit("Inset", |d| {
            d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 1, sub: 0 }.pack(), at: DVec3::new(20.0, 10.0, 0.0) }, inset: 3.0, depth: -1.0 });
        });
        h.settle();
        eprintln!("  order {:?}", h.st.doc.features.iter().map(|f| format!("{}{}", f.name, if f.suspended { " (out)" } else { "" })).collect::<Vec<_>>());
        h.st.select(Some(1), false);
        h.steps(3);
        h.shot("zero-history-6-inserted");
        // nothing selected: the scene's undo history
        h.st.select(None, false);
        h.st.settings.sub_fixed = false;
        h.steps(3);
        h.move_to(cell(&h, 2));
        h.steps(3);
        h.shot("zero-history-7-scene");
    }

    if want("qwer") {
        // Maya's Q W E R: E rings turn the slot, R cubes scale the boss, the
        // centre scales all, Q hides the gizmo, W brings the arrows back
        h.reset(test_part(), "Test part");
        h.st.select(Some(4), false);
        h.key(egui::Key::E);
        h.settle();
        h.shot("zero-qwer-1-rotate");
        let pivot = h.st.pivot().unwrap().as_vec3();
        let v = h.st.view();
        let k = v.pixel_size_at((pivot - v.eye).length()) * 78.0 * 0.9;
        let eye = v.eye - pivot;
        // the Y ring (in X Z): its front point between two axes
        let ring = |t: f32| pivot + (Vec3::Z * t.cos() + Vec3::X * t.sin()) * k;
        let t0 = [45f32, 135.0, 225.0, 315.0].into_iter().map(f32::to_radians).max_by(|a, b| (ring(*a) - pivot).dot(eye).total_cmp(&(ring(*b) - pivot).dot(eye))).unwrap();
        let (a, b) = (h.screen_of(ring(t0)), h.screen_of(ring(t0 + 33f32.to_radians())));
        h.move_to(a);
        h.steps(2);
        h.drag(a, b, 12, false);
        h.settle();
        h.shot("zero-qwer-2-rotating");
        eprintln!("  rotate readout {:?}", h.st.gizmo_readout);
        h.press(b, false);
        h.settle();
        h.shot("zero-qwer-3-rotated");
        eprintln!("  slot rot {:?}", h.st.doc.features[3].rot);
        // R on the boss: its Y cube pulled up half as much again
        h.st.select(Some(3), false);
        h.key(egui::Key::R);
        h.settle();
        h.shot("zero-qwer-4-scale");
        // the classic gizmo again (21 nov.): the Y cube pulled up half as far
        // again
        let pivot = h.st.pivot().unwrap().as_vec3();
        let v = h.st.view();
        let k = v.pixel_size_at((pivot - v.eye).length()) * 78.0;
        let (a, b) = (h.screen_of(pivot + Vec3::Y * k), h.screen_of(pivot + Vec3::Y * k * 1.5));
        h.move_to(a);
        h.steps(2);
        eprintln!("  hot {:?}", h.st.hot_handle);
        h.drag(a, b, 12, false);
        h.settle();
        h.shot("zero-qwer-5-scaling");
        eprintln!("  scale readout {:?}", h.st.gizmo_readout);
        h.press(b, false);
        h.settle();
        // the centre: dragged to the right, bigger all round
        let c = h.screen_of(h.st.pivot().unwrap().as_vec3());
        h.move_to(c);
        h.steps(2);
        h.drag(c, c + vec2(40.0, 0.0), 8, false);
        h.settle();
        eprintln!("  scale all readout {:?}", h.st.gizmo_readout);
        h.press(c + vec2(40.0, 0.0), false);
        h.settle();
        h.shot("zero-qwer-6-scaled");
        eprintln!("  boss {:?}", h.st.doc.features[2].prim);
        h.key(egui::Key::Q);
        h.steps(2);
        h.shot("zero-qwer-7-select");
        h.key(egui::Key::W);
        h.steps(2);
        h.shot("zero-qwer-8-move");
        let names: Vec<String> = h.st.undo_labels();
        eprintln!("  undo steps: {names:?}");
    }

    if want("fields") {
        // value fields: a middle-button drag across changes the value, a
        // click types one (3 decimals at most); the thin sliders
        h.reset(test_part(), "Test part");
        h.st.select(Some(3), false);
        h.steps(3);
        let f = pos2(270.0, 406.0);
        h.move_to(f);
        h.steps(2);
        h.mpress(f, true);
        for i in 1..=12 {
            h.move_to(f + vec2(5.0 * i as f32, 0.0));
        }
        h.steps(2);
        h.shot("zero-fields-1-middle-drag");
        h.mpress(f + vec2(60.0, 0.0), false);
        h.settle();
        eprintln!("  boss after middle drag {:?}", h.st.doc.features[2].prim);
        h.click(f);
        h.steps(2);
        for _ in 0..8 {
            h.key(egui::Key::Backspace);
        }
        h.type_text("8,12345");
        h.steps(1);
        h.shot("zero-fields-2-typing");
        h.key(egui::Key::Enter);
        h.settle();
        h.shot("zero-fields-3-typed");
        eprintln!("  boss after typing {:?}", h.st.doc.features[2].prim);
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
    }

    if want("materials") {
        // the materials island under the rail: a ball dragged onto a shape,
        // in Face mode onto a face, then a click on picked faces
        h.reset(test_part(), "Test part");
        h.st.select(Some(3), false);
        h.steps(3);
        let ball = |h: &Harness, i: usize| h.ctx.data(|d| d.get_temp::<Pos2>(egui::Id::new(("mat-ball", i)))).expect("ball drawn");
        let front = h.screen_of(Vec3::new(-12.0, 8.0, 15.0));
        h.move_to(ball(&h, 2));
        h.steps(3);
        h.shot("zero-materials-1-island");
        // Orange onto the Block
        h.drag(ball(&h, 2), front, 14, false);
        h.steps(3);
        h.shot("zero-materials-2-drag");
        h.press(front, false);
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(2);
        h.shot("zero-materials-3-dropped");
        eprintln!("  block material {} paint {:?}", h.st.doc.features[0].material, h.st.doc.paint);
        // Face mode: Steel onto the top face
        h.st.set_sel_mode(SelMode::Face);
        h.steps(2);
        let top = h.screen_of(Vec3::new(-14.0, 20.0, 10.0));
        h.drag(ball(&h, 4), top, 14, false);
        h.steps(3);
        h.shot("zero-materials-4-face-drag");
        h.press(top, false);
        h.settle();
        // two faces picked (the boss's side and the front), Green clicked
        let side = h.screen_of(Vec3::new(10.0 + 7.0 * 0.6, 25.0, 7.0 * 0.8));
        h.click(side);
        h.hold(Modifiers::SHIFT);
        h.click(front);
        h.hold(Modifiers::NONE);
        h.click(ball(&h, 1));
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(2);
        h.shot("zero-materials-5-faces");
        eprintln!("  paint {:?}", h.st.doc.paint);
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
    }

    if want("looks") {
        // the slots under the balls (Romain 20 nov., 15 h 42): a colour
        // picked, the specular up, a material made see-through
        h.reset(test_part(), "Test part");
        let ball = |h: &Harness, i: usize| h.ctx.data(|d| d.get_temp::<Pos2>(egui::Id::new(("mat-ball", i)))).expect("ball drawn");
        let slot = |h: &Harness, k: usize| h.ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("look-slot", k)))).expect("slot drawn");
        h.st.select(Some(1), false);
        h.steps(3);
        h.click(ball(&h, 2));
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(2);
        h.shot("zero-looks-1-slots");
        // the colour row opens the picker; a teal in the square, the hue bar
        h.click(slot(&h, 0).center());
        h.steps(3);
        let hb = slot(&h, 4);
        h.drag(hb.center(), pos2(hb.min.x + hb.width() * 0.53, hb.center().y), 6, true);
        h.steps(2);
        let sq = slot(&h, 3);
        h.drag(sq.center(), pos2(sq.min.x + sq.width() * 0.62, sq.min.y + sq.height() * 0.3), 6, true);
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(2);
        h.shot("zero-looks-2-color");
        eprintln!("  looks {:?}", h.st.doc.looks);
        // the picker closed, the specular to 100 %
        h.click(slot(&h, 0).center());
        h.steps(3);
        let sp = slot(&h, 1);
        h.drag(sp.center(), pos2(sp.max.x - 1.0, sp.center().y), 6, true);
        h.settle();
        // nothing selected, Orange for the new shapes: its slots stay shown
        h.st.select(None, false);
        h.steps(2);
        h.click(ball(&h, 2));
        h.move_to(pos2(800.0, 120.0));
        h.steps(2);
        h.shot("zero-looks-3-specular");
        // the boss in Steel, Steel at 65 % transparency
        h.st.select(Some(3), false);
        h.steps(3);
        h.click(ball(&h, 4));
        h.settle();
        let tr = slot(&h, 2);
        h.drag(tr.min + vec2(4.0, tr.height() / 2.0), pos2(tr.min.x + 2.0 + (tr.width() - 4.0) * 0.65, tr.center().y), 8, true);
        h.settle();
        h.st.select(None, false);
        h.steps(2);
        h.click(ball(&h, 4));
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-looks-4-transparent");
        eprintln!("  looks {:?}", h.st.doc.looks);
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
        // one undo of the document: the transparency back
        h.st.undo_doc();
        h.settle();
        eprintln!("  after undo {:?}", h.st.doc.looks);
    }

    if want("mirror") {
        // Mirror (Alt + M): half a part (a block from x = −5 to 35, a hole,
        // a boss), mirrored across X; then a fillet picked on the mirror
        // side, the boss moved, a slot carved on the far side: all made on
        // both sides; the window set to Z
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::new(15.0, 0.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(b).unwrap().name = "Block".into();
        let hole = d.add(Op::Carve, Prim::Cylinder { radius: 4.0, height: 30.0 }, DVec3::new(24.0, -5.0, -6.0), DQuat::IDENTITY);
        d.feature_mut(hole).unwrap().name = "Hole".into();
        let boss = d.add(Op::Add, Prim::Cylinder { radius: 5.0, height: 8.0 }, DVec3::new(14.0, 20.0, 5.0), DQuat::IDENTITY);
        d.feature_mut(boss).unwrap().name = "Boss".into();
        h.reset(d, "Mirror");
        h.st.camera.dist *= 1.35;
        h.st.camera.yaw -= 0.35;
        h.st.camera.pitch += 0.12;
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-mirror-1-half");
        h.hold(Modifiers::ALT);
        h.key(egui::Key::M);
        h.hold(Modifiers::NONE);
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-mirror-2-mirrored");
        eprintln!("  mirror: {:?}", h.st.selected_mirror().map(|f| (f.name.clone(), f.mirror)));
        // the far end's top front edge picked (the mirror side), filleted
        h.st.select(None, false);
        h.key(egui::Key::Num2);
        let e = h.screen_of(Vec3::new(-35.0, 20.0, 0.0));
        h.click(e);
        h.st.set_round(4.0, false, true);
        h.st.end_live();
        h.settle();
        h.key(egui::Key::Q);
        h.st.select(None, false);
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-mirror-3-fillet");
        // the boss moved (its own side): both move
        h.st.edit("Move", |d| d.feature_mut(boss).unwrap().origin.z = -4.0);
        // a slot carved on the far side, after the Mirror
        h.st.edit("Slot", |d| {
            let s = d.add(Op::Carve, Prim::Box { x: 6.0, y: 6.0, z: 40.0 }, DVec3::new(-26.0, 15.0, 0.0), DQuat::IDENTITY);
            d.feature_mut(s).unwrap().name = "Slot".into();
        });
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-mirror-4-live");
        // the Mirror selected again: across Z, keeping −Z
        let mid = h.st.doc.features.iter().find(|f| f.mirror.is_some()).map(|f| f.id).unwrap();
        h.st.selection = vec![mid];
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-mirror-5-selected");
        h.st.set_mirror(nurbs_kernel::Mirror { axis: 2, offset: 0.0, flip: true }, true);
        h.st.end_live();
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-mirror-6-z");
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
    }

    if want("array") {
        // Pattern / Array (Ctrl + Shift + D): a vent carved in a plate, its
        // rim rounded, picked and repeated: 4 in a line as it comes, 8 a
        // step of 10, a grid of 2 rows; the original made wider (the copies
        // follow), an inset sunk in a copy's floor (made in every one);
        // then a boss with its hole, 8 round a disc, then 5 over 180°
        let mut d = Doc::default();
        let plate = d.add(Op::Add, Prim::Box { x: 90.0, y: 8.0, z: 60.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(plate).unwrap().name = "Plate".into();
        let vent = d.add(Op::Carve, Prim::Profile { x: 5.0, y: 10.0, z: 16.0 }, DVec3::new(-36.0, 5.0, -12.0), DQuat::IDENTITY);
        d.feature_mut(vent).unwrap().name = "Vent".into();
        d.feature_mut(vent).unwrap().profile = Some(nurbs_kernel::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![2.5; 4], tension: vec![], open: false, spline: false });
        h.reset(d, "Array");
        h.st.camera.dist *= 1.2;
        h.st.camera.pitch += 0.25;
        h.settle();
        // the rim rounded (one edge: OpenCascade runs it round)
        h.key(egui::Key::Num2);
        h.click(h.screen_of(Vec3::new(-33.5, 8.0, -12.0)));
        h.st.set_round(0.8, false, true);
        h.st.end_live();
        h.settle();
        h.key(egui::Key::Num4);
        h.st.select(None, false);
        h.click(h.screen_of(Vec3::new(-36.0, 5.0, -12.0)));
        h.settle();
        eprintln!("  picked: {:?}", h.st.selection.iter().filter_map(|id| h.st.doc.feature(*id)).map(|f| f.name.clone()).collect::<Vec<_>>());
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-array-1-vent");
        h.hold(Modifiers::CTRL | Modifiers::SHIFT);
        h.key(egui::Key::D);
        h.hold(Modifiers::NONE);
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-array-2-four");
        let arr = |h: &Harness| h.st.selected_array().and_then(|f| f.array.clone()).unwrap();
        eprintln!("  array: {:?}", arr(&h));
        let a = arr(&h);
        h.st.set_array(nurbs_kernel::Array { count: 8, step: DVec3::new(10.0, 0.0, 0.0), ..a }, true);
        h.st.end_live();
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-array-3-line");
        let a = arr(&h);
        h.st.set_array(nurbs_kernel::Array { kind: nurbs_kernel::ArrayKind::Grid, count2: 2, step2: DVec3::new(0.0, 0.0, 24.0), ..a }, true);
        h.st.end_live();
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-array-4-grid");
        // the original wider: the copies follow
        h.st.edit("Width", |d| {
            let f = d.feature_mut(vent).unwrap();
            f.prim = Prim::Profile { x: 6.0, y: 10.0, z: 16.0 };
            f.profile.as_mut().unwrap().corner = vec![3.0; 4];
        });
        h.st.select(None, false);
        h.settle();
        // the floor of a copy (the second row's sixth) inset and sunk
        h.key(egui::Key::Num3);
        let floor = h.screen_of(Vec3::new(14.0, 5.0, 12.0));
        h.click(floor);
        h.settle();
        h.move_to(floor);
        h.key(egui::Key::I);
        h.settle();
        h.st.set_push(1.0, -1.5, true);
        h.st.end_live();
        h.settle();
        eprintln!("  order: {:?}", h.st.doc.features.iter().map(|f| f.name.clone()).collect::<Vec<_>>());
        h.key(egui::Key::Num4);
        h.st.select(None, false);
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-array-5-follow");
        eprintln!("  errors: {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
        // round a disc: a boss and its hole, 8 times
        let mut d = Doc::default();
        let disc = d.add(Op::Add, Prim::Cylinder { radius: 40.0, height: 8.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(disc).unwrap().name = "Disc".into();
        let boss = d.add(Op::Add, Prim::Cylinder { radius: 5.0, height: 6.0 }, DVec3::new(30.0, 8.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(boss).unwrap().name = "Boss".into();
        let hole = d.add(Op::Carve, Prim::Cylinder { radius: 2.0, height: 20.0 }, DVec3::new(30.0, -2.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(hole).unwrap().name = "Hole".into();
        h.reset(d, "Array round");
        h.st.camera.dist *= 1.15;
        h.st.camera.pitch += 0.2;
        h.settle();
        // the boss's foot rounded
        h.key(egui::Key::Num2);
        h.click(h.screen_of(Vec3::new(30.0, 8.0, 5.0)));
        h.st.set_round(1.5, false, true);
        h.st.end_live();
        h.settle();
        h.key(egui::Key::Num4);
        h.st.selection = vec![boss, hole];
        h.hold(Modifiers::CTRL | Modifiers::SHIFT);
        h.key(egui::Key::D);
        h.hold(Modifiers::NONE);
        h.settle();
        let a = arr(&h);
        h.st.set_array(nurbs_kernel::Array { kind: nurbs_kernel::ArrayKind::Circle, count: 8, ..a }, true);
        h.st.end_live();
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-array-6-circle");
        eprintln!("  circle: {:?}", arr(&h));
        let a = arr(&h);
        h.st.set_array(nurbs_kernel::Array { count: 5, angle: 180.0, ..a }, true);
        h.st.end_live();
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-array-7-half");
        eprintln!("  errors: {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
    }

    if want("shell") {
        // Shell (Romain 20 nov., 20 h 58: "Continue les shell"): a case with
        // rounded corners and a hole through it; its top face picked in Face
        // mode, Alt + S: hollowed with 2 mm walls, the top open; its dot
        // dragged to 4 mm; outwards; the front opened too (Pick faces); an
        // inner edge rounded after it; then a closed block hollowed (X-ray)
        // and asked too thick (it stops at the thickest that works)
        use nurbs_app::state::Handle;
        let mut d = Doc::default();
        let case = d.add(Op::Add, Prim::Box { x: 80.0, y: 30.0, z: 50.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(case).unwrap().name = "Case".into();
        let tg = |sub: u16| nurbs_kernel::model::FaceTag { feature: case, sub }.pack();
        let corners: Vec<nurbs_kernel::EdgeRef> = [(0u16, 4u16, 40.0, 25.0), (1, 4, -40.0, 25.0), (0, 5, 40.0, -25.0), (1, 5, -40.0, -25.0)].iter().map(|(a, b, x, z)| nurbs_kernel::EdgeRef { faces: [tg(*a), tg(*b)], at: DVec3::new(*x, 15.0, *z) }).collect();
        d.add_round(8.0, false, corners);
        let hole = d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 40.0 }, DVec3::new(24.0, -5.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(hole).unwrap().name = "Hole".into();
        h.reset(d, "Shell");
        h.st.camera.dist *= 1.3;
        h.st.camera.pitch += 0.3;
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-shell-1-part");
        // the top face picked
        h.key(egui::Key::Num3);
        h.click(h.screen_of(Vec3::new(-12.0, 30.0, 6.0)));
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-shell-2-face");
        h.hold(Modifiers::ALT);
        h.key(egui::Key::S);
        h.hold(Modifiers::NONE);
        h.settle();
        let shell = |h: &Harness| h.st.selected_shell().and_then(|f| f.shell.clone());
        eprintln!("  shell: {:?} errors {:?}", shell(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-shell-3-open");
        // its dot hovered, then dragged to 4 mm
        let g = h.st.shell_grip().expect("grip");
        let p = h.screen_of(g.at.as_vec3());
        h.move_to(p);
        h.steps(3);
        eprintln!("  hot: {:?}", h.st.hot_handle == Some(Handle::ShellThick));
        h.shot("zero-shell-4-hover");
        let q = h.screen_of((g.from + g.dir * 4.0).as_vec3());
        h.drag(p, q, 10, false);
        h.settle();
        h.shot("zero-shell-5-four");
        h.press(q, false);
        h.settle();
        eprintln!("  thickness: {:?}", shell(&h).map(|s| s.thickness));
        // outwards
        let s0 = shell(&h).unwrap();
        h.st.set_shell(nurbs_kernel::Shell { outward: true, ..s0.clone() }, true);
        h.st.end_live();
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-shell-6-outward");
        h.st.set_shell(nurbs_kernel::Shell { outward: false, thickness: 2.0, ..s0 }, true);
        h.st.end_live();
        h.settle();
        // Pick faces: the front opened too, the left side hovered
        h.st.shell_pick_toggle();
        h.click(h.screen_of(Vec3::new(-14.0, 14.0, 25.0)));
        h.settle();
        h.move_to(h.screen_of(Vec3::new(40.0, 15.0, -6.0)));
        h.steps(3);
        eprintln!("  open: {:?}", shell(&h).map(|s| s.faces.len()));
        h.shot("zero-shell-7-pick");
        h.key(egui::Key::Escape);
        h.settle();
        // an inner edge rounded after it (the floor along the back wall)
        h.key(egui::Key::Num2);
        h.click(h.screen_of(Vec3::new(-6.0, 2.0, -23.0)));
        h.st.set_round(3.0, false, true);
        h.st.end_live();
        h.settle();
        h.key(egui::Key::Num4);
        h.st.select(None, false);
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        eprintln!("  errors: {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-shell-8-inner-fillet");
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
        // a closed block: a cavity inside, seen in X-ray
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 60.0, y: 30.0, z: 40.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(b).unwrap().name = "Block".into();
        h.reset(d, "Shell closed");
        h.st.camera.dist *= 1.3;
        h.settle();
        h.key(egui::Key::Num4);
        h.click(h.screen_of(Vec3::new(0.0, 30.0, 0.0)));
        h.settle();
        h.hold(Modifiers::ALT);
        h.key(egui::Key::S);
        h.key(egui::Key::X);
        h.hold(Modifiers::NONE);
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        eprintln!("  closed: {:?} volume-ish errors {:?}", shell(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-shell-9-closed-xray");
        // asked 30 mm: the block is 30 high, it stops at the thickest
        let s0 = shell(&h).unwrap();
        h.st.set_shell(nurbs_kernel::Shell { thickness: 30.0, ..s0 }, true);
        h.st.end_live();
        h.settle();
        h.steps(10);
        h.settle();
        eprintln!("  limit: {:?} toast {:?}", shell(&h).map(|s| s.thickness), h.st.toast.as_ref().map(|t| t.text.clone()));
        h.shot("zero-shell-10-limit");
    }

    if want("array-grips") {
        // the Array's grips in the view (Romain 20 nov., 19 h 01: "des
        // poignées pour manipuler directement l'influence des array"): the
        // vent's line, its first copy dragged (the step), its arrow pulled
        // (more copies), a grid's rows pulled; the operation window stays
        // where it was put while the view turns; round a disc, the arrow
        // turned back to 180° and the first copy brought further (fewer)
        use nurbs_app::state::Handle;
        let mut d = Doc::default();
        let plate = d.add(Op::Add, Prim::Box { x: 90.0, y: 8.0, z: 60.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(plate).unwrap().name = "Plate".into();
        let vent = d.add(Op::Carve, Prim::Profile { x: 5.0, y: 10.0, z: 16.0 }, DVec3::new(-36.0, 5.0, -12.0), DQuat::IDENTITY);
        d.feature_mut(vent).unwrap().name = "Vent".into();
        d.feature_mut(vent).unwrap().profile = Some(nurbs_kernel::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![2.5; 4], tension: vec![], open: false, spline: false });
        h.reset(d, "Array grips");
        h.st.camera.dist *= 1.2;
        h.st.camera.pitch += 0.25;
        h.settle();
        h.key(egui::Key::Num2);
        h.click(h.screen_of(Vec3::new(-33.5, 8.0, -12.0)));
        h.st.set_round(0.8, false, true);
        h.st.end_live();
        h.settle();
        h.key(egui::Key::Num4);
        h.st.select(None, false);
        h.click(h.screen_of(Vec3::new(-36.0, 5.0, -12.0)));
        h.settle();
        h.hold(Modifiers::CTRL | Modifiers::SHIFT);
        h.key(egui::Key::D);
        h.hold(Modifiers::NONE);
        h.settle();
        let arr = |h: &Harness| h.st.selected_array().and_then(|f| f.array.clone()).unwrap();
        // where a grip is on screen (an arrow: three quarters along it)
        let grip = |h: &Harness, which: Handle| -> Pos2 {
            let g = h.st.array_grips().into_iter().find(|g| g.handle == which).expect("grip");
            match g.from {
                None => h.screen_of(g.at.as_vec3()),
                Some(f) => {
                    let (a, b) = (h.screen_of(f.as_vec3()), h.screen_of(g.at.as_vec3()));
                    a + (b - a) * 0.75
                }
            }
        };
        let origin = |h: &Harness| h.st.array_marks().unwrap().1[0];
        eprintln!("  array: {:?}", arr(&h));
        let p = grip(&h, Handle::ArrayStep(0));
        h.move_to(p);
        h.steps(3);
        h.shot("zero-array-grips-1-hover");
        // the first copy dragged out to 11 mm from the original
        let o = origin(&h);
        let q = h.screen_of((o + DVec3::X * 11.0).as_vec3());
        h.drag(p, q, 10, false);
        h.settle();
        h.shot("zero-array-grips-2-step");
        h.press(q, false);
        h.settle();
        eprintln!("  step: {:?}", arr(&h).step);
        // the arrow pulled out by three steps: 3 more
        let p = grip(&h, Handle::ArrayCount(0));
        let a = arr(&h);
        let q = p + (h.screen_of((o + a.step * 3.0).as_vec3()) - h.screen_of(o.as_vec3()));
        h.drag(p, q, 12, false);
        h.settle();
        h.shot("zero-array-grips-3-count");
        h.press(q, false);
        h.settle();
        eprintln!("  count: {}", arr(&h).count);
        // a grid: its rows' arrow pulled by one row
        let a = arr(&h);
        h.st.set_array(nurbs_kernel::Array { kind: nurbs_kernel::ArrayKind::Grid, count2: 2, step2: DVec3::new(0.0, 0.0, 20.0), ..a }, true);
        h.st.end_live();
        h.settle();
        let p = grip(&h, Handle::ArrayCount(1));
        let q = p + (h.screen_of((o + DVec3::Z * 20.0).as_vec3()) - h.screen_of(o.as_vec3()));
        h.drag(p, q, 10, false);
        h.settle();
        h.shot("zero-array-grips-4-rows");
        h.press(q, false);
        h.settle();
        eprintln!("  rows: {}", arr(&h).count2);
        // the view turned: the window stays where it was put
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        let before = h.ctx.memory(|m| m.area_rect(egui::Id::new("op-pop")));
        h.shot("zero-array-grips-5-window");
        h.st.camera.yaw += 0.6;
        h.st.camera.dist *= 0.85;
        h.settle();
        h.steps(3);
        let after = h.ctx.memory(|m| m.area_rect(egui::Id::new("op-pop")));
        h.shot("zero-array-grips-6-turned");
        eprintln!("  window before {:?} after {:?}", before.map(|r| r.min), after.map(|r| r.min));
        // round a disc: 8, the arrow turned back to 180°, then the first
        // copy brought further round (fewer copies)
        let mut d = Doc::default();
        let disc = d.add(Op::Add, Prim::Cylinder { radius: 40.0, height: 8.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(disc).unwrap().name = "Disc".into();
        let boss = d.add(Op::Add, Prim::Cylinder { radius: 5.0, height: 6.0 }, DVec3::new(30.0, 8.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(boss).unwrap().name = "Boss".into();
        h.reset(d, "Array grips round");
        h.st.camera.dist *= 1.15;
        h.st.camera.pitch += 0.3;
        h.settle();
        h.st.selection = vec![boss];
        h.hold(Modifiers::CTRL | Modifiers::SHIFT);
        h.key(egui::Key::D);
        h.hold(Modifiers::NONE);
        h.settle();
        let a = arr(&h);
        h.st.set_array(nurbs_kernel::Array { kind: nurbs_kernel::ArrayKind::Circle, count: 8, ..a }, true);
        h.st.end_live();
        h.settle();
        let p = grip(&h, Handle::ArrayAngle);
        h.move_to(p);
        h.steps(3);
        h.shot("zero-array-grips-7-round");
        // along the arc, from the end (360°) back to 180°
        let a = arr(&h);
        let o = origin(&h);
        let c = DVec3::new(a.centre.x, o.y, a.centre.z);
        let r0 = o - c;
        h.press(p, true);
        for i in 1..=12 {
            let ang = (360.0 - 15.0 * i as f64).to_radians();
            let w = c + DQuat::from_axis_angle(DVec3::Y, ang) * r0 * 1.15;
            h.move_to(h.screen_of(w.as_vec3()));
        }
        h.settle();
        h.shot("zero-array-grips-8-angle");
        let w = c + DQuat::from_axis_angle(DVec3::Y, 180f64.to_radians()) * r0 * 1.15;
        h.press(h.screen_of(w.as_vec3()), false);
        h.settle();
        eprintln!("  angle: {} count {}", arr(&h).angle, arr(&h).count);
        // the first copy taken to 45°: 180 / 45 + 1 = 5
        let p = grip(&h, Handle::ArrayCount(0));
        let a0 = arr(&h).step_angle();
        h.press(p, true);
        for i in 1..=8 {
            let ang = (a0 + (45.0 - a0) * i as f64 / 8.0).to_radians();
            h.move_to(h.screen_of((c + DQuat::from_axis_angle(DVec3::Y, ang) * r0).as_vec3()));
        }
        h.settle();
        h.shot("zero-array-grips-9-fewer");
        let w = c + DQuat::from_axis_angle(DVec3::Y, 45f64.to_radians()) * r0;
        h.press(h.screen_of(w.as_vec3()), false);
        h.settle();
        eprintln!("  angle: {} count {}", arr(&h).angle, arr(&h).count);
        eprintln!("  errors: {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
    }

    if want("align-window") {
        // Align (3ds Max's, Alt + A): a bracket turned 30° to the right of
        // the block, picked, Align, the block clicked; centre on centre,
        // then on the ground along Y with the block's orientation (Apply),
        // its Min against the block's Max along X (Apply), centred along Z
        // (OK)
        let mut d = test_part();
        let b = d.add(Op::Add, Prim::Box { x: 12.0, y: 8.0, z: 10.0 }, DVec3::new(48.0, 6.0, -26.0), DQuat::from_rotation_y(30f64.to_radians()));
        d.feature_mut(b).unwrap().name = "Bracket".into();
        h.reset(d, "Test part");
        h.st.camera.dist *= 1.35;
        h.settle();
        let rect = |h: &Harness, k: &str, i: usize| h.ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new((k, i)))).expect("drawn");
        h.click(h.screen_of(Vec3::new(48.0, 14.0, -26.0)));
        h.settle();
        h.st.begin_align();
        let block = h.screen_of(Vec3::new(-12.0, 8.0, 15.0));
        h.move_to(block);
        h.steps(3);
        h.shot("zero-alignwin-1-pick");
        h.click(block);
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-alignwin-2-center");
        // first square with the block and on the ground: Y only, Min on
        // Min, the block's orientation (Apply)
        h.click(rect(&h, "align-axis", 0).center());
        h.click(rect(&h, "align-axis", 2).center());
        if let Some(a) = h.st.align.as_mut() {
            a.cur = nurbs_app::align::AlignPoint::Min;
            a.tgt = nurbs_app::align::AlignPoint::Min;
            a.orient = true;
        }
        h.st.align_update();
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-alignwin-3-orient");
        h.click(rect(&h, "align-btn", 1).center());
        h.settle();
        // X: the bracket's Min on the block's Max (Apply)
        h.click(rect(&h, "align-axis", 0).center());
        if let Some(a) = h.st.align.as_mut() {
            a.cur = nurbs_app::align::AlignPoint::Min;
            a.tgt = nurbs_app::align::AlignPoint::Max;
        }
        h.st.align_update();
        h.settle();
        h.click(rect(&h, "align-btn", 1).center());
        h.settle();
        // Z: centre on centre (OK)
        h.click(rect(&h, "align-axis", 2).center());
        if let Some(a) = h.st.align.as_mut() {
            a.cur = nurbs_app::align::AlignPoint::Center;
            a.tgt = nurbs_app::align::AlignPoint::Center;
        }
        h.st.align_update();
        h.settle();
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-alignwin-4-x-z");
        h.click(rect(&h, "align-btn", 2).center());
        h.settle();
        h.st.select(None, false);
        h.move_to(pos2(800.0, 120.0));
        h.steps(3);
        h.shot("zero-alignwin-5-done");
        let f = h.st.doc.feature(b).unwrap();
        eprintln!("  bracket origin {:?} rot {:?}", f.origin, f.rot.to_euler(glam::EulerRot::YXZ));
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
        // Cancel puts it back
        h.st.selection = vec![b];
        h.st.begin_align();
        h.click(block);
        h.settle();
        let moved = h.st.doc.feature(b).unwrap().origin;
        h.click(rect(&h, "align-btn", 0).center());
        h.settle();
        eprintln!("  cancel: moved to {:?}, back to {:?}", moved, h.st.doc.feature(b).unwrap().origin);
    }

    if want("marking") {
        // the marking menu (right button): on a shape, its material arc, a
        // quick flick, on a picked edge, in empty space
        h.reset(test_part(), "Test part");
        let boss = h.screen_of(Vec3::new(10.0, 30.0, 0.0));
        h.rpress(boss, true);
        h.steps(15);
        h.move_to(boss + vec2(70.0, 0.0));
        h.steps(2);
        h.shot("zero-marking-1-shape");
        // up: the material arc, then the third ball (Orange)
        h.move_to(boss + vec2(0.0, -60.0));
        h.steps(12);
        let ball = boss + vec2((-0.17f32).sin() * 150.0, -(-0.17f32).cos() * 150.0 * 0.86);
        h.move_to(ball);
        h.steps(3);
        h.shot("zero-marking-2-materials");
        h.rpress(ball, false);
        h.settle();
        h.shot("zero-marking-3-orange");
        eprintln!("  boss material {}", h.st.doc.features[2].material);
        // a quick flick to the right: Make it Carve, at once
        h.rpress(boss, true);
        h.move_to(boss + vec2(40.0, 0.0));
        h.move_to(boss + vec2(80.0, 0.0));
        h.rpress(boss + vec2(80.0, 0.0), false);
        h.steps(3);
        h.shot("zero-marking-4-flick");
        h.settle();
        eprintln!("  boss op {:?}", h.st.doc.features[2].op);
        h.st.undo_doc();
        h.settle();
        // Edge mode: on an edge, Fillet
        h.key(egui::Key::F10);
        let e = h.screen_of(Vec3::new(-12.0, 20.0, 15.0));
        h.rpress(e, true);
        h.steps(15);
        h.move_to(e + vec2(0.0, -70.0));
        h.steps(2);
        h.shot("zero-marking-5-edge");
        h.rpress(e + vec2(0.0, -70.0), false);
        h.settle();
        h.shot("zero-marking-6-fillet");
        // empty space: new shapes, Frame all, Snap
        h.key(egui::Key::F8);
        let empty = pos2(1150.0, 300.0);
        h.rpress(empty, true);
        h.steps(15);
        eprintln!("  empty at {empty:?}: open {} ctx {:?}", h.st.marking.is_open(), h.st.marking.menu.as_ref().map(|m| m.ctx));
        h.move_to(empty + vec2(60.0, 0.0));
        h.steps(2);
        h.shot("zero-marking-7-empty");
        h.key(egui::Key::Escape);
        h.rpress(empty + vec2(60.0, 0.0), false);
        h.steps(2);
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
    }

    if want("extrude-curved") {
        // Extrude on a curved face: the Boss's side pulled out along its
        // normals (a thicker boss), the Hole's wall pushed in (a bigger
        // hole), a fillet's face pulled out
        h.reset(test_part(), "Test part");
        h.key(egui::Key::Num3);
        h.st.camera.frame(Vec3::new(-20.0, 0.0, -15.0), Vec3::new(20.0, 30.0, 15.0));
        h.steps(2);
        h.st.arm_push(PushKind::Extrude, None);
        let side = Vec3::new(10.0, 25.0, 7.0);
        let a = h.screen_of(side);
        let b = h.screen_of(side + Vec3::Z * 4.0);
        h.move_to(a);
        h.steps(2);
        h.drag(a, b, 8, false);
        h.settle();
        h.shot("zero-extrude-curved-1-drag");
        h.press(b, false);
        h.steps(2);
        h.settle();
        let ex = |h: &Harness| h.st.doc.features.iter().filter_map(|f| f.push.as_ref().map(|p| (f.name.clone(), p.inset, p.depth))).collect::<Vec<_>>();
        eprintln!("  extrusions {:?} errors {:?}", ex(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.move_to(h.screen_of(Vec3::new(0.0, -10.0, 40.0)));
        h.steps(2);
        h.shot("zero-extrude-curved-2-out");
        // the Hole's wall pushed in 2 mm (into the block): a bigger hole
        let hole = h.st.doc.features.iter().find(|f| f.name == "Hole").map(|f| f.id).unwrap();
        let mut q = 0;
        h.st.edit("Extrude", |d| q = d.add_push(nurbs_kernel::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: hole, sub: 0 }.pack(), at: DVec3::new(-10.0, 10.0, 5.0) }, inset: 0.0, depth: -2.0 }));
        h.st.selection = vec![q];
        h.settle();
        eprintln!("  hole wall pushed {:?} errors {:?}", ex(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-extrude-curved-3-hole");
        // a fillet on the block's front top edge, its face pulled out 1.5 mm
        let e = {
            let m = h.st.model.as_ref().unwrap();
            let p = Vec3::new(14.0, 20.0, 15.0);
            let near = |i: usize| m.edges[i].pts.windows(2).map(|w| {
                let (u, v) = (Vec3::from_array(w[0]), Vec3::from_array(w[1]));
                let d = v - u;
                let t = ((p - u).dot(d) / d.length_squared().max(1e-12)).clamp(0.0, 1.0);
                (u + d * t).distance(p)
            }).fold(f32::MAX, f32::min);
            (0..m.edges.len()).min_by(|a, b| near(*a).total_cmp(&near(*b))).and_then(|i| h.st.edge_ref(i))
        };
        let mut r = 0;
        h.st.edit("Fillet", |d| r = d.add_round(4.0, false, e.into_iter().collect()));
        h.settle();
        let at = h.st.model.as_ref().unwrap().faces.iter().find(|f| f.tag.map(|t| t.feature) == Some(r)).map(|f| DVec3::from_array(f.point)).unwrap();
        let mut p = 0;
        h.st.edit("Extrude", |d| p = d.add_push(nurbs_kernel::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: r, sub: 0 }.pack(), at }, inset: 0.0, depth: 1.5 }));
        h.st.selection = vec![p];
        h.settle();
        eprintln!("  fillet face pulled {:?} errors {:?}", ex(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-extrude-curved-4-fillet-face");
    }
    if want("tension-box") {
        // a block with its 12 edges filleted (8 sphere corners), then the
        // fillet's tension: fuller, flatter; the corners remade to fit
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Rounded box");
        h.settle();
        let mut mids = vec![];
        for (a, b) in [(20.0, 15.0), (20.0, -15.0), (-20.0, 15.0), (-20.0, -15.0)] {
            mids.push(Vec3::new(a, 10.0, b));
        }
        for (y, z) in [(0.0, 15.0), (0.0, -15.0), (20.0, 15.0), (20.0, -15.0)] {
            mids.push(Vec3::new(0.0, y, z));
        }
        for (x, y) in [(20.0, 0.0), (-20.0, 0.0), (20.0, 20.0), (-20.0, 20.0)] {
            mids.push(Vec3::new(x, y, 0.0));
        }
        let edges: Vec<EdgeRef> = {
            let m = h.st.model.as_ref().unwrap();
            mids.iter()
                .filter_map(|p| {
                    // the distance to the edge's polyline
                    let near = |i: usize| {
                        let pts: Vec<Vec3> = m.edges[i].pts.iter().map(|q| Vec3::from_array(*q)).collect();
                        pts.windows(2)
                            .map(|w| {
                                let d = w[1] - w[0];
                                let t = ((*p - w[0]).dot(d) / d.length_squared().max(1e-12)).clamp(0.0, 1.0);
                                (w[0] + d * t).distance(*p)
                            })
                            .fold(f32::MAX, f32::min)
                    };
                    let i = (0..m.edges.len()).min_by(|a, b| near(*a).total_cmp(&near(*b)))?;
                    h.st.edge_ref(i)
                })
                .collect()
        };
        let mut id = 0;
        h.st.edit("Fillet", |d| id = d.add_round(4.0, false, edges));
        h.settle();
        h.st.select(Some(id), false);
        h.st.camera.frame(Vec3::new(-20.0, 0.0, -15.0), Vec3::new(20.0, 20.0, 15.0));
        h.steps(2);
        h.settle();
        h.shot("zero-tension-box-1-circular");
        for (t, name) in [(0.75, "2-fuller"), (0.25, "3-flatter")] {
            h.st.set_round_tension(id, t, true);
            h.st.end_live();
            h.settle();
            eprintln!("  tension {t}: errors {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
            h.shot(&format!("zero-tension-box-{name}"));
        }
        // a corner seen close, with its edges
        h.st.select(None, false);
        h.st.camera.frame(Vec3::new(12.0, 12.0, 7.0), Vec3::new(20.0, 20.0, 15.0));
        h.steps(2);
        h.settle();
        h.shot("zero-tension-box-4-corner-flatter");
        h.st.set_round_tension(id, 0.75, true);
        h.st.end_live();
        h.settle();
        h.shot("zero-tension-box-5-corner-fuller");
    }
    if want("tension-two") || want("tension-small") {
        // two fillet sizes on a block: the top edges at 3 mm, then the
        // vertical ones at 5 mm (their corners run round under the top
        // one), circular, then each with its tension; then the other order.
        // tension-small: the vertical ones at 3 mm under top ones at 5 mm
        // (OpenCascade's corner between them follows)
        let refs = |h: &Harness, mids: &[Vec3]| -> Vec<EdgeRef> {
            let m = h.st.model.as_ref().unwrap();
            mids.iter()
                .filter_map(|p| {
                    let near = |i: usize| {
                        let pts: Vec<Vec3> = m.edges[i].pts.iter().map(|q| Vec3::from_array(*q)).collect();
                        pts.windows(2)
                            .map(|w| {
                                let d = w[1] - w[0];
                                let t = ((*p - w[0]).dot(d) / d.length_squared().max(1e-12)).clamp(0.0, 1.0);
                                (w[0] + d * t).distance(*p)
                            })
                            .fold(f32::MAX, f32::min)
                    };
                    let i = (0..m.edges.len()).min_by(|a, b| near(*a).total_cmp(&near(*b)))?;
                    h.st.edge_ref(i)
                })
                .collect()
        };
        let vert = [Vec3::new(20.0, 10.0, 15.0), Vec3::new(-20.0, 10.0, 15.0), Vec3::new(20.0, 10.0, -15.0), Vec3::new(-20.0, 10.0, -15.0)];
        let top = [Vec3::new(0.0, 20.0, 15.0), Vec3::new(0.0, 20.0, -15.0), Vec3::new(20.0, 20.0, 0.0), Vec3::new(-20.0, 20.0, 0.0)];
        let runs = [("two", 5.0, 3.0, "top-first", false, 0.5), ("two", 5.0, 3.0, "vertical-first", true, 0.5), ("two", 5.0, 3.0, "top-first-too-full", false, 0.7), ("two", 5.0, 3.0, "vertical-first-too-full", true, 0.7), ("small", 3.0, 5.0, "top-first", false, 0.5), ("small", 3.0, 5.0, "vertical-first", true, 0.5)];
        for (set, rv, rt, order, vfirst, tvert) in runs {
            if !want(&format!("tension-{set}")) {
                continue;
            }
            let mut d = Doc::default();
            d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            h.reset(d, "Two fillets");
            h.settle();
            let (m1, r1, m2, r2) = if vfirst { (&vert, rv, &top, rt) } else { (&top, rt, &vert, rv) };
            let e1 = refs(&h, m1);
            let mut a = 0;
            h.st.edit("Fillet", |d| a = d.add_round(r1, false, e1));
            h.settle();
            let e2 = refs(&h, m2);
            let mut b = 0;
            h.st.edit("Fillet", |d| b = d.add_round(r2, false, e2));
            h.settle();
            let (tv, tt) = if vfirst { (a, b) } else { (b, a) };
            h.st.select(Some(b), false);
            h.st.camera.frame(Vec3::new(-20.0, 0.0, -15.0), Vec3::new(20.0, 20.0, 15.0));
            h.steps(2);
            h.settle();
            if order == "top-first" {
                h.shot(&format!("zero-tension-{set}-1-circular"));
                if set == "small" {
                    h.st.select(None, false);
                    h.st.camera.frame(Vec3::new(10.0, 10.0, 5.0), Vec3::new(20.0, 20.0, 15.0));
                    h.steps(2);
                    h.settle();
                    h.shot("zero-tension-small-1-circular-corner");
                    h.st.select(Some(b), false);
                    h.st.camera.frame(Vec3::new(-20.0, 0.0, -15.0), Vec3::new(20.0, 20.0, 15.0));
                    h.steps(2);
                    h.settle();
                }
            }
            h.st.set_round_tension(tt, 0.7, true);
            h.st.end_live();
            h.settle();
            h.st.set_round_tension(tv, tvert, true);
            h.st.end_live();
            h.settle();
            eprintln!("  {order}: errors {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
            h.shot(&format!("zero-tension-{set}-2-{order}"));
            h.st.select(None, false);
            h.st.camera.frame(Vec3::new(10.0, 10.0, 5.0), Vec3::new(20.0, 20.0, 15.0));
            h.steps(2);
            h.settle();
            h.shot(&format!("zero-tension-{set}-3-{order}-corner"));
            if tvert > 0.6 && set == "two" {
                // seen from above: the top fillet's edge round the corner
                h.st.camera.preset(nurbs_app::camera::ViewPreset::Top);
                h.st.camera.frame(Vec3::new(8.0, 18.0, 3.0), Vec3::new(20.0, 20.0, 15.0));
                h.steps(2);
                h.settle();
                h.shot(&format!("zero-tension-two-4-{order}-above"));
            }
        }
    }
    if want("reference") {
        // retopology on a mesh, step 1: the pistol (an AI-made mesh, 1.95 M
        // triangles, in cm) shown as the reference, solid then seen through
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        if std::path::Path::new(&path).exists() {
            h.reset(Doc::default(), "Retopo");
            h.st.import_reference(&path);
            h.step();
            let end = Instant::now() + Duration::from_secs(180);
            while h.st.ref_busy() && Instant::now() < end {
                h.step();
                std::thread::sleep(Duration::from_millis(20));
            }
            h.steps(2);
            if let Some(r) = &h.st.reference {
                eprintln!("  reference {}: {} triangles, {:.1} s, box {:?} {:?}", r.name, r.triangles(), r.secs, r.lo, r.hi);
            }
            h.shot("zero-ref-0-loaded");
            // the pointer's ray against it (the BVH)
            let c = h.st.principal();
            let t0 = Instant::now();
            let hit = h.st.reference_at(c);
            eprintln!("  ray from the view's centre: {hit:?} in {:?}", t0.elapsed());
            h.st.toast = None;
            // a block and a hole in the grip, seen through the ghost
            let mut d = Doc::default();
            d.reference = h.st.doc.reference.clone();
            let b = d.add(Op::Add, Prim::Box { x: 30.0, y: 40.0, z: 20.0 }, DVec3::new(-20.0, 20.0, 0.0), DQuat::IDENTITY);
            d.feature_mut(b).unwrap().name = "Block".into();
            h.st.edit("Part", |doc| *doc = d);
            h.settle();
            h.shot("zero-ref-1-solid");
            h.st.ref_ghost = true;
            h.steps(2);
            h.shot("zero-ref-2-ghost");
            h.st.select(None, false);
            h.st.ref_ghost = false;
            h.st.ref_shown = false;
            h.steps(2);
            h.shot("zero-ref-3-hidden");
            h.st.ref_shown = true;
        } else {
            eprintln!("reference: {path} not found (FORME_REF)");
        }
    }

    if want("scan") {
        // retopology, step 2: the Cylinder dragged across the muzzle's
        // holes on the scan reads each one (centre, Ø, axis, depth) and
        // makes a Carve cylinder of it; a block stands for the muzzle
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        if std::path::Path::new(&path).exists() {
            let mut d = Doc::default();
            d.reference = Some(nurbs_kernel::RefMesh { path: path.clone() });
            let b = d.add(Op::Add, Prim::Box { x: 30.0, y: 46.0, z: 26.0 }, DVec3::new(44.5, 55.0, 2.0), DQuat::IDENTITY);
            d.feature_mut(b).unwrap().name = "Muzzle".into();
            h.reset(d, "Retopo");
            h.step();
            let end = Instant::now() + Duration::from_secs(180);
            while h.st.ref_busy() && Instant::now() < end {
                h.step();
                std::thread::sleep(Duration::from_millis(20));
            }
            h.settle();
            h.st.toast = None;
            h.st.select(None, false);
            h.st.camera.yaw = 1.05;
            h.st.camera.pitch = 0.22;
            h.st.camera.frame(Vec3::new(40.0, 52.0, -14.0), Vec3::new(60.0, 104.0, 18.0));
            h.steps(2);
            let refm = h.st.reference.clone().expect("reference");
            let rim = |y: f64, z: f64| -> Vec3 {
                refm.hit(DVec3::new(100.0, y, z), DVec3::new(-1.0, 0.0, 0.0)).map(|q| q.0.as_vec3()).unwrap_or(Vec3::new(59.0, y as f32, z as f32))
            };
            h.st.tool = Tool::Create(PrimKind::Cylinder);
            // from the hole's middle outwards: the first point on the face
            // round it, a little past
            let out = |y: f64, z: f64, way: f64| -> f64 {
                let mut q = z + way * 4.0;
                while (q - z).abs() < 12.0 && refm.hit(DVec3::new(100.0, y, q), DVec3::new(-1.0, 0.0, 0.0)).map(|p| p.0.x < 58.5).unwrap_or(true) {
                    q += way * 0.25;
                }
                q + way * 0.75
            };
            for (k, (y, z)) in [(88.5, 0.2), (68.0, -0.3)].into_iter().enumerate() {
                let (a, bq) = (h.screen_of(rim(y, out(y, z, -1.0))), h.screen_of(rim(y, out(y, z, 1.0))));
                h.move_to(a);
                h.steps(2);
                if k == 0 {
                    h.shot("zero-scan-1-armed");
                }
                h.drag(a, bq, 10, false);
                h.steps(2);
                if k == 0 {
                    h.shot("zero-scan-2-drag");
                }
                h.press(bq, false);
                h.step();
                h.settle();
                h.shot(&format!("zero-scan-{}-read", 3 + k));
            }
            h.st.tool = Tool::Select;
            h.st.select(None, false);
            h.st.toast = None;
            h.st.ref_ghost = true;
            h.steps(2);
            h.shot("zero-scan-5-ghost");
            h.st.ref_shown = false;
            h.steps(2);
            h.shot("zero-scan-6-part");
            h.st.ref_shown = true;
            h.st.ref_ghost = false;
            // bosses on the side: the round button, the knob behind it
            h.st.camera.yaw = 0.35;
            h.st.camera.pitch = 0.25;
            h.st.camera.frame(Vec3::new(-46.0, 70.0, 5.0), Vec3::new(-14.0, 90.0, 18.0));
            h.steps(2);
            let side = |x: f64, y: f64| -> Vec3 { refm.hit(DVec3::new(x, y, 100.0), DVec3::new(0.0, 0.0, -1.0)).map(|q| q.0.as_vec3()).unwrap_or(Vec3::new(x as f32, y as f32, 12.0)) };
            h.st.tool = Tool::Create(PrimKind::Cylinder);
            // two rivets, found on screen
            let rivets: Vec<(f64, f64)> = [[630.0, 300.0], [535.0, 412.0]].iter().filter_map(|p| h.st.reference_at(*p)).map(|(q, _)| (q.x, q.y)).collect();
            eprintln!("  rivets at {rivets:.2?}");
            let mut drags = vec![(-24.25, 80.45, 3.9), (-24.25, 80.45, 5.6)];
            drags.extend(rivets.iter().map(|&(x, y)| (x, y, 1.6)));
            for (k, (x, y, half)) in drags.into_iter().enumerate() {
                let (a, bq) = (h.screen_of(side(x - half, y)), h.screen_of(side(x + half, y)));
                h.move_to(a);
                h.steps(2);
                h.drag(a, bq, 10, false);
                h.steps(2);
                if k == 0 {
                    h.shot("zero-scan-7-boss-drag");
                }
                h.press(bq, false);
                h.step();
                h.settle();
                h.shot(&format!("zero-scan-{}-boss", 8 + k));
                if k == 0 {
                    // undone: the outer step next, at the same place
                    h.st.undo_doc();
                    h.settle();
                }
            }
            h.st.tool = Tool::Select;
            h.st.select(None, false);
            h.st.toast = None;
            h.st.ref_ghost = true;
            h.steps(2);
            h.shot("zero-scan-12-boss-ghost");
            // step 3: the part coloured by its distance to the scan
            h.st.ref_ghost = false;
            h.st.set_deviation(true);
            let end = Instant::now() + Duration::from_secs(60);
            h.step();
            while h.st.dev_busy() && Instant::now() < end {
                h.step();
                std::thread::sleep(Duration::from_millis(10));
            }
            h.steps(2);
            if let Some(d) = &h.st.deviation {
                eprintln!("  deviation: {} points near, {} within {} mm, median {:.3}, 95 % {:.3} mm, {} triangles, {:.0} ms", d.near, d.within, d.tol, d.p50, d.p95, d.verts.len() / 3, d.ms);
            }
            h.shot("zero-scan-13-dev-bosses");
            h.st.ref_shown = false;
            h.steps(2);
            h.shot("zero-scan-14-dev-bosses-alone");
            h.st.ref_shown = true;
            h.st.camera.yaw = 1.05;
            h.st.camera.pitch = 0.22;
            h.st.camera.frame(Vec3::new(40.0, 52.0, -14.0), Vec3::new(60.0, 104.0, 18.0));
            h.steps(2);
            h.shot("zero-scan-15-dev-muzzle");
            h.st.dev_range = 2.0;
            h.st.dev_tol = 0.1;
            h.st.model_gen += 1;
            h.step();
            while h.st.dev_busy() && Instant::now() < end {
                h.step();
                std::thread::sleep(Duration::from_millis(10));
            }
            h.steps(2);
            h.shot("zero-scan-16-dev-muzzle-2mm");
            for f in &h.st.doc.features {
                eprintln!("  {} {:?} {:?} at {:.2?} axis {:.3?}", f.name, f.op, f.prim, f.origin, f.rot * DVec3::Y);
            }
        }
    }

    if want("scan-plane") {
        // retopology, step 4: the Cube and the Sketch start on the scan's
        // flat face under the pointer (else its tangent plane): the side's
        // large face, a slanted face of the grip, the top's slope
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        if std::path::Path::new(&path).exists() {
            let mut d = Doc::default();
            d.reference = Some(nurbs_kernel::RefMesh { path: path.clone() });
            h.reset(d, "Retopo");
            h.step();
            let end = Instant::now() + Duration::from_secs(180);
            while h.st.ref_busy() && Instant::now() < end {
                h.step();
                std::thread::sleep(Duration::from_millis(20));
            }
            h.settle();
            h.st.toast = None;
            let refm = h.st.reference.clone().expect("reference");
            let side = |x: f64, y: f64| -> Vec3 { refm.hit(DVec3::new(x, y, 100.0), DVec3::new(0.0, 0.0, -1.0)).map(|q| q.0.as_vec3()).unwrap_or(Vec3::new(x as f32, y as f32, 12.0)) };
            let top = |x: f64, z: f64| -> Vec3 { refm.hit(DVec3::new(x, 200.0, z), DVec3::new(0.0, -1.0, 0.0)).map(|q| q.0.as_vec3()).unwrap_or(Vec3::new(x as f32, 100.0, z as f32)) };
            // the pointer rests: the flat face is searched
            let rest = |h: &mut Harness, p: egui::Pos2| {
                std::thread::sleep(Duration::from_millis(100));
                h.move_to(p);
                h.steps(2);
            };
            let plane = |h: &Harness, p: egui::Pos2| h.st.start_plane([p.x, p.y]).map(|(pl, _, _)| format!("{} n {:.3?}", pl.label(), pl.n));
            // 1. the side's large flat face: a Cube
            h.st.camera.yaw = 0.45;
            h.st.camera.pitch = 0.3;
            h.st.camera.frame(Vec3::new(-30.0, 66.0, 0.0), Vec3::new(18.0, 100.0, 18.0));
            h.steps(2);
            h.st.tool = Tool::Create(PrimKind::Box);
            let a = h.screen_of(side(-9.0, 79.0));
            rest(&mut h, a);
            eprintln!("  side: {:?}", plane(&h, a));
            h.shot("zero-scan-plane-1-hover");
            let b = h.screen_of(side(3.0, 89.0));
            h.drag(a, b, 10, true);
            h.settle();
            h.shot("zero-scan-plane-2-base");
            // the height: the pointer over the base's centre, raised along
            // its normal
            // (seen head-on, the pointer goes up the screen)
            let raised = |h: &Harness, by: f64| -> egui::Pos2 {
                let c = h.st.creation.as_ref().expect("creation");
                let m = c.plane.at([(c.a[0] + c.b[0]) * 0.5, (c.a[1] + c.b[1]) * 0.5]);
                let eye = h.st.view().eye.as_dvec3();
                let f = pos2(c.height_from[0], c.height_from[1]);
                if c.plane.n.dot((m - eye).normalize()).abs() > 0.94 {
                    return pos2(f.x, f.y - (by / h.st.mm_per_point(m)) as f32);
                }
                f + (h.screen_of((m + c.plane.n * by).as_vec3()) - h.screen_of(m.as_vec3()))
            };
            let up = raised(&h, 5.0);
            h.move_to(up);
            h.settle();
            h.click(up);
            h.settle();
            h.st.tool = Tool::Select;
            h.st.select(None, false);
            h.steps(2);
            h.shot("zero-scan-plane-3-cube");
            // 2. where the scan is round (the button's rim): its tangent plane
            h.st.tool = Tool::Create(PrimKind::Box);
            let rim = h.screen_of(side(-24.25 + 4.4, 80.45));
            rest(&mut h, rim);
            eprintln!("  button's rim: {:?}", plane(&h, rim));
            h.shot("zero-scan-plane-4-tangent");
            // 3. a slanted face of the grip (14° off the side): a Cube on it
            h.st.camera.yaw = 0.2;
            h.st.camera.pitch = 0.15;
            h.st.camera.frame(Vec3::new(-40.0, 15.0, -2.0), Vec3::new(-12.0, 48.0, 14.0));
            h.steps(2);
            let a = h.screen_of(side(-27.0, 29.0));
            rest(&mut h, a);
            eprintln!("  grip: {:?}", plane(&h, a));
            h.shot("zero-scan-plane-5-slant-hover");
            let b = h.screen_of(side(-23.0, 37.0));
            h.drag(a, b, 10, true);
            h.settle();
            let up = raised(&h, 3.0);
            h.move_to(up);
            h.settle();
            h.click(up);
            h.settle();
            h.st.tool = Tool::Select;
            h.st.select(None, false);
            h.steps(2);
            h.shot("zero-scan-plane-6-slant-cube");
            // 4. a Sketch on the top's slope (8.5° off level)
            h.st.camera.yaw = 0.5;
            h.st.camera.pitch = 0.75;
            h.st.camera.frame(Vec3::new(-24.0, 92.0, -8.0), Vec3::new(8.0, 106.0, 14.0));
            h.steps(2);
            h.key(egui::Key::L);
            let pts = [top(-14.0, 4.5), top(-6.0, 4.5), top(-6.0, 8.0), top(-14.0, 8.0)].map(|w| h.screen_of(w));
            rest(&mut h, pts[0]);
            eprintln!("  top: {:?}", plane(&h, pts[0]));
            h.shot("zero-scan-plane-7-top-hover");
            for p in pts {
                h.click(p);
            }
            // closed on its first point (as placed, on the grid)
            let first = h.st.sketch.as_ref().map(|k| h.screen_of(k.plane.at(k.pts[0]).as_vec3())).unwrap_or(pts[0]);
            h.move_to(first);
            h.steps(2);
            h.click(first);
            let up = raised(&h, 2.5);
            h.move_to(up);
            h.settle();
            h.click(up);
            h.settle();
            h.st.tool = Tool::Select;
            h.st.select(None, false);
            h.steps(2);
            h.shot("zero-scan-plane-8-top-sketch");
            h.st.ref_shown = false;
            h.steps(2);
            h.shot("zero-scan-plane-9-part");
            h.st.ref_shown = true;
            for f in &h.st.doc.features {
                eprintln!("  {} {:?} {:?} at {:.2?} up {:.3?}", f.name, f.op, f.prim, f.origin, f.rot * DVec3::Y);
            }
        }
    }

    if want("scan-slot") {
        // retopology, step 5: the Slot (S) dragged along the vents of the
        // scan, end to end (a block stands for the front part, so the slots
        // cut it), then the Cube drawn on the scan corner to corner across
        // the square button (a block): each read when let go; on a plain
        // face the Cube goes on to its height
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        if std::path::Path::new(&path).exists() {
            let mut d = Doc::default();
            d.reference = Some(nurbs_kernel::RefMesh { path: path.clone() });
            let f = d.add(Op::Add, Prim::Box { x: 18.0, y: 37.0, z: 24.8 }, DVec3::new(47.0, 63.0, 0.0), DQuat::IDENTITY);
            d.feature_mut(f).unwrap().name = "Front".into();
            h.reset(d, "Retopo");
            h.step();
            let end = Instant::now() + Duration::from_secs(180);
            while h.st.ref_busy() && Instant::now() < end {
                h.step();
                std::thread::sleep(Duration::from_millis(20));
            }
            h.settle();
            h.st.toast = None;
            let refm = h.st.reference.clone().expect("reference");
            let side = |x: f64, y: f64| -> Vec3 { refm.hit(DVec3::new(x, y, 100.0), DVec3::new(0.0, 0.0, -1.0)).map(|q| q.0.as_vec3()).unwrap_or(Vec3::new(x as f32, y as f32, 12.0)) };
            let toast = |h: &Harness| h.st.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default();
            // 1. the four vents
            h.st.camera.yaw = 0.3;
            h.st.camera.pitch = 0.2;
            h.st.camera.frame(Vec3::new(34.0, 62.0, 4.0), Vec3::new(56.0, 100.0, 16.0));
            h.steps(2);
            h.key(egui::Key::S);
            for (k, yc) in [70.0, 77.6, 85.6, 93.4].into_iter().enumerate() {
                let (a, b) = (h.screen_of(side(40.6, yc)), h.screen_of(side(49.6, yc)));
                h.move_to(a);
                h.steps(2);
                if k == 0 {
                    h.shot("zero-scan-slot-1-armed");
                }
                h.drag(a, b, 10, false);
                h.steps(2);
                if k == 0 {
                    h.shot("zero-scan-slot-2-drag");
                }
                h.press(b, false);
                h.step();
                h.settle();
                eprintln!("  vent {k}: {}", toast(&h));
                if k == 0 {
                    h.shot("zero-scan-slot-3-read");
                }
            }
            h.st.tool = Tool::Select;
            h.st.select(None, false);
            h.st.toast = None;
            h.steps(2);
            h.shot("zero-scan-slot-4-vents");
            h.st.ref_ghost = true;
            h.steps(2);
            h.shot("zero-scan-slot-5-ghost");
            h.st.ref_ghost = false;
            h.st.ref_shown = false;
            h.steps(2);
            h.shot("zero-scan-slot-6-part");
            h.st.ref_shown = true;
            // 2. the square button: the Cube drawn across it, corner to corner
            h.st.camera.yaw = 0.35;
            h.st.camera.pitch = 0.3;
            h.st.camera.frame(Vec3::new(-24.0, 68.0, 8.0), Vec3::new(-3.0, 88.0, 18.0));
            h.steps(2);
            h.st.tool = Tool::Create(PrimKind::Box);
            // from just outside a corner to just outside the other (shot
            // scan-box: from 2 mm outside)
            let (a, b) = (h.screen_of(side(-16.25, 75.25)), h.screen_of(side(-9.75, 81.25)));
            h.move_to(a);
            h.steps(2);
            h.drag(a, b, 10, false);
            h.steps(2);
            h.shot("zero-scan-slot-7-box-drag");
            h.press(b, false);
            h.step();
            h.settle();
            eprintln!("  button: {}", toast(&h));
            h.shot("zero-scan-slot-8-block");
            // 3. a plain face: nothing read, the Cube goes on to its height
            h.st.toast = None;
            h.st.tool = Tool::Create(PrimKind::Box);
            let (a, b) = (h.screen_of(side(-5.0, 76.0)), h.screen_of(side(1.0, 84.0)));
            h.move_to(a);
            h.steps(2);
            h.drag(a, b, 10, true);
            h.settle();
            eprintln!("  plain face: phase {:?}, toast {:?}", h.st.creation.as_ref().map(|c| c.phase), toast(&h));
            h.shot("zero-scan-slot-9-plain");
            h.key(egui::Key::Escape);
            h.st.tool = Tool::Select;
            h.st.select(None, false);
            h.st.toast = None;
            h.st.ref_shown = false;
            h.steps(2);
            h.shot("zero-scan-slot-10-block-alone");
            h.st.ref_shown = true;
            for f in &h.st.doc.features {
                eprintln!("  {} {:?} {:?} at {:.2?} up {:.3?} corner {:?}", f.name, f.op, f.prim, f.origin, f.rot * DVec3::Y, f.profile.as_ref().map(|p| p.corner.clone()));
            }
        }
    }

    if want("scan-box") {
        // the Cube read on the scan from a loose drag: the square button
        // from 2 mm outside its corners (the plate under it, whose edges
        // pass 1.5 mm outside it, is not taken for its sides), then the
        // small pocket by the vents (3.3 mm deep, a groove along its side)
        // from 1 mm outside its corners
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        if std::path::Path::new(&path).exists() {
            let mut d = Doc::default();
            d.reference = Some(nurbs_kernel::RefMesh { path: path.clone() });
            let f = d.add(Op::Add, Prim::Box { x: 12.0, y: 14.0, z: 25.0 }, DVec3::new(32.0, 70.0, 0.0), DQuat::IDENTITY);
            d.feature_mut(f).unwrap().name = "Side".into();
            h.reset(d, "Retopo");
            h.step();
            let end = Instant::now() + Duration::from_secs(180);
            while h.st.ref_busy() && Instant::now() < end {
                h.step();
                std::thread::sleep(Duration::from_millis(20));
            }
            h.settle();
            h.st.toast = None;
            let refm = h.st.reference.clone().expect("reference");
            let side = |x: f64, y: f64| -> Vec3 { refm.hit(DVec3::new(x, y, 100.0), DVec3::new(0.0, 0.0, -1.0)).map(|q| q.0.as_vec3()).unwrap_or(Vec3::new(x as f32, y as f32, 12.0)) };
            let toast = |h: &Harness| h.st.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default();
            // (name, its corners, the margin outside them, whether the drag
            // goes from the top left corner (the face is flat there), the
            // view)
            for (k, (name, lo, hi, margin, flip, from, to)) in [
                ("button", (-15.75, 75.75), (-10.25, 80.75), 2.0, false, Vec3::new(-24.0, 68.0, 8.0), Vec3::new(-3.0, 88.0, 18.0)),
                ("pocket", (30.25, 74.25), (33.75, 78.25), 1.0, true, Vec3::new(18.0, 62.0, 4.0), Vec3::new(46.0, 90.0, 16.0)),
            ]
            .into_iter()
            .enumerate()
            {
                h.st.camera.yaw = 0.35;
                h.st.camera.pitch = 0.3;
                h.st.camera.frame(from, to);
                h.steps(2);
                h.st.tool = Tool::Create(PrimKind::Box);
                let (a, b) = if flip {
                    (h.screen_of(side(lo.0 - margin, hi.1 + margin)), h.screen_of(side(hi.0 + margin, lo.1 - margin)))
                } else {
                    (h.screen_of(side(lo.0 - margin, lo.1 - margin)), h.screen_of(side(hi.0 + margin, hi.1 + margin)))
                };
                h.move_to(a);
                h.steps(2);
                h.drag(a, b, 10, false);
                h.steps(2);
                h.shot(&format!("zero-scan-box-{}-{name}-drag", 3 * k + 1));
                h.press(b, false);
                h.step();
                h.settle();
                eprintln!("  {name}: {}", toast(&h));
                h.shot(&format!("zero-scan-box-{}-{name}-read", 3 * k + 2));
                h.st.tool = Tool::Select;
                h.st.select(None, false);
                h.st.toast = None;
                h.st.ref_shown = false;
                h.steps(2);
                h.shot(&format!("zero-scan-box-{}-{name}-alone", 3 * k + 3));
                h.st.ref_shown = true;
            }
            for f in &h.st.doc.features {
                eprintln!("  {} {:?} {:?} at {:.2?} up {:.3?} corner {:?}", f.name, f.op, f.prim, f.origin, f.rot * DVec3::Y, f.profile.as_ref().map(|p| p.corner.clone()));
            }
        }
    }

    if want("scan-steps") {
        // the round button in two steps (a ring, a cap on it) read in one
        // drag across the ring, rim to rim: two cylinders round one axis;
        // then the deviation
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        if std::path::Path::new(&path).exists() {
            let mut d = Doc::default();
            d.reference = Some(nurbs_kernel::RefMesh { path: path.clone() });
            h.reset(d, "Retopo");
            h.step();
            let end = Instant::now() + Duration::from_secs(180);
            while h.st.ref_busy() && Instant::now() < end {
                h.step();
                std::thread::sleep(Duration::from_millis(20));
            }
            h.settle();
            h.st.toast = None;
            let refm = h.st.reference.clone().expect("reference");
            let side = |x: f64, y: f64| -> Vec3 { refm.hit(DVec3::new(x, y, 100.0), DVec3::new(0.0, 0.0, -1.0)).map(|q| q.0.as_vec3()).unwrap_or(Vec3::new(x as f32, y as f32, 12.0)) };
            let toast = |h: &Harness| h.st.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default();
            h.st.camera.yaw = 0.4;
            h.st.camera.pitch = 0.45;
            h.st.camera.frame(Vec3::new(-33.0, 72.0, 8.0), Vec3::new(-17.0, 89.0, 18.0));
            h.steps(2);
            h.st.tool = Tool::Create(PrimKind::Cylinder);
            // across the ring, 1 mm off its middle (at -25.3, 80.5)
            let (a, b) = (h.screen_of(side(-30.7, 79.5)), h.screen_of(side(-19.9, 79.5)));
            h.move_to(a);
            h.steps(2);
            h.drag(a, b, 10, false);
            h.steps(2);
            h.shot("zero-scan-steps-1-drag");
            h.press(b, false);
            h.step();
            h.settle();
            eprintln!("  button: {}", toast(&h));
            h.shot("zero-scan-steps-2-read");
            h.st.tool = Tool::Select;
            h.st.select(None, false);
            h.st.toast = None;
            h.st.ref_ghost = true;
            h.steps(2);
            h.shot("zero-scan-steps-3-ghost");
            h.st.ref_ghost = false;
            h.st.ref_shown = false;
            h.steps(2);
            h.shot("zero-scan-steps-4-alone");
            h.st.ref_shown = true;
            h.st.set_deviation(true);
            let end = Instant::now() + Duration::from_secs(60);
            h.step();
            while h.st.dev_busy() && Instant::now() < end {
                h.step();
                std::thread::sleep(Duration::from_millis(10));
            }
            h.steps(2);
            if let Some(d) = &h.st.deviation {
                eprintln!("  deviation: {} points near, {} within {} mm, median {:.3}, 95 % {:.3} mm", d.near, d.within, d.tol, d.p50, d.p95);
            }
            h.shot("zero-scan-steps-5-deviation");
            h.st.set_deviation(false);
            for f in &h.st.doc.features {
                eprintln!("  {} {:?} {:?} at {:.2?} axis {:.3?}", f.name, f.op, f.prim, f.origin, f.rot * DVec3::Y);
            }
        }
    }

    if want("scan-primary") {
        // the "scan → shapes" button's first stage: the primary shapes (the
        // big blocks) read as slabs seen from the scan's side, made as Cube
        // and Sketch steps; through the ghost, alone, then the deviation
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        if std::path::Path::new(&path).exists() {
            let mut d = Doc::default();
            d.reference = Some(nurbs_kernel::RefMesh { path: path.clone() });
            h.reset(d, "Retopo");
            h.step();
            let end = Instant::now() + Duration::from_secs(180);
            while h.st.ref_busy() && Instant::now() < end {
                h.step();
                std::thread::sleep(Duration::from_millis(20));
            }
            h.settle();
            h.st.toast = None;
            let toast = |h: &Harness| h.st.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default();
            let wait_dev = |h: &mut Harness| {
                let end = Instant::now() + Duration::from_secs(90);
                h.step();
                while h.st.dev_busy() && Instant::now() < end {
                    h.step();
                    std::thread::sleep(Duration::from_millis(10));
                }
                h.steps(2);
            };
            h.st.camera.yaw = 0.55;
            h.st.camera.pitch = 0.3;
            if let Some(m) = &h.st.reference {
                let (lo, hi) = (m.lo.as_vec3(), m.hi.as_vec3());
                h.st.camera.frame(lo, hi);
            }
            h.steps(2);
            h.shot("zero-scan-primary-0-scan");
            h.st.read_primaries();
            h.step();
            h.settle();
            eprintln!("  primary: {}", toast(&h));
            wait_dev(&mut h);
            h.st.set_deviation(false);
            h.st.ref_ghost = true;
            h.steps(2);
            h.shot("zero-scan-primary-1-ghost");
            h.st.ref_ghost = false;
            h.st.ref_shown = false;
            h.st.toast = None;
            h.steps(2);
            h.shot("zero-scan-primary-2-alone");
            h.st.set_deviation(true);
            wait_dev(&mut h);
            if let Some(d) = &h.st.deviation {
                eprintln!("  deviation: {} points near, {} within {} mm, median {:.3}, 95 % {:.3} mm", d.near, d.within, d.tol, d.p50, d.p95);
            }
            h.shot("zero-scan-primary-3-deviation");
            h.st.camera.yaw = 0.0;
            h.st.camera.pitch = 0.0;
            h.steps(2);
            h.shot("zero-scan-primary-4-side");
            h.st.camera.yaw = std::f32::consts::PI;
            h.steps(2);
            h.shot("zero-scan-primary-5-other-side");
            h.st.set_deviation(false);
            for f in &h.st.doc.features {
                eprintln!("  {} {:?} {:?} at {:.2?}", f.name, f.op, f.prim, f.origin);
            }
            if let Some(m) = &h.st.model {
                eprintln!("  model: {} errors {:?}", m.faces.len(), m.errors);
            }
        }
    }

    if want("scan-fillet") {
        // retopology, step 6: a block laid on the receiver's back (its side
        // and top on the scan's), the edge between them picked and Fit to
        // scan: the scan read in slices across it, the fillet made at the
        // radius read; the deviation before and after; then the square
        // button's block (read as in step 5), its four top edges fitted
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        if std::path::Path::new(&path).exists() {
            let mut d = Doc::default();
            d.reference = Some(nurbs_kernel::RefMesh { path: path.clone() });
            let f = d.add(Op::Add, Prim::Box { x: 5.5, y: 21.3, z: 26.4 }, DVec3::new(-7.25, 80.0, 0.0), DQuat::IDENTITY);
            d.feature_mut(f).unwrap().name = "Receiver".into();
            h.reset(d, "Retopo");
            h.step();
            let end = Instant::now() + Duration::from_secs(180);
            while h.st.ref_busy() && Instant::now() < end {
                h.step();
                std::thread::sleep(Duration::from_millis(20));
            }
            h.settle();
            h.st.toast = None;
            let refm = h.st.reference.clone().expect("reference");
            let side = |x: f64, y: f64| -> Vec3 { refm.hit(DVec3::new(x, y, 100.0), DVec3::new(0.0, 0.0, -1.0)).map(|q| q.0.as_vec3()).unwrap_or(Vec3::new(x as f32, y as f32, 12.0)) };
            let toast = |h: &Harness| h.st.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default();
            let wait_dev = |h: &mut Harness| {
                let end = Instant::now() + Duration::from_secs(60);
                h.step();
                while h.st.dev_busy() && Instant::now() < end {
                    h.step();
                    std::thread::sleep(Duration::from_millis(10));
                }
                h.steps(2);
            };
            let tag = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
            // 1. the receiver's back: side and top, a large blend between
            h.st.camera.yaw = 0.55;
            h.st.camera.pitch = 0.5;
            h.st.camera.frame(Vec3::new(-16.0, 90.0, 2.0), Vec3::new(2.0, 106.0, 16.0));
            h.steps(2);
            h.shot("zero-scan-fillet-1-block");
            h.st.set_deviation(true);
            wait_dev(&mut h);
            h.shot("zero-scan-fillet-2-dev-before");
            h.st.set_deviation(false);
            h.st.set_sel_mode(SelMode::Edge);
            h.st.select(None, false);
            h.st.edge_sel = vec![EdgeRef { faces: [tag(f, 2), tag(f, 4)], at: DVec3::new(-7.25, 101.3, 13.2) }];
            h.steps(3);
            h.settle();
            h.steps(12);
            h.shot("zero-scan-fillet-3-edge");
            h.st.fit_round_to_scan();
            h.settle();
            eprintln!("  receiver: {}", toast(&h));
            h.shot("zero-scan-fillet-4-fit");
            // the deviation with nothing selected (the arcs read are shown
            // while the fillet is)
            let fillet = h.st.selection.clone();
            h.st.select(None, false);
            h.st.set_deviation(true);
            wait_dev(&mut h);
            if let Some(d) = &h.st.deviation {
                eprintln!("  deviation after: {} near, {} within {} mm, median {:.3}, 95 % {:.3}", d.near, d.within, d.tol, d.p50, d.p95);
            }
            h.shot("zero-scan-fillet-5-dev-after");
            h.st.set_deviation(false);
            h.st.selection = fillet;
            h.st.ref_shown = false;
            h.steps(12);
            h.shot("zero-scan-fillet-6-alone");
            h.st.ref_shown = true;
            // 2. the square button: its block read as in step 5, then its
            // four straight top edges fitted
            h.st.set_sel_mode(SelMode::Object);
            h.st.select(None, false);
            h.st.toast = None;
            h.st.camera.yaw = 0.35;
            h.st.camera.pitch = 0.3;
            h.st.camera.frame(Vec3::new(-24.0, 68.0, 8.0), Vec3::new(-3.0, 88.0, 18.0));
            h.steps(2);
            h.st.tool = Tool::Create(PrimKind::Box);
            let (a, b) = (h.screen_of(side(-16.25, 75.25)), h.screen_of(side(-9.75, 81.25)));
            h.move_to(a);
            h.steps(2);
            h.drag(a, b, 10, false);
            h.steps(2);
            h.press(b, false);
            h.step();
            h.settle();
            eprintln!("  button: {}", toast(&h));
            let block = h.st.selection.first().copied().unwrap_or(0);
            h.st.tool = Tool::Select;
            h.st.set_sel_mode(SelMode::Edge);
            h.st.select(None, false);
            let top = tag(block, 1);
            let m = h.st.model.clone().expect("model");
            let flat = |t: i64| m.faces.iter().any(|x| x.tag.map(|g| g.pack()) == Some(t) && x.kind == nurbs_kernel::model::SurfKind::Plane);
            let picks: Vec<EdgeRef> = (0..m.edges.len())
                .filter_map(|i| {
                    let r = h.st.edge_ref(i)?;
                    let other = if r.faces[0] == top { r.faces[1] } else if r.faces[1] == top { r.faces[0] } else { return None };
                    flat(other).then_some(r)
                })
                .collect();
            eprintln!("  button: {} top edges picked", picks.len());
            h.st.edge_sel = picks;
            h.st.toast = None;
            h.st.camera.yaw = 0.45;
            h.st.camera.pitch = 0.55;
            h.st.camera.frame(Vec3::new(-19.0, 72.0, 11.0), Vec3::new(-7.0, 84.0, 18.0));
            h.steps(3);
            h.settle();
            h.steps(12);
            h.shot("zero-scan-fillet-7-button-edges");
            h.st.fit_round_to_scan();
            h.settle();
            eprintln!("  button edges: {}", toast(&h));
            h.shot("zero-scan-fillet-8-button-fit");
            h.st.ref_shown = false;
            h.steps(2);
            h.shot("zero-scan-fillet-9-button-alone");
            h.st.ref_shown = true;
            for f in &h.st.doc.features {
                eprintln!("  {} {:?} {:?} round {:?}", f.name, f.op, f.prim, f.round.as_ref().map(|r| (r.size, r.edges.len())));
            }
        }
    }

    if want("tension-point") {
        // the top front edge filleted at 2 mm, then the top -X edge at 3 mm:
        // OpenCascade carries the second round the first one's end and down
        // the front vertical edge, its corner there closing on a point of
        // the -X side; with tension that corner turns conic too
        let mut d = Doc::default();
        let bx = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Point corner");
        h.settle();
        let tag = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
        let mut a = 0;
        h.st.edit("Fillet", |d| a = d.add_round(2.0, false, vec![EdgeRef { faces: [tag(bx, 2), tag(bx, 4)], at: DVec3::new(0.0, 20.0, 15.0) }]));
        h.settle();
        let mut b = 0;
        h.st.edit("Fillet", |d| b = d.add_round(3.0, false, vec![EdgeRef { faces: [tag(bx, 2), tag(bx, 1)], at: DVec3::new(-20.0, 20.0, 0.0) }]));
        h.settle();
        let _ = a;
        h.st.select(None, false);
        h.st.camera.yaw = -0.75;
        h.st.camera.pitch = 0.55;
        h.st.camera.frame(Vec3::new(-24.0, 6.0, 4.0), Vec3::new(-10.0, 22.0, 18.0));
        h.st.camera.target = Vec3::new(-18.5, 17.0, 13.0);
        h.st.camera.dist *= 0.45;
        h.steps(2);
        h.settle();
        h.shot("zero-tension-point-0-blended");
        // (Blend corners off: the shape tension is made on, still circular)
        h.st.edit("Blend", |d| d.feature_mut(b).unwrap().round.as_mut().unwrap().blend = false);
        h.settle();
        h.shot("zero-tension-point-1-circular");
        for (t, name) in [(0.7, "2-full"), (0.3, "3-flat")] {
            h.st.set_round_tension(b, t, true);
            h.st.end_live();
            h.settle();
            eprintln!("  tension {t}: errors {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
            h.shot(&format!("zero-tension-point-{name}"));
        }
    }
    if want("tension") {
        // the direct fillet's modifiers: Shift held during the drag sets
        // the tension (up fuller), Ctrl held makes a chamfer
        h.reset(test_part(), "Test part");
        h.key(egui::Key::Num2);
        let a = h.screen_of(Vec3::new(12.0, 20.0, 15.0));
        h.drag(a, a + egui::vec2(0.0, -60.0), 10, true);
        h.settle();
        let fillet = |h: &Harness| h.st.doc.features.iter().filter(|f| f.round.is_some()).map(|f| (f.name.clone(), f.round.as_ref().unwrap().size, f.round.as_ref().unwrap().tension, f.round.as_ref().unwrap().chamfer)).collect::<Vec<_>>();
        eprintln!("  fillet {:?}", fillet(&h));
        h.shot("zero-tension-1-circular");
        // on the fillet's face: Shift held, up = fuller
        let face = h.screen_of(Vec3::new(14.0, 19.3, 14.3));
        h.press(face, true);
        h.move_to(face + egui::vec2(0.0, -4.0));
        h.hold(Modifiers::SHIFT);
        for i in 1..=10 {
            h.move_to(face + egui::vec2(0.0, -4.0 - 6.0 * i as f32));
        }
        h.settle();
        h.shot("zero-tension-2-shift-drag");
        h.press(face + egui::vec2(0.0, -64.0), false);
        h.hold(Modifiers::NONE);
        h.settle();
        eprintln!("  after Shift drag {:?} errors {:?}", fillet(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-tension-3-fuller");
        // down = flatter
        let face = h.screen_of(Vec3::new(14.0, 19.3, 14.3));
        h.press(face, true);
        h.move_to(face + egui::vec2(0.0, 4.0));
        h.hold(Modifiers::SHIFT);
        for i in 1..=10 {
            h.move_to(face + egui::vec2(0.0, 4.0 + 12.0 * i as f32));
        }
        h.press(face + egui::vec2(0.0, 124.0), false);
        h.hold(Modifiers::NONE);
        h.settle();
        eprintln!("  flatter {:?}", fillet(&h));
        h.shot("zero-tension-4-flatter");
        // Ctrl held on another edge: a chamfer
        h.key(egui::Key::Escape);
        let b = h.screen_of(Vec3::new(20.0, 8.0, 15.0));
        h.press(b, true);
        h.move_to(b + egui::vec2(4.0, 0.0));
        h.hold(Modifiers::CTRL);
        for i in 1..=10 {
            h.move_to(b + egui::vec2(4.0 + 4.0 * i as f32, 0.0));
        }
        h.press(b + egui::vec2(44.0, 0.0), false);
        h.hold(Modifiers::NONE);
        h.settle();
        eprintln!("  Ctrl drag {:?}", fillet(&h));
        h.shot("zero-tension-5-ctrl-chamfer");
        // a sketch corner: rounded, then Shift drag on it
        h.key(egui::Key::Num4);
        h.st.camera.frame(Vec3::new(-60.0, 0.0, 10.0), Vec3::new(20.0, 0.0, 50.0));
        h.steps(2);
        h.key(egui::Key::L);
        let g = |h: &Harness, x: f32, z: f32| h.screen_of(Vec3::new(x, 0.0, z));
        for (x, z) in [(-50.0, 20.0), (-20.0, 20.0), (-20.0, 45.0), (-50.0, 45.0), (-50.0, 20.0)] {
            h.click(g(&h, x, z));
        }
        h.click(g(&h, -30.0, 30.0) + egui::vec2(0.0, -40.0));
        h.settle();
        let sk = h.st.doc.features.iter().find(|f| f.profile.is_some() && !f.is_path()).map(|f| f.id);
        h.key(egui::Key::Num1);
        h.steps(2);
        if let Some(id) = sk {
            let top = h.st.doc.feature(id).map(|f| f.prim.height()).unwrap_or(0.0);
            if let Some(v) = h.st.corner_vertex(id, 1, top) {
                let s = h.screen_of(v.as_vec3());
                h.drag(s, s + egui::vec2(-20.0, 20.0), 10, true);
                h.settle();
                h.shot("zero-tension-6-corner-circular");
                if let Some(v) = h.st.vert_sel.first().copied() {
                    // Shift: the dot towards the sharp corner (fuller)
                    let s = h.screen_of(v.as_vec3());
                    let f = h.st.doc.feature(id).unwrap().clone();
                    let (Prim::Profile { x, z, .. }, Some(pr)) = (&f.prim, &f.profile) else { panic!() };
                    let c = pr.cuts(*x, *z)[1].unwrap();
                    let cw = f.matrix().transform_point3(DVec3::new(c.corner[0], top, c.corner[1]));
                    let k = h.screen_of(cw.as_vec3());
                    h.press(s, true);
                    h.move_to(s + (k - s) * 0.05);
                    h.hold(Modifiers::SHIFT);
                    for i in 1..=10 {
                        h.move_to(s + (k - s) * (0.05 + 0.055 * i as f32));
                    }
                    h.settle();
                    h.shot("zero-tension-7-corner-shift");
                    h.press(s + (k - s) * 0.6, false);
                    h.hold(Modifiers::NONE);
                    h.settle();
                }
                eprintln!("  corner {:?} tension {:?}", h.st.doc.feature(id).and_then(|f| f.profile.as_ref()).map(|p| p.corner.clone()), h.st.doc.feature(id).and_then(|f| f.profile.as_ref()).map(|p| p.tension.clone()));
                h.shot("zero-tension-8-corner-fuller");
            }
        }
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
    }

    if want("knife") {
        // from Forme Rectify: the Knife (K) across the block's top, from
        // the front edge's middle to the back edge's, then a point (J)
        h.reset(test_part(), "Test part");
        h.key(egui::Key::K);
        let a = h.screen_of(Vec3::new(-10.0, 20.0, 15.0));
        h.move_to(a);
        h.steps(2);
        eprintln!("  hover {:?}", h.st.knife_hover);
        h.click(a);
        h.settle();
        let b = h.screen_of(Vec3::new(-10.0, 20.0, -15.0));
        h.move_to(b);
        h.steps(2);
        h.shot("zero-knife-1-line");
        h.click(b);
        h.settle();
        h.key(egui::Key::Enter);
        h.settle();
        for f in h.st.doc.features.iter().filter(|f| f.cut.is_some()) {
            let k = f.cut.as_ref().unwrap();
            eprintln!("  {} {} points {:?} lines", f.name, k.pts.len(), k.segs);
        }
        eprintln!("  faces {} errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-knife-2-done");
        h.key(egui::Key::J);
        let c = h.screen_of(Vec3::new(20.0, 8.0, 15.0));
        h.move_to(c);
        h.steps(2);
        h.click(c);
        h.settle();
        h.key(egui::Key::Escape);
        h.shot("zero-knife-3-point");
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
        eprintln!("  edges {} errors {:?}", h.st.model.as_ref().map(|m| m.edges.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
    }

    if want("knife-corner") {
        // Romain's tab (20 nov. 6 h 45): the block's history marker back
        // (its fillet left out), a Knife from corner to corner across the
        // front face: one step, one line (each click made a Knife of its
        // own, and a corner's edge could miss the face)
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, glam::DVec3::ZERO, glam::DQuat::IDENTITY);
        let t = |sub: u16| FaceTag { feature: b, sub }.pack();
        d.add_round(3.0, false, vec![EdgeRef { faces: [t(2), t(5)], at: glam::DVec3::new(0.0, 20.0, -15.0) }]);
        h.reset(d, "Corner");
        h.settle();
        h.st.set_marker(b, 0);
        h.settle();
        h.key(egui::Key::S);
        for at in [Vec3::new(-20.0, 0.0, 15.0), Vec3::new(20.0, 20.0, 15.0)] {
            let p = h.screen_of(at);
            h.move_to(p);
            h.steps(2);
            eprintln!("  pick at {at:?}: {:?}", h.st.knife_pick([p.x, p.y]).map(|k| (h.st.edge_ref(k.0).map(|r| r.faces), k.2, k.3)));
            h.click(p);
            h.settle();
        }
        h.key(egui::Key::Enter);
        h.settle();
        for f in h.st.doc.features.iter().filter(|f| f.cut.is_some()) {
            let k = f.cut.as_ref().unwrap();
            eprintln!("  {} {} points {:?} lines, suspended {}", f.name, k.pts.len(), k.segs, f.suspended);
        }
        eprintln!("  faces {} errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-knife-corner");
    }

    if want("knife-loop") {
        // loop cuts (19 nov., 23 h): Ctrl over the top back edge of a box
        // whose top front edge is filleted: the loop round it, across the
        // fillet; then a plain click: a cut across one face only
        let say = |h: &Harness| {
            for f in h.st.doc.features.iter().filter(|f| f.cut.is_some()) {
                let k = f.cut.as_ref().unwrap();
                eprintln!("  {} {} points {:?} lines", f.name, k.pts.len(), k.segs);
            }
            eprintln!("  faces {} errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
        };
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let t = |sub: u16| FaceTag { feature: c, sub }.pack();
        d.add_round(5.0, false, vec![EdgeRef { faces: [t(2), t(4)], at: DVec3::new(0.0, 20.0, 15.0) }]);
        h.reset(d, "Filleted box");
        h.settle();
        say(&h);
        h.key(egui::Key::K);
        h.hold(Modifiers::CTRL | Modifiers::SHIFT);
        let a = h.screen_of(Vec3::new(8.0, 20.0, -15.0));
        h.move_to(a);
        h.steps(2);
        eprintln!("  loop preview {:?}", h.st.knife_preview.as_ref().map(|(v, c)| (v.len(), *c)));
        h.shot("zero-knife-loop-1-preview");
        h.click(a);
        h.settle();
        h.hold(Modifiers::CTRL);
        say(&h);
        h.shot("zero-knife-loop-2-done");
        // Ctrl + click on the +X side's bottom edge: across that face
        let b = h.screen_of(Vec3::new(20.0, 0.6, -5.0));
        h.move_to(b);
        h.steps(2);
        eprintln!("  face preview {:?}", h.st.knife_preview.as_ref().map(|(v, c)| (v.clone(), *c)));
        h.shot("zero-knife-loop-3-face");
        h.click(b);
        h.settle();
        h.hold(Modifiers::NONE);
        say(&h);
        // the test part's front face (notched by the slot): square to its
        // bottom edge
        h.reset(test_part(), "Test part");
        h.key(egui::Key::S);
        h.hold(Modifiers::CTRL);
        let c2 = h.screen_of(Vec3::new(-15.0, 0.6, 15.0));
        h.move_to(c2);
        h.steps(2);
        eprintln!("  front preview {:?}", h.st.knife_preview.as_ref().map(|(v, c)| (v.clone(), *c)));
        h.click(c2);
        h.settle();
        say(&h);
        // Ctrl + Shift on the test part: the loop goes on across flat faces
        h.hold(Modifiers::CTRL | Modifiers::SHIFT);
        let d2 = h.screen_of(Vec3::new(15.0, 0.6, 15.0));
        h.move_to(d2);
        h.steps(2);
        eprintln!("  test part loop {:?}", h.st.knife_preview.as_ref().map(|(v, c)| (v.len(), *c)));
        h.shot("zero-knife-loop-4-front");
        h.click(d2);
        h.settle();
        h.hold(Modifiers::NONE);
        say(&h);
        h.shot("zero-knife-loop-5-front-loop");
        // X-ray (Alt + X)
        h.hold(Modifiers::ALT);
        h.key(egui::Key::X);
        h.hold(Modifiers::NONE);
        eprintln!("  xray {}", h.st.xray);
        h.shot("zero-knife-loop-6-xray");
        eprintln!("  undo steps: {:?}", h.st.undo_labels());
    }

    if want("comp-flow") {
        // Romain's flow (19 nov. 23 h 18): a cube, the marking menu to
        // Vertex mode, two vertices moved with W, the menu back to Object
        // mode, the cube scaled; then an edge and a face moved and scaled
        let pull = |h: &mut Harness, axis: usize, mm: f32| {
            let c = h.st.pivot().expect("a gizmo").as_vec3();
            let s0 = h.screen_of(c);
            let dir = (h.screen_of(c + Vec3::AXES[axis]) - s0).normalized();
            let grab = s0 + dir * 55.0;
            let by = h.screen_of(c + Vec3::AXES[axis] * mm) - s0;
            let t = Instant::now();
            h.drag(grab, grab + by, 10, true);
            let live = t.elapsed();
            h.settle();
            eprintln!("  drag {:?}, built {:?}", live, t.elapsed());
        };
        let say = |h: &Harness, what: &str| {
            let steps: Vec<String> = h.st.doc.features.iter().map(|f| f.name.clone()).collect();
            eprintln!("  {what}: mode {:?} tool {:?} steps {:?} faces {} errors {:?}", h.st.sel_mode, h.st.tool, steps, h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
        };
        let flick = |h: &mut Harness, at: Pos2, d: egui::Vec2| {
            h.rpress(at, true);
            h.move_to(at + d * 0.5);
            h.move_to(at + d);
            h.rpress(at + d, false);
            h.steps(3);
            h.settle();
        };
        h.reset(Doc::default(), "Untitled");
        h.hold(Modifiers::SHIFT);
        h.key(egui::Key::B);
        h.hold(Modifiers::NONE);
        h.settle();
        h.st.camera.frame(Vec3::new(-15.0, 0.0, -15.0), Vec3::new(15.0, 25.0, 15.0));
        h.steps(2);
        // (the shape's window took its place before the view was framed and
        // stays there, over the cube now: picked again, it is placed again)
        let sel = std::mem::take(&mut h.st.selection);
        h.steps(2);
        h.st.selection = sel;
        h.steps(2);
        say(&h, "cube");
        let mid = h.screen_of(Vec3::new(0.0, 10.0, 10.0));
        flick(&mut h, mid, vec2(-80.0, 0.0));
        say(&h, "menu west");
        h.click(h.screen_of(Vec3::new(-10.0, 20.0, 10.0)));
        h.hold(Modifiers::SHIFT);
        h.click(h.screen_of(Vec3::new(10.0, 20.0, 10.0)));
        h.hold(Modifiers::NONE);
        h.steps(2);
        eprintln!("  vertices picked {:?}", h.st.vert_sel);
        h.key(egui::Key::W);
        pull(&mut h, 1, 5.0);
        say(&h, "two vertices up 5");
        h.shot("zero-comp-flow-1-vertices");
        flick(&mut h, mid, vec2(57.0, -57.0));
        say(&h, "menu NE");
        h.key(egui::Key::R);
        let c = h.screen_of(h.st.pivot().expect("pivot").as_vec3());
        let t = Instant::now();
        h.drag(c, c + vec2(60.0, 0.0), 10, true);
        h.settle();
        eprintln!("  scale {:?} prim {:?}", t.elapsed(), h.st.doc.features[0].prim);
        say(&h, "scaled");
        h.shot("zero-comp-flow-2-scaled");
        // an edge: W then R
        h.st.undo_doc();
        h.settle();
        h.key(egui::Key::F10);
        h.click(h.screen_of(Vec3::new(0.0, 0.0, 10.0)));
        h.steps(2);
        eprintln!("  edges picked {}", h.st.edge_sel.len());
        h.key(egui::Key::W);
        pull(&mut h, 2, 4.0);
        say(&h, "edge out 4");
        h.st.select(None, false);
        h.key(egui::Key::Escape);
        // a face: W then R
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(10.0, 8.0, 0.0)));
        h.steps(2);
        eprintln!("  faces picked {}", h.st.face_sel.len());
        h.key(egui::Key::W);
        pull(&mut h, 0, 3.0);
        say(&h, "face +X 3");
        h.st.select(None, false);
        h.click(h.screen_of(Vec3::new(0.0, 8.0, 10.0)));
        h.key(egui::Key::R);
        h.steps(2);
        let c = h.screen_of(h.st.pivot().expect("pivot").as_vec3());
        h.drag(c, c - vec2(40.0, 0.0), 10, true);
        h.settle();
        say(&h, "face scaled");
        h.shot("zero-comp-flow-3-face");
        // Shift + drag on a picked face: extrude; two faces picked
        h.st.select(None, false);
        h.key(egui::Key::W);
        h.key(egui::Key::Escape);
        h.click(h.screen_of(Vec3::new(0.0, 10.0, 10.0)));
        h.hold(Modifiers::SHIFT);
        h.click(h.screen_of(Vec3::new(10.0, 10.0, 0.0)));
        eprintln!("  faces picked {}", h.st.face_sel.len());
        let a = h.screen_of(Vec3::new(0.0, 10.0, 10.0));
        let b = h.screen_of(Vec3::new(0.0, 10.0, 16.0));
        h.drag(a, b, 10, true);
        h.hold(Modifiers::NONE);
        h.settle();
        say(&h, "shift drag on two faces");
        h.shot("zero-comp-flow-4-extrude");
        // a vertex fillet on a fresh cube: a drag from its corner
        h.reset(Doc::default(), "Untitled");
        h.hold(Modifiers::SHIFT);
        h.key(egui::Key::B);
        h.hold(Modifiers::NONE);
        h.settle();
        h.key(egui::Key::F9);
        h.key(egui::Key::Q);
        let v = h.screen_of(Vec3::new(10.0, 20.0, 10.0));
        h.move_to(v);
        h.steps(2);
        h.drag(v, v + vec2(0.0, -40.0), 10, true);
        h.settle();
        say(&h, "vertex fillet");
        eprintln!("  fillet edges {:?}", h.st.doc.features.iter().filter_map(|f| f.round.as_ref().map(|r| (r.size, r.edges.len()))).collect::<Vec<_>>());
        h.shot("zero-comp-flow-5-vertex-fillet");
        // the corner's faces picked, Ctrl + D: the cube and its fillet copied in place, moved
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(0.0, 10.0, 10.0)));
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::D);
        h.hold(Modifiers::NONE);
        h.settle();
        say(&h, "duplicated");
        let c = h.st.pivot().expect("a gizmo").as_vec3();
        let s0 = h.screen_of(c);
        let dir = (h.screen_of(c + Vec3::Z) - s0).normalized();
        let grab = s0 + dir * 55.0;
        let by = h.screen_of(c + Vec3::Z * 30.0) - s0;
        h.drag(grab, grab + by, 10, true);
        h.settle();
        say(&h, "copy moved");
        h.shot("zero-comp-flow-6-duplicate");
    }

    if want("slot-hand") {
        // the Slot by hand (20 nov.): a drag on the block's top along X,
        // the width moved, a click, the height moved down, a click (Carve)
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 60.0, y: 20.0, z: 40.0 }, glam::DVec3::ZERO, glam::DQuat::IDENTITY);
        h.reset(d, "Block");
        h.st.camera.frame(Vec3::new(-30.0, 0.0, -20.0), Vec3::new(30.0, 20.0, 20.0));
        h.steps(2);
        h.settle();
        h.st.mode = Op::Carve;
        h.hold(Modifiers::SHIFT);
        h.key(egui::Key::S);
        h.hold(Modifiers::NONE);
        h.steps(2);
        let (a, b) = (h.screen_of(Vec3::new(-15.0, 20.0, 0.0)), h.screen_of(Vec3::new(15.0, 20.0, 0.0)));
        h.drag(a, b, 10, true);
        h.steps(2);
        let w = h.screen_of(Vec3::new(0.0, 20.0, 4.0));
        h.move_to(w);
        h.steps(2);
        eprintln!("  slot drawn: {:?}", h.st.slot_draw.as_ref().map(|s| (s.len(), s.w, s.width)));
        h.shot("zero-slot-hand-1-width");
        h.click(w);
        h.steps(2);
        let down = w + egui::vec2(0.0, 60.0);
        h.move_to(down);
        h.steps(2);
        h.click(down);
        h.settle();
        eprintln!("  slot made: steps {:?} faces {} errors {:?}", h.st.doc.features.iter().map(|f| (f.name.clone(), f.prim)).collect::<Vec<_>>(), h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-slot-hand-2-made");
        // Object mode: a click on the fillet selects the block, all lit
        h.key(egui::Key::Num4);
        h.click(h.screen_of(Vec3::new(20.0, 20.0, 0.0)));
        h.steps(2);
        eprintln!("  object click: selection {:?}", h.st.selection);
        h.shot("zero-slot-hand-3-object");
        // Shift held (Romain 20 nov., 9 h 27): a drag 6° off X drawn level
        h.key(egui::Key::Escape);
        h.hold(Modifiers::SHIFT);
        h.key(egui::Key::S);
        h.steps(2);
        let (a, b) = (h.screen_of(Vec3::new(-20.0, 20.0, -12.0)), h.screen_of(Vec3::new(10.0, 20.0, -9.0)));
        h.drag(a, b, 10, true);
        h.hold(Modifiers::NONE);
        h.steps(2);
        eprintln!("  shift slot: {:?}", h.st.slot_draw.as_ref().map(|s| (s.a, s.b)));
        h.shot("zero-slot-hand-4-shift");
        h.key(egui::Key::Escape);
    }

    if want("dup-faces") {
        // Ctrl + D on faces (20 nov.): a block's faces all but its bottom
        // picked by a rectangle in X-ray, copied as a piece (closed by a
        // bottom) and moved 50 along X; then the top alone (a sheet)
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, glam::DVec3::ZERO, glam::DQuat::IDENTITY);
        h.reset(d, "Block");
        h.st.camera.frame(Vec3::new(-25.0, 0.0, -20.0), Vec3::new(75.0, 25.0, 20.0));
        h.steps(2);
        h.settle();
        h.key(egui::Key::F11);
        h.st.xray = true;
        let (a, b) = (h.screen_of(Vec3::new(-22.0, 22.0, 0.0)), h.screen_of(Vec3::new(22.0, 5.0, 0.0)));
        h.drag(egui::pos2(a.x.min(b.x) - 60.0, a.y.min(b.y) - 60.0), egui::pos2(a.x.max(b.x) + 60.0, a.y.max(b.y)), 10, true);
        h.steps(2);
        eprintln!("  faces picked {}", h.st.face_sel.len());
        h.st.xray = false;
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::D);
        h.hold(Modifiers::NONE);
        h.settle();
        let c = h.st.pivot().expect("a gizmo").as_vec3();
        let s0 = h.screen_of(c);
        let dir = (h.screen_of(c + Vec3::X) - s0).normalized();
        let grab = s0 + dir * 55.0;
        let by = h.screen_of(c + Vec3::X * 50.0) - s0;
        h.drag(grab, grab + by, 10, true);
        h.settle();
        eprintln!("  tweak {:?}", h.st.doc.features.last().and_then(|f| f.tweak.as_ref()).map(|t| (t.delta, t.copy)));
        eprintln!("  copy moved: steps {:?} faces {} errors {:?}", h.st.doc.features.iter().map(|f| f.name.clone()).collect::<Vec<_>>(), h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-dup-faces-1-piece");
        // a double-click on the copy's top (20 nov.): its piece's faces,
        // then moved up 10 as one
        h.key(egui::Key::F11);
        h.key(egui::Key::Escape);
        let p = h.screen_of(Vec3::new(50.0, 20.0, 0.0));
        h.click(p);
        h.click(p);
        h.steps(2);
        eprintln!("  double-click: faces picked {} ({:?})", h.st.face_sel.len(), h.st.picked_faces());
        let c = h.st.pivot().expect("a gizmo").as_vec3();
        let s0 = h.screen_of(c);
        let dir = (h.screen_of(c + Vec3::Y) - s0).normalized();
        let grab = s0 + dir * 55.0;
        let by = h.screen_of(c + Vec3::Y * 10.0) - s0;
        h.drag(grab, grab + by, 10, true);
        h.settle();
        eprintln!("  tweak {:?}", h.st.doc.features.last().and_then(|f| f.tweak.as_ref()).map(|t| (t.delta, t.copy)));
        if let Some(m) = &h.st.model {
            let ys: Vec<f32> = m.positions.iter().filter(|q| q[0] > 30.0).map(|q| q[1]).collect();
            eprintln!("  the copy's y: {:?} to {:?}", ys.iter().cloned().fold(f32::MAX, f32::min), ys.iter().cloned().fold(f32::MIN, f32::max));
        }
        eprintln!("  piece moved: steps {:?} errors {:?}", h.st.doc.features.iter().map(|f| f.name.clone()).collect::<Vec<_>>(), h.st.model.as_ref().map(|m| m.errors.clone()));
        eprintln!("  journal: {:?}", h.st.journal.iter().rev().take(4).map(|j| format!("{} {}", j.kind, j.what)).collect::<Vec<_>>());
        h.shot("zero-dup-faces-2-linked");
        // an edge's loop: the copy's top front edge double-clicked
        h.key(egui::Key::F10);
        let p = h.screen_of(Vec3::new(50.0, 30.0, 15.0));
        h.click(p);
        h.click(p);
        h.steps(2);
        eprintln!("  edge double-click: {} edges", h.st.edge_sel.len());
    }

    if want("linked") {
        // double-clicks (20 nov.): a block whose four vertical edges are
        // filleted, its top's edges double-clicked (the loop round it)
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, glam::DVec3::ZERO, glam::DQuat::IDENTITY);
        let t = |sub: u16| FaceTag { feature: b, sub }.pack();
        let e = |a: u16, c: u16, x: f64, z: f64| EdgeRef { faces: [t(a), t(c)], at: glam::DVec3::new(x, 10.0, z) };
        d.add_round(5.0, false, vec![e(0, 4, 20.0, 15.0), e(0, 5, 20.0, -15.0), e(1, 4, -20.0, 15.0), e(1, 5, -20.0, -15.0)]);
        h.reset(d, "Rounded");
        h.settle();
        h.key(egui::Key::F10);
        let p = h.screen_of(Vec3::new(0.0, 20.0, 15.0));
        h.click(p);
        h.click(p);
        h.steps(2);
        eprintln!("  top edge double-clicked: {} edges", h.st.edge_sel.len());
        h.shot("zero-linked-1-edges");
        h.key(egui::Key::F11);
        h.steps(40);
        h.click(p + egui::vec2(0.0, 30.0));
        h.click(p + egui::vec2(0.0, 30.0));
        h.steps(2);
        eprintln!("  front face double-clicked: {} faces", h.st.face_sel.len());
        h.shot("zero-linked-2-faces");
        h.key(egui::Key::F9);
        h.steps(40);
        let q = h.screen_of(Vec3::new(-20.0, 20.0, 10.0));
        let v = h.st.pick_vertex([q.x, q.y]);
        if let Some(v) = v {
            let q = h.screen_of(v.as_vec3());
            h.click(q);
            h.click(q);
            h.steps(2);
        }
        eprintln!("  vertex {v:?} double-clicked: {} vertices", h.st.vert_sel.len());
    }

    if want("dup-move") {
        // Romain's steps (20 nov. 6 h 57): a cube, three faces picked,
        // duplicated, the copy moved, deselected, then one face of the copy
        // moved, then one of its edges
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, glam::DVec3::ZERO, glam::DQuat::IDENTITY);
        h.reset(d, "Cube");
        h.st.camera.frame(Vec3::new(-25.0, 0.0, -20.0), Vec3::new(75.0, 25.0, 20.0));
        h.steps(2);
        h.settle();
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(0.0, 20.0, 0.0)));
        h.hold(Modifiers::SHIFT);
        h.click(h.screen_of(Vec3::new(20.0, 10.0, 0.0)));
        h.click(h.screen_of(Vec3::new(0.0, 10.0, 15.0)));
        h.hold(Modifiers::NONE);
        eprintln!("  picked {}", h.st.face_sel.len());
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::D);
        h.hold(Modifiers::NONE);
        h.settle();
        let arrow = |h: &mut Harness, axis: Vec3, by: f32| {
            let c = h.st.pivot().expect("a gizmo").as_vec3();
            let s0 = h.screen_of(c);
            let dir = (h.screen_of(c + axis) - s0).normalized();
            let grab = s0 + dir * 55.0;
            let to = h.screen_of(c + axis * by) - s0;
            h.drag(grab, grab + to, 10, true);
            h.settle();
        };
        arrow(&mut h, Vec3::X, 50.0);
        eprintln!("  copy moved: {:?} errors {:?}", h.st.doc.features.last().and_then(|f| f.tweak.as_ref()).map(|t| t.delta), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.key(egui::Key::Escape);
        h.steps(30);
        // the copy's top
        h.click(h.screen_of(Vec3::new(50.0, 20.0, 0.0)));
        eprintln!("  picked {:?}", h.st.face_sel.iter().map(|r| r.tag).collect::<Vec<_>>());
        arrow(&mut h, Vec3::Y, 6.0);
        eprintln!("  face moved: {:?} {:?} errors {:?} toast {:?}", h.st.doc.features.last().map(|f| f.name.clone()), h.st.doc.features.last().and_then(|f| f.tweak.as_ref()).map(|t| t.delta), h.st.model.as_ref().map(|m| m.errors.clone()), h.st.toast.as_ref().map(|t| t.text.clone()));
        h.shot("zero-dup-move-1-face");
        // an edge of the copy: its top front one
        h.key(egui::Key::F10);
        h.key(egui::Key::Escape);
        h.steps(30);
        h.click(h.screen_of(Vec3::new(50.0, 26.0, 15.0)));
        eprintln!("  edges picked {:?}", h.st.edge_sel.iter().map(|e| e.faces).collect::<Vec<_>>());
        arrow(&mut h, Vec3::Z, 5.0);
        eprintln!("  edge moved: {:?} {:?} errors {:?} toast {:?}", h.st.doc.features.last().map(|f| f.name.clone()), h.st.doc.features.last().and_then(|f| f.tweak.as_ref()).map(|t| t.delta), h.st.model.as_ref().map(|m| m.errors.clone()), h.st.toast.as_ref().map(|t| t.text.clone()));
        h.shot("zero-dup-move-2-edge");
    }

    if want("dup-object") {
        // Romain (20 nov. 9 h 24): "CTRL + D doit dupliquer sur place et pas
        // à côté": in Object mode too, the copy in place, the move gizmo on
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, glam::DVec3::ZERO, glam::DQuat::IDENTITY);
        h.reset(d, "Cube");
        h.st.camera.frame(Vec3::new(-25.0, 0.0, -20.0), Vec3::new(75.0, 25.0, 20.0));
        h.steps(2);
        h.settle();
        h.key(egui::Key::F8);
        h.click(h.screen_of(Vec3::new(0.0, 20.0, 0.0)));
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::D);
        h.hold(Modifiers::NONE);
        h.settle();
        let o = |h: &Harness, id| h.st.doc.feature(id).map(|f| f.origin);
        let copy = *h.st.selection.first().expect("the copy selected");
        eprintln!("  original {:?} copy {:?} gizmo {:?} toast {:?}", o(&h, b), o(&h, copy), h.st.gizmo, h.st.toast.as_ref().map(|t| t.text.clone()));
        h.shot("zero-dup-object-1-in-place");
        let c = h.st.pivot().expect("a gizmo").as_vec3();
        let s0 = h.screen_of(c);
        let dir = (h.screen_of(c + Vec3::X) - s0).normalized();
        let grab = s0 + dir * 55.0;
        let to = h.screen_of(c + Vec3::X * 50.0) - s0;
        h.drag(grab, grab + to, 10, true);
        h.settle();
        eprintln!("  original {:?} copy {:?}", o(&h, b), o(&h, copy));
        h.shot("zero-dup-object-2-moved");
    }

    if want("move-free") {
        // the Move gizmo's centre (Romain 20 nov., 9 h 34, Maya): on all
        // three axes, in the plane facing the eye; on a point of interest
        // the block's corner lands on it in depth too
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, glam::DVec3::ZERO, glam::DQuat::IDENTITY);
        let b = d.add(Op::Add, Prim::Box { x: 10.0, y: 10.0, z: 10.0 }, glam::DVec3::new(60.0, 0.0, 20.0), glam::DQuat::IDENTITY);
        h.reset(d, "Cube");
        h.st.camera.frame(Vec3::new(-25.0, 0.0, -20.0), Vec3::new(80.0, 25.0, 30.0));
        h.steps(2);
        h.settle();
        h.click(h.screen_of(Vec3::new(60.0, 10.0, 20.0)));
        let c = h.st.pivot().expect("a gizmo").as_vec3();
        let s0 = h.screen_of(c);
        h.move_to(s0);
        h.steps(2);
        eprintln!("  hot {:?}", h.st.hot_handle);
        // up and left on screen: free, off the axes
        h.drag(s0, s0 + vec2(-120.0, -60.0), 10, false);
        h.steps(3);
        eprintln!("  free: block at {:?}", h.st.doc.feature(b).map(|f| f.origin));
        h.shot("zero-move-free-1-drag");
        h.press(s0 + vec2(-120.0, -60.0), false);
        h.settle();
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::Z);
        h.hold(Modifiers::NONE);
        h.settle();
        // onto the big block's top corner (20, 20, 15): the pointer there
        let c = h.st.pivot().expect("a gizmo").as_vec3();
        let s0 = h.screen_of(c);
        let to = h.screen_of(Vec3::new(20.0, 20.0, 15.0));
        h.drag(s0, to, 12, false);
        h.steps(3);
        eprintln!("  snapped {:?}", h.st.snap_shown.as_ref().map(|s| (s.kind, s.pos)));
        h.shot("zero-move-free-2-snap");
        h.press(to, false);
        h.settle();
        eprintln!("  on the corner: block at {:?}", h.st.doc.feature(b).map(|f| f.origin));
        h.shot("zero-move-free-3-made");
    }

    if want("undo-select") {
        // Ctrl + Z through the clicks (Romain 20 nov., 9 h 38): three faces
        // picked one by one, three Ctrl + Z take them off one by one; then a
        // move undone gives back its selection
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, glam::DVec3::ZERO, glam::DQuat::IDENTITY);
        h.reset(d, "Cube");
        h.st.camera.frame(Vec3::new(-25.0, 0.0, -20.0), Vec3::new(25.0, 25.0, 20.0));
        h.steps(2);
        h.settle();
        let z = |h: &mut Harness| {
            h.hold(Modifiers::CTRL);
            h.key(egui::Key::Z);
            h.hold(Modifiers::NONE);
            h.steps(2);
        };
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(0.0, 20.0, 0.0)));
        h.hold(Modifiers::SHIFT);
        h.click(h.screen_of(Vec3::new(20.0, 10.0, 0.0)));
        h.click(h.screen_of(Vec3::new(0.0, 10.0, 15.0)));
        h.hold(Modifiers::NONE);
        h.steps(2);
        eprintln!("  picked {} · undo {:?}", h.st.face_sel.len(), h.st.undo.iter().map(|u| u.label.clone()).collect::<Vec<_>>());
        for _ in 0..4 {
            z(&mut h);
            eprintln!("  Ctrl + Z: faces {} mode {:?} toast {:?}", h.st.face_sel.len(), h.st.sel_mode, h.st.toast.as_ref().map(|t| t.text.clone()));
        }
        for _ in 0..2 {
            h.hold(Modifiers::CTRL);
            h.key(egui::Key::Y);
            h.hold(Modifiers::NONE);
            h.steps(2);
            eprintln!("  Ctrl + Y: faces {} mode {:?}", h.st.face_sel.len(), h.st.sel_mode);
        }
        h.shot("zero-undo-select-1-redo");
        // the three faces moved up, the move undone: the move gone, the
        // three faces still picked
        h.hold(Modifiers::SHIFT);
        h.click(h.screen_of(Vec3::new(0.0, 10.0, 15.0)));
        h.hold(Modifiers::NONE);
        h.steps(2);
        let c = h.st.pivot().expect("a gizmo").as_vec3();
        let s0 = h.screen_of(c);
        let dir = (h.screen_of(c + Vec3::Y) - s0).normalized();
        let grab = s0 + dir * 55.0;
        let to = h.screen_of(c + Vec3::Y * 5.0) - s0;
        h.drag(grab, grab + to, 10, true);
        h.settle();
        eprintln!("  moved: steps {} faces picked {}", h.st.doc.features.len(), h.st.face_sel.len());
        z(&mut h);
        h.settle();
        eprintln!("  move undone: steps {} faces picked {} selection {:?}", h.st.doc.features.len(), h.st.face_sel.len(), h.st.selection);
        h.shot("zero-undo-select-2-move-undone");
    }

    if want("corner4") {
        // Romain 20 nov., 9 h 56: a cap standing flush on a pocket's end
        // (extrudes there and back): four edges at one corner. The top cut
        // across first (his Loop cut), so the pocket's rim comes in two
        // pieces: its piece by the corner picked is drawn picked (it was
        // drawn on its other piece); the four edges filleted in one step,
        // also after the cap's top edge was rounded
        let tg = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
        let mut d = Doc::default();
        let a = d.add(Op::Add, Prim::Box { x: 40.0, y: 10.0, z: 20.0 }, glam::DVec3::new(15.0, 0.0, 0.0), DQuat::IDENTITY);
        let front = EdgeRef { faces: [tg(a, 2), tg(a, 5)], at: glam::DVec3::new(15.0, 10.0, -10.0) };
        let back = EdgeRef { faces: [tg(a, 2), tg(a, 4)], at: glam::DVec3::new(15.0, 10.0, 10.0) };
        d.add_knife(
            nurbs_kernel::doc::Knife {
                pts: vec![
                    nurbs_kernel::doc::KnifePoint { edge: front, f: 0.4375, at: glam::DVec3::new(17.5, 10.0, -10.0) },
                    nurbs_kernel::doc::KnifePoint { edge: back, f: 0.4375, at: glam::DVec3::new(17.5, 10.0, 10.0) },
                ],
                segs: vec![[0, 1]],
                remove_faces: vec![],
                remove_edges: vec![],
            },
            "Knife",
        );
        let c = d.add(Op::Add, Prim::Cylinder { radius: 2.0, height: 10.0 }, glam::DVec3::new(30.0, 10.0, 0.0), DQuat::IDENTITY);
        let k = d.add(Op::Carve, Prim::Box { x: 25.0, y: 18.0, z: 4.0 }, glam::DVec3::new(17.5, 7.0, 0.0), DQuat::IDENTITY);
        let top = EdgeRef { faces: [tg(k, 0), tg(c, 1)], at: glam::DVec3::new(30.0, 20.0, 0.0) };
        let mut d2 = d.clone();
        d2.add_round(0.9, false, vec![top]);
        for (n, doc) in [("", d), ("-after-top", d2)] {
            h.reset(doc, "Corner");
            let look = |h: &mut Harness, yaw: f32| {
                h.st.camera.frame(Vec3::new(20.0, 6.0, -6.0), Vec3::new(34.0, 21.0, 2.0));
                h.st.camera.yaw = yaw;
                h.st.camera.pitch = 0.6;
                h.steps(2);
            };
            look(&mut h, -1.3);
            h.settle();
            h.key(egui::Key::F10);
            h.steps(1);
            // seen from the pocket: the rim's piece by the corner, the
            // corner's upright edges; from the other side: the cap's foot
            let pts = [(Vec3::new(24.0, 10.0, -2.0), -1.3), (Vec3::new(30.0, 15.0, -2.0), -1.3), (Vec3::new(30.0, 8.5, -2.0), -1.3), (Vec3::new(31.414, 10.0, -1.414), 2.0)];
            for (i, (p, yaw)) in pts.iter().enumerate() {
                if h.st.camera.yaw != *yaw {
                    look(&mut h, *yaw);
                }
                h.hold(if i == 0 { Modifiers::NONE } else { Modifiers::SHIFT });
                h.click(h.screen_of(*p));
                h.hold(Modifiers::NONE);
                h.steps(1);
            }
            look(&mut h, -1.3);
            let m = h.st.model.as_ref().unwrap();
            let drawn: Vec<String> = h.st.edge_sel.iter().filter_map(|r| h.st.model_edge_of(r)).map(|i| {
                let e = &m.edges[i];
                format!("{:.1},{:.1},{:.1} → {:.1},{:.1},{:.1}", e.pts[0][0], e.pts[0][1], e.pts[0][2], e.pts.last().unwrap()[0], e.pts.last().unwrap()[1], e.pts.last().unwrap()[2])
            }).collect();
            eprintln!("  picked {}: drawn on {:?}", h.st.edge_sel.len(), drawn);
            h.shot(&format!("zero-corner4{n}-1-picked"));
            h.st.set_round(0.5, false, true);
            h.st.end_live();
            h.settle();
            let m = h.st.model.as_ref().unwrap();
            eprintln!("  fillet: errors {:?} limits {:?} faces {}", m.errors, m.limits, m.faces.len());
            h.key(egui::Key::Escape);
            h.steps(2);
            h.shot(&format!("zero-corner4{n}-2-filleted"));
            look(&mut h, 2.0);
            h.shot(&format!("zero-corner4{n}-3-filleted-cap-side"));
        }
    }

    if want("carve-order") {
        // a Carve over a shape made after it (Romain 20 nov., 10 h 47: a
        // Sketch made first, a Pill after it, the Sketch set to Carve cut
        // nothing): set to Carve, it goes last and cuts; a Carve made first
        // beside, a block after it, the Carve moved onto it: likewise; a
        // boss standing in a pocket made before it, the pocket moved: kept
        let mut d = Doc::default();
        let a = d.add(Op::Add, Prim::Box { x: 40.0, y: 12.0, z: 12.0 }, DVec3::new(0.0, 14.0, 4.0), DQuat::IDENTITY);
        d.add(Op::Add, Prim::Box { x: 50.0, y: 20.0, z: 16.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let c = d.add(Op::Carve, Prim::Box { x: 10.0, y: 30.0, z: 10.0 }, DVec3::new(0.0, -5.0, 40.0), DQuat::IDENTITY);
        let e = d.add(Op::Add, Prim::Box { x: 30.0, y: 10.0, z: 20.0 }, DVec3::new(0.0, 0.0, -30.0), DQuat::IDENTITY);
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::new(70.0, 0.0, 0.0), DQuat::IDENTITY);
        let pocket = d.add(Op::Carve, Prim::Box { x: 16.0, y: 7.0, z: 12.0 }, DVec3::new(70.0, 14.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Add, Prim::Box { x: 4.0, y: 4.0, z: 4.0 }, DVec3::new(70.0, 14.0, 0.0), DQuat::IDENTITY);
        h.reset(d, "Carves");
        h.st.camera.frame(Vec3::new(-30.0, -5.0, -45.0), Vec3::new(95.0, 30.0, 50.0));
        h.st.camera.dist *= 1.3;
        h.steps(2);
        h.settle();
        let order = |h: &Harness| h.st.doc.features.iter().map(|f| f.name.clone()).collect::<Vec<_>>().join(", ");
        let faces = |h: &Harness| h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0);
        eprintln!("  before: {} · faces {}", order(&h), faces(&h));
        h.shot("zero-carve-order-1-before");
        h.st.selection = vec![a];
        h.st.set_ops(&[a], Op::Carve);
        h.settle();
        eprintln!("  {a} set to Carve: {} · faces {} · toast {:?}", order(&h), faces(&h), h.st.toast.as_ref().map(|t| t.text.clone()));
        h.shot("zero-carve-order-2-set");
        // the Carve made first, moved onto the block made after it
        h.st.selection = vec![c];
        h.steps(2);
        // the gizmo's Z arrow dragged 70 mm towards the block (the drag
        // itself: begin, the shape moved live, end)
        let at = h.screen_of(h.st.pivot().unwrap().as_vec3());
        h.st.begin_drag(nurbs_app::state::Handle::Axis(2), [at.x, at.y]);
        h.st.live(|d| d.feature_mut(c).unwrap().origin.z -= 70.0);
        h.st.end_drag();
        h.settle();
        eprintln!("  {c} moved: {} · faces {} · toast {:?} · origin {:?}", order(&h), faces(&h), h.st.toast.as_ref().map(|t| t.text.clone()), h.st.doc.feature(c).map(|f| f.origin));
        h.shot("zero-carve-order-3-moved");
        let _ = e;
        // the pocket moved: the boss made in it after it stays
        h.st.selection = vec![pocket];
        h.steps(2);
        let at = h.screen_of(h.st.pivot().unwrap().as_vec3());
        h.st.begin_drag(nurbs_app::state::Handle::Axis(0), [at.x, at.y]);
        h.st.live(|d| d.feature_mut(pocket).unwrap().origin.x += 3.0);
        h.st.end_drag();
        h.settle();
        eprintln!("  pocket {pocket} moved: {} · faces {} · toast {:?}", order(&h), faces(&h), h.st.toast.as_ref().map(|t| t.text.clone()));
        h.shot("zero-carve-order-4-pocket");
        h.st.undo_doc();
        h.st.undo_doc();
        h.settle();
        eprintln!("  undo ×2: {} · faces {}", order(&h), faces(&h));
    }

    if want("only-selected") {
        // Only Selected (Romain 20 nov., 10 h 54): the pill's top straight
        // edge picked, filleted alone (it ends square where the edge
        // ends), then chamfered; off, the whole tangent outline
        let mut d = Doc::default();
        let p = d.add(Op::Add, Prim::Profile { x: 60.0, y: 20.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(p).unwrap().profile = Some(nurbs_kernel::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![10.0; 4], tension: vec![], open: false, spline: false });
        h.reset(d, "Pill");
        h.st.camera.frame(Vec3::new(-32.0, 0.0, -12.0), Vec3::new(32.0, 22.0, 12.0));
        h.steps(2);
        h.settle();
        h.key(egui::Key::Num2);
        h.click(h.screen_of(Vec3::new(-5.0, 20.0, 10.0)));
        eprintln!("  picked {}", h.st.edge_sel.len());
        h.st.set_round_only(true);
        h.steps(2);
        h.shot("zero-only-1-picked");
        h.st.set_round(2.0, false, true);
        h.st.end_live();
        h.settle();
        let its = |h: &Harness| h.st.model.as_ref().map(|m| m.faces.iter().filter(|f| f.tag.map_or(false, |t| t.feature > p)).count()).unwrap_or(0);
        eprintln!("  fillet only: its faces {} errors {:?}", its(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-only-2-fillet");
        h.st.set_round(2.0, true, true);
        h.st.end_live();
        h.settle();
        eprintln!("  chamfer only: its faces {} errors {:?}", its(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-only-3-chamfer");
        h.st.set_round_only(false);
        h.settle();
        eprintln!("  chamfer, tangent edges: its faces {} errors {:?}", its(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-only-4-off");
    }

    if want("preview-mirror") {
        // 21 nov.: whole shapes dragged show at once where they go even
        // with a Mirror in the scene: a block with a filleted edge on the +X
        // side, a Mirror across X, a block across the plane. The block and
        // its image as the app draws them at once (before the kernel's
        // answer), against the kernel's model of the same document.
        let mut d = Doc::default();
        let a = d.add(Op::Add, Prim::Box { x: 20.0, y: 12.0, z: 20.0 }, DVec3::new(40.0, 0.0, 0.0), DQuat::IDENTITY);
        let mid = d.add(Op::Add, Prim::Box { x: 30.0, y: 6.0, z: 30.0 }, DVec3::new(0.0, 0.0, 0.0), DQuat::IDENTITY);
        h.reset(d, "Preview");
        h.settle();
        let e = h.st.model.as_ref().and_then(|m| m.edges.iter().position(|e| e.pts.iter().all(|p| (p[1] - 12.0).abs() < 1e-3 && (p[2] - 10.0).abs() < 1e-3))).and_then(|i| h.st.edge_ref(i));
        if let Some(e) = e {
            h.st.sel_mode = SelMode::Edge;
            h.st.edge_sel = vec![e];
            h.st.set_round(3.0, false, true);
            h.settle();
            h.key(egui::Key::Num4);
            h.key(egui::Key::Escape);
        }
        h.st.selection.clear();
        h.hold(Modifiers::ALT);
        h.key(egui::Key::M);
        h.hold(Modifiers::NONE);
        h.settle();
        h.key(egui::Key::Escape);
        h.st.frame_all();
        h.settle();
        // the boxes of an element's faces on each side of the plane
        let boxes = |m: &nurbs_kernel::Model, fm: &[Option<glam::Mat4>], st: &AppState, el: u32| {
            let mut b = [[f32::MAX, f32::MIN, f32::MAX, f32::MIN, f32::MAX, f32::MIN]; 2];
            for (t, tri) in m.triangles.iter().enumerate() {
                let f = m.tri_face[t] as usize;
                if m.faces[f].tag.map(|t| st.element_of(t.feature)) != Some(el) {
                    continue;
                }
                for &v in tri {
                    let mut p = Vec3::from(m.positions[v as usize]);
                    if let Some(Some(x)) = fm.get(f) {
                        p = x.transform_point3(p);
                    }
                    let s = (p.x < 0.0) as usize;
                    for a in 0..3 {
                        b[s][2 * a] = b[s][2 * a].min(p[a]);
                        b[s][2 * a + 1] = b[s][2 * a + 1].max(p[a]);
                    }
                }
            }
            b
        };
        for (id, name) in [(a, "the block on one side"), (mid, "the block across the plane")] {
            h.st.selection = vec![id];
            h.key(egui::Key::W);
            h.settle();
            let pivot = h.st.pivot().unwrap().as_vec3();
            let v = h.st.view();
            let k = v.pixel_size_at((pivot - v.eye).length()) * 78.0;
            let (p0, p1) = (h.screen_of(pivot + Vec3::X * k * 0.8), h.screen_of(pivot + Vec3::X * k * 1.6));
            h.move_to(p0);
            h.steps(2);
            h.press(p0, true);
            // (the pointer moved in one frame: the kernel has not answered)
            h.pending.push(Event::PointerMoved(p1));
            h.step();
            let m = h.st.model.clone().unwrap();
            let pv = h.st.drag_preview();
            let fm = pv.as_ref().map(|pv| nurbs_app::draw::preview_faces(&h.st, &m, pv)).unwrap_or_default();
            let shown = boxes(&m, &fm, &h.st, id);
            eprintln!("  {name}: handle {:?}, moved to x {:.2}, preview {} (faces moved {})", h.st.hot_handle, h.st.doc.feature(id).unwrap().origin.x, pv.is_some(), fm.iter().flatten().count());
            if id == a {
                h.shot("zero-preview-mirror-1-at-once");
            }
            h.settle();
            let m2 = h.st.model.clone().unwrap();
            let real = boxes(&m2, &[], &h.st, id);
            let same = (0..2).all(|s| (0..6).all(|i| (shown[s][i] - real[s][i]).abs() < 2e-3));
            eprintln!("    as drawn at once  +X {:?}  −X {:?}", shown[0], shown[1]);
            eprintln!("    the kernel's      +X {:?}  −X {:?}  (the same: {same})", real[0], real[1]);
            if id == a {
                h.shot("zero-preview-mirror-2-kernel");
            }
            h.press(p1, false);
            h.settle();
            h.st.undo();
            h.settle();
        }
    }

    if want("perf") {
        // FORME_DOC = a scene, FORME_DRAG = ids ("157,132"; else its last
        // shape): each selected and dragged along X by the move gizmo's
        // arrow for 60 frames at 60 a second; per frame the time of the
        // tick, of the interface and of the drawing (a software GPU here:
        // the drawing says little), how many answers the kernel gave
        // meanwhile and how long the last took; then the fine model
        if let Ok(p) = std::env::var("FORME_DOC") {
            let d = Doc::from_json(&std::fs::read_to_string(p).unwrap()).unwrap();
            let t0 = Instant::now();
            h.reset(d, "Scene");
            let m = h.st.model.clone();
            eprintln!("  loaded and built in {:.2} s: {} faces, {} triangles, {} edges, {} errors", t0.elapsed().as_secs_f64(), m.as_ref().map_or(0, |m| m.faces.len()), m.as_ref().map_or(0, |m| m.triangles.len()), m.as_ref().map_or(0, |m| m.edges.len()), m.as_ref().map_or(0, |m| m.errors.len()));
            let last = h.st.doc.features.iter().rev().find(|f| f.is_solid()).map(|f| f.id).unwrap_or(1);
            let ids: Vec<u32> = std::env::var("FORME_DRAG").ok().map(|v| v.split(',').filter_map(|w| w.parse().ok()).collect()).unwrap_or_else(|| vec![last]);
            let stats = |v: &[f64]| {
                let mut s = v.to_vec();
                s.sort_by(|a, b| a.total_cmp(b));
                (s.iter().sum::<f64>() / s.len().max(1) as f64, s[s.len() / 2], *s.last().unwrap_or(&0.0))
            };
            for id in ids {
                h.key(egui::Key::Escape);
                h.key(egui::Key::Num4);
                h.st.selection = vec![id];
                h.key(egui::Key::W);
                h.key(egui::Key::F);
                h.settle();
                let name = h.st.doc.feature(id).map(|f| f.name.clone()).unwrap_or_default();
                // at rest
                let rest: Vec<[f64; 3]> = (0..10).map(|_| h.frame_timed()).collect();
                let Some(pivot) = h.st.pivot() else {
                    eprintln!("  {id} {name}: no pivot");
                    continue;
                };
                let pivot = pivot.as_vec3();
                let v = h.st.view();
                let k = v.pixel_size_at((pivot - v.eye).length()) * 78.0;
                let (a, b) = (h.screen_of(pivot + Vec3::X * k * 0.8), h.screen_of(pivot + Vec3::X * k * 1.8));
                h.move_to(a);
                h.steps(2);
                let hot = h.st.hot_handle;
                h.press(a, true);
                let gen0 = h.st.model_gen;
                let mut frames: Vec<[f64; 3]> = vec![];
                let mut answers: Vec<f64> = vec![];
                let mut seen = gen0;
                let n = 60;
                let t0 = Instant::now();
                for i in 1..=n {
                    let f0 = Instant::now();
                    h.pending.push(Event::PointerMoved(a + (b - a) * (i as f32 / n as f32)));
                    frames.push(h.frame_timed());
                    if h.st.model_gen != seen {
                        seen = h.st.model_gen;
                        answers.push(h.st.round_ms as f64);
                    }
                    // 60 frames a second at most (the kernel works meanwhile)
                    if let Some(left) = Duration::from_millis(16).checked_sub(f0.elapsed()) {
                        std::thread::sleep(left);
                    }
                }
                let took = t0.elapsed().as_secs_f64();
                h.press(b, false);
                let t1 = Instant::now();
                h.settle();
                let fine = t1.elapsed().as_secs_f64();
                let col = |v: &[[f64; 3]], c: usize| stats(&v.iter().map(|f| f[c]).collect::<Vec<_>>());
                let (tk, ui, dr) = (col(&frames, 0), col(&frames, 1), col(&frames, 2));
                let (rt, ru, rd) = (col(&rest, 0), col(&rest, 1), col(&rest, 2));
                eprintln!("  {id} {name} (handle {hot:?}): {n} frames in {took:.2} s ({:.0} a second)", n as f64 / took);
                eprintln!("    at rest   tick {:.1} ms, interface {:.1}, drawing {:.1} (medians)", rt.1, ru.1, rd.1);
                eprintln!("    dragging  tick {:.1} / {:.1} max, interface {:.1} / {:.1} max ({} frames over 8 ms), drawing {:.1} / {:.1} max", tk.1, tk.2, ui.1, ui.2, frames.iter().filter(|f| f[0] + f[1] > 8.0).count(), dr.1, dr.2);
                eprintln!("    kernel    {} answers during the drag ({:.1} a second), each {:?} ms; fine model {:.2} s after the release", answers.len(), answers.len() as f64 / took, answers.iter().map(|a| a.round() as i64).collect::<Vec<_>>(), fine);
                eprintln!("    errors {:?}", h.st.model.as_ref().map(|m| m.errors.len()));
                h.st.undo();
                h.settle();
            }
        }
    }

    if want("heavy") {
        // FORME_DOC = a scene: the tools tried on it as it is (nothing kept),
        // each frame's tick + interface time in ms (the drawing is left out:
        // a software GPU here). Hover and rectangle in each mode, X-ray, the
        // Knife and the Cube over the part, a click, a double-click, Isolate,
        // hide, delete, undo.
        if let Ok(p) = std::env::var("FORME_DOC") {
            let d = Doc::from_json(&std::fs::read_to_string(p).unwrap()).unwrap();
            h.reset(d, "Scene");
            let n0 = h.st.doc.features.len();
            let m = h.st.model.clone();
            eprintln!("  {} features, {} faces, {} triangles, {} edges", n0, m.as_ref().map_or(0, |m| m.faces.len()), m.as_ref().map_or(0, |m| m.triangles.len()), m.as_ref().map_or(0, |m| m.edges.len()));
            let stats = |v: &[f64]| {
                let mut s = v.to_vec();
                s.sort_by(|a, b| a.total_cmp(b));
                (s[s.len() / 2], *s.last().unwrap_or(&0.0))
            };
            let c = h.screen_of(h.st.camera.target);
            let (a, b) = (pos2(c.x - 300.0, c.y - 220.0), pos2(c.x + 300.0, c.y + 220.0));
            // the pointer swept over the part, a frame per move
            let sweep = |h: &mut Harness, n: usize| -> Vec<f64> {
                (0..=n)
                    .map(|i| {
                        h.pending.push(Event::PointerMoved(a + (b - a) * (i as f32 / n as f32)));
                        let f = h.frame_timed();
                        f[0] + f[1]
                    })
                    .collect()
            };
            // a rectangle from the view's corner over the part
            let marquee = |h: &mut Harness, n: usize| -> Vec<f64> {
                let from = pos2(a.x - 60.0, a.y - 40.0);
                h.press(from, true);
                let v: Vec<f64> = (1..=n)
                    .map(|i| {
                        h.pending.push(Event::PointerMoved(from + (b - from) * (i as f32 / n as f32)));
                        let f = h.frame_timed();
                        f[0] + f[1]
                    })
                    .collect();
                let t = Instant::now();
                h.press(b, false);
                h.step();
                let mut v = v;
                v.push(t.elapsed().as_secs_f64() * 1e3);
                v
            };
            for xray in [false, true] {
                if h.st.xray != xray {
                    h.hold(Modifiers::ALT);
                    h.key(egui::Key::X);
                    h.hold(Modifiers::NONE);
                    h.settle();
                }
                for (k, name) in [(egui::Key::F8, "Object"), (egui::Key::F11, "Face"), (egui::Key::F10, "Edge"), (egui::Key::F9, "Vertex")] {
                    h.key(k);
                    h.steps(3);
                    let hv = stats(&sweep(&mut h, 40));
                    let mq = marquee(&mut h, 30);
                    let (release, mv) = (*mq.last().unwrap(), stats(&mq[..mq.len() - 1]));
                    let picked = match name {
                        "Object" => h.st.selection.len(),
                        "Face" => h.st.face_sel.len(),
                        "Edge" => h.st.edge_sel.len(),
                        _ => h.st.vert_sel.len(),
                    };
                    eprintln!("  {name}{}: hover {:.1} ms / {:.1} max · rectangle {:.1} / {:.1} max, release {:.1} ms, {picked} picked", if xray { " (X-ray)" } else { "" }, hv.0, hv.1, mv.0, mv.1, release);
                    h.key(egui::Key::Escape);
                    h.steps(2);
                }
            }
            h.hold(Modifiers::ALT);
            h.key(egui::Key::X);
            h.hold(Modifiers::NONE);
            h.settle();
            // tools over the part
            for (k, name) in [(egui::Key::K, "Knife"), (egui::Key::B, "Cube"), (egui::Key::C, "Cylinder"), (egui::Key::L, "Sketch"), (egui::Key::M, "Forms")] {
                h.key(egui::Key::F8);
                h.key(k);
                h.steps(3);
                let hv = stats(&sweep(&mut h, 40));
                eprintln!("  {name} over the part: {:.1} ms / {:.1} max", hv.0, hv.1);
                h.key(egui::Key::Escape);
                h.key(egui::Key::Q);
                h.steps(2);
            }
            // a click on the part in Object mode, a double-click in Face mode
            h.key(egui::Key::F8);
            h.move_to(c);
            h.steps(2);
            let t = Instant::now();
            h.click(c);
            let click = t.elapsed().as_secs_f64() * 1e3 / 4.0;
            let sel = h.st.selection.clone();
            let rest = stats(&(0..10).map(|_| { let f = h.frame_timed(); f[0] + f[1] }).collect::<Vec<_>>());
            eprintln!("  click in Object mode: {:.1} ms a frame, selected {:?}, then at rest {:.1} ms / {:.1} max", click, sel, rest.0, rest.1);
            h.key(egui::Key::F11);
            let t = Instant::now();
            h.click(c);
            h.click(c);
            let dbl = t.elapsed().as_secs_f64() * 1e3 / 8.0;
            eprintln!("  double-click in Face mode: {:.1} ms a frame, {} faces linked", dbl, h.st.face_sel.len());
            h.key(egui::Key::Escape);
            h.key(egui::Key::F8);
            // Isolate on and off, hide and show, delete and undo: until the
            // kernel's model is there
            if let Some(&id) = sel.first() {
                h.st.selection = vec![id];
                h.steps(2);
                let timed = |h: &mut Harness, what: &str, f: &mut dyn FnMut(&mut Harness)| {
                    let t = Instant::now();
                    f(h);
                    h.step();
                    let first = t.elapsed().as_secs_f64() * 1e3;
                    h.settle();
                    eprintln!("  {what}: first frame {:.0} ms, the kernel's model after {:.0} ms ({} errors)", first, t.elapsed().as_secs_f64() * 1e3, h.st.model.as_ref().map_or(0, |m| m.errors.len()));
                };
                timed(&mut h, "Isolate on", &mut |h| h.st.toggle_isolate());
                timed(&mut h, "Isolate off", &mut |h| h.st.toggle_isolate());
                timed(&mut h, "Duplicate", &mut |h| h.st.duplicate_selected());
                timed(&mut h, "undo", &mut |h| h.st.undo_doc());
                h.st.selection = vec![id];
                timed(&mut h, "Delete", &mut |h| h.st.delete_selected());
                timed(&mut h, "undo", &mut |h| h.st.undo_doc());
                timed(&mut h, "redo", &mut |h| h.st.redo_doc());
                timed(&mut h, "undo", &mut |h| h.st.undo_doc());
            }
            eprintln!("  features {} (were {n0})", h.st.doc.features.len());
        }
    }

    if want("isopc") {
        // his scene of 10 h 50 (FORME_DOC), his camera, the Pill picked,
        // Ctrl + I with the pointer over the view
        if let Ok(p) = std::env::var("FORME_DOC") {
            let d = Doc::from_json(&std::fs::read_to_string(p).unwrap()).unwrap();
            h.reset(d, "Scene");
            h.st.camera.target = Vec3::new(37.909, 52.647, -120.267);
            h.st.camera.dist = 511.876;
            h.st.camera.pitch = 7.614f32.to_radians();
            h.st.camera.yaw = 10.415f32.to_radians();
            h.steps(2);
            h.settle();
            h.st.selection = vec![107];
            h.move_to(pos2(900.0, 500.0));
            let cam0 = h.st.camera;
            for k in 0..4 {
                h.hold(Modifiers::CTRL);
                h.key(egui::Key::I);
                h.hold(Modifiers::NONE);
                for _ in 0..30 {
                    h.move_to(pos2(900.0 + k as f32, 500.0));
                }
                eprintln!("  isolate {}: camera {:?} {} (from {:?} {}) anim {}", h.st.isolate.is_some(), h.st.camera.target, h.st.camera.dist, cam0.target, cam0.dist, h.st.cam_anim.is_some());
            }
        }
    }
    if want("isolate") {
        // Isolate (Romain 20 nov., 9 h 38, Maya): a block beside the test
        // part picked, the top bar's icon: it alone, framed; again: all
        let mut d = test_part();
        let b = d.add(Op::Add, Prim::Box { x: 20.0, y: 15.0, z: 20.0 }, glam::DVec3::new(70.0, 0.0, 0.0), glam::DQuat::IDENTITY);
        h.reset(d, "Part");
        h.st.camera.frame(Vec3::new(-40.0, 0.0, -30.0), Vec3::new(85.0, 30.0, 30.0));
        h.steps(2);
        h.settle();
        h.click(h.screen_of(Vec3::new(70.0, 15.0, 0.0)));
        eprintln!("  selected {:?} (block {b})", h.st.selection);
        let cam0 = h.st.camera;
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::I);
        h.hold(Modifiers::NONE);
        eprintln!("  at once: faces {} (animating {})", h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.cam_anim.is_some());
        h.steps(6);
        h.shot("zero-isolate-0-at-once");
        h.settle();
        // (Romain 21 nov., 0 h 11: "NE PAS BOUGER LA CAMERA, pas de focus")
        let still = |a: &nurbs_app::camera::OrbitCamera, b: &nurbs_app::camera::OrbitCamera| (a.target - b.target).length() < 1e-4 && (a.dist - b.dist).abs() < 1e-4 && (a.yaw - b.yaw).abs() < 1e-5 && (a.pitch - b.pitch).abs() < 1e-5;
        eprintln!("  camera {:?} {} → {:?} {} (unmoved: {})", cam0.target, cam0.dist, h.st.camera.target, h.st.camera.dist, still(&cam0, &h.st.camera));
        eprintln!("  isolated {:?}: faces {}", h.st.isolate.as_ref().map(|i| i.ids.clone()), h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0));
        h.shot("zero-isolate-1-on");
        // the part's block picked by a face (Face mode), isolated with its hole, boss, slot
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::Num1);
        h.hold(Modifiers::NONE);
        h.settle();
        h.st.camera.frame(Vec3::new(-40.0, 0.0, -30.0), Vec3::new(85.0, 30.0, 30.0));
        h.steps(2);
        h.click(h.screen_of(Vec3::new(-20.0, 10.0, 10.0)));
        eprintln!("  selected {:?}", h.st.selection);
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::Num1);
        h.hold(Modifiers::NONE);
        h.settle();
        eprintln!("  isolated {:?}: faces {}", h.st.isolated_ids(), h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0));
        h.shot("zero-isolate-2-part");
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::I);
        h.hold(Modifiers::NONE);
        eprintln!("  off at once: faces {}", h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0));
        h.settle();
        eprintln!("  off: faces {}", h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0));
        h.shot("zero-isolate-3-off");
        eprintln!("  camera after {:?} {}", h.st.camera.target, h.st.camera.dist);
        // F in Edge mode: the edge picked framed
        h.key(egui::Key::Num2);
        h.click(h.screen_of(Vec3::new(80.0, 20.0, 0.0)));
        eprintln!("  edges picked {}", h.st.edge_sel.len());
        h.key(egui::Key::F);
        h.settle();
        eprintln!("  F on the edge: {:?} {}", h.st.camera.target, h.st.camera.dist);
        h.shot("zero-isolate-4-f-edge");
        h.key(egui::Key::Num4);
        h.key(egui::Key::Escape);
        h.st.selection.clear();
        h.key(egui::Key::F);
        h.settle();
        eprintln!("  F on nothing: {:?} {}", h.st.camera.target, h.st.camera.dist);
    }

    if want("edges-depth") {
        // edges seen a little behind a face near its outline (Romain 20
        // nov., 9 h 30, his stacked pills with a groove): stacked pills, a
        // groove, a thin pill on top, seen from above and in front
        let mut d = Doc::default();
        let y = |d: &mut Doc, op: Op, x: f64, h: f64, z: f64, at: DVec3| {
            d.add(op, Prim::Box { x, y: h, z }, at, DQuat::IDENTITY);
            if op == Op::Add {
                for s in [-1.0, 1.0] {
                    d.add(op, Prim::Cylinder { radius: z * 0.5, height: h }, at + DVec3::new(s * x * 0.5, 0.0, 0.0), DQuat::IDENTITY);
                }
            }
        };
        y(&mut d, Op::Add, 80.0, 10.0, 30.0, DVec3::ZERO);
        y(&mut d, Op::Add, 76.0, 14.0, 22.0, DVec3::new(0.0, 10.0, 0.0));
        y(&mut d, Op::Carve, 70.0, 6.0, 5.0, DVec3::new(4.0, 19.0, 0.0));
        y(&mut d, Op::Add, 4.0, 20.0, 5.0, DVec3::new(-36.0, 24.0, -6.0));
        y(&mut d, Op::Add, 60.0, 30.0, 16.0, DVec3::new(10.0, 0.0, -40.0));
        h.reset(d, "Pills");
        h.st.camera.frame(Vec3::new(-45.0, 0.0, -50.0), Vec3::new(10.0, 45.0, 15.0));
        h.st.camera.yaw = -0.55;
        h.st.camera.pitch = 0.45;
        h.st.camera.dist *= 0.55;
        h.steps(3);
        h.settle();
        h.shot("zero-edges-depth-1");
        h.st.camera.yaw = 0.35;
        h.st.camera.pitch = 0.25;
        h.steps(3);
        h.shot("zero-edges-depth-2");
    }

    if want("align") {
        // Romain (20 nov. 7 h 22): a point of the moved piece, not only its
        // centre, lands on a point of interest it passes near
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, glam::DVec3::ZERO, glam::DQuat::IDENTITY);
        let b = d.add(Op::Add, Prim::Box { x: 10.0, y: 20.0, z: 30.0 }, glam::DVec3::new(69.63, 0.0, 0.0), glam::DQuat::IDENTITY);
        h.reset(d, "Cube");
        h.st.camera.frame(Vec3::new(-25.0, 0.0, -20.0), Vec3::new(80.0, 25.0, 20.0));
        h.steps(2);
        h.settle();
        // the pointer grabs the arrow, goes `by` along it, held for a shot
        let arrow = |h: &mut Harness, axis: Vec3, by: f32, name: &str| {
            let c = h.st.pivot().expect("a gizmo").as_vec3();
            let s0 = h.screen_of(c);
            let dir = (h.screen_of(c + axis) - s0).normalized();
            let grab = s0 + dir * 55.0;
            let to = h.screen_of(c + axis * by) - s0;
            h.drag(grab, grab + to, 10, false);
            h.steps(3);
            eprintln!("  snapped {:?}", h.st.snap_shown.as_ref().map(|s| (s.kind, s.pos, s.source)));
            h.shot(name);
            h.press(grab + to, false);
            h.steps(2);
            h.settle();
        };
        // Object mode: the small block along -X, its -X corners onto the
        // big one's +X corners (x 20): moved by -44.63, off the grid
        h.click(h.screen_of(Vec3::new(69.63, 20.0, 0.0)));
        eprintln!("  selected {:?}", h.st.selection);
        arrow(&mut h, Vec3::X, -44.0, "zero-align-1-object");
        eprintln!("  block at x {:?} (want 25)", h.st.doc.feature(b).map(|f| f.origin.x));
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::Z);
        h.hold(Modifiers::NONE);
        h.settle();
        eprintln!("  undone: block at x {:?}", h.st.doc.feature(b).map(|f| f.origin.x));
        // Face mode: the big block's top, +X and +Z faces copied, the copy
        // along +X: its +X corners onto the small block's -X ones (x 64.63)
        h.key(egui::Key::Escape);
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(0.0, 20.0, 0.0)));
        h.hold(Modifiers::SHIFT);
        h.click(h.screen_of(Vec3::new(20.0, 10.0, 0.0)));
        h.click(h.screen_of(Vec3::new(0.0, 10.0, 15.0)));
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::D);
        h.hold(Modifiers::NONE);
        h.settle();
        arrow(&mut h, Vec3::X, 46.0, "zero-align-2-copy");
        eprintln!(
            "  copy moved by {:?} (want x 44.63), errors {:?}",
            h.st.doc.features.last().and_then(|f| f.tweak.as_ref()).map(|t| t.delta),
            h.st.model.as_ref().map(|m| m.errors.clone())
        );
    }

    if want("surf-curve") {
        // a clean closed curve (Romain 20 nov., 13 h 05): the Path tool in
        // CURVE, five points on the ground, one made sharp, closed on its
        // first point; filled (Fill); a point moved with the gizmo in
        // Vertex mode; the Surface tool on its plane: a curve filled
        h.reset(Doc::default(), "Curves");
        h.st.camera.frame(Vec3::new(-40.0, 0.0, -30.0), Vec3::new(60.0, 5.0, 30.0));
        h.st.camera.pitch = 0.95;
        h.steps(2);
        h.key(egui::Key::P);
        h.st.surface_curve = true;
        h.steps(1);
        let g = |h: &Harness, x: f32, z: f32| h.screen_of(Vec3::new(x, 0.0, z));
        let pts = [(-30.0, 10.0), (-10.0, -22.0), (15.0, -14.0), (12.0, 18.0), (-14.0, 22.0)];
        for (x, z) in pts {
            let q = g(&h, x, z);
            h.move_to(q);
            h.click(q);
        }
        // the second point made sharp (Shift + click on it)
        h.hold(Modifiers::SHIFT);
        let q = g(&h, -10.0, -22.0);
        h.move_to(q);
        h.click(q);
        h.hold(Modifiers::NONE);
        let q = g(&h, -30.0, 10.0);
        h.move_to(q);
        h.steps(2);
        eprintln!("  sharp {:?}", h.st.sketch.as_ref().map(|k| k.sharp.clone()));
        h.shot("zero-surf-curve-1-drawing");
        h.click(q);
        h.settle();
        let id = h.st.doc.features.last().map(|f| f.id).unwrap();
        eprintln!("  made: {:?}", h.st.doc.feature(id).map(|f| (f.name.clone(), f.profile.as_ref().map(|p| (p.spline, p.open, p.smooth.clone())))));
        h.shot("zero-surf-curve-2-curve");
        h.st.set_curve_make(id, nurbs_app::surfaces::CurveMake::Fill);
        h.settle();
        eprintln!("  filled: faces {:?} edges {:?} errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()), h.st.model.as_ref().map(|m| m.edges.len()), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-surf-curve-3-filled");
        // Vertex mode: the fourth point picked and moved 12 mm along X
        h.key(egui::Key::Num1);
        h.steps(1);
        let v = nurbs_app::surfaces::path_point_pos(h.st.doc.feature(id).unwrap(), 3).unwrap();
        let q = h.screen_of(v.as_vec3());
        h.click(q);
        h.steps(2);
        eprintln!("  picked {:?} path points {:?} pivot {:?}", h.st.vert_sel.len(), h.st.picked_path_points(), h.st.pivot());
        let at = h.screen_of(h.st.pivot().unwrap().as_vec3());
        h.st.begin_drag(nurbs_app::state::Handle::Axis(0), [at.x, at.y]);
        let to = h.screen_of((v + DVec3::new(12.0, 0.0, 0.0)).as_vec3());
        h.st.update_drag([to.x, to.y]);
        h.st.end_drag();
        h.settle();
        eprintln!("  moved: point {:?} errors {:?}", nurbs_app::surfaces::path_point_pos(h.st.doc.feature(id).unwrap(), 3), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-surf-curve-4-moved");
        // the Surface tool on its plane, LINEAR, a corner filleted: a curve
        // filled, editable
        h.key(egui::Key::Num4);
        h.st.selection.clear();
        h.key(egui::Key::U);
        h.st.surface_curve = false;
        h.steps(1);
        for (x, z) in [(30.0, -15.0), (55.0, -15.0), (55.0, 15.0), (30.0, 15.0)] {
            let q = g(&h, x, z);
            h.move_to(q);
            h.click(q);
        }
        h.hold(Modifiers::CTRL);
        h.drag(g(&h, 55.0, 15.0), g(&h, 55.0, 23.0), 8, false);
        h.steps(2);
        h.press(g(&h, 55.0, 23.0), false);
        h.hold(Modifiers::NONE);
        h.key(egui::Key::Enter);
        h.settle();
        let s = h.st.doc.features.last().cloned().unwrap();
        eprintln!("  surface: {} fill {} makes {:?} corners {:?} errors {:?}", s.name, s.fill, h.st.doc.makes(&s), s.profile.as_ref().map(|p| p.corner.clone()), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-surf-curve-5-surface");
    }

    if want("surf-revolve") {
        // Revolve (3ds Max's Lathe): a bottle's half profile drawn in
        // CURVE, its two ends on the axis (the box's left side), turned
        // into a solid; half a turn; its axis along X; a ring carved from
        // a block
        h.reset(Doc::default(), "Revolve");
        h.st.camera.frame(Vec3::new(-10.0, 0.0, -45.0), Vec3::new(30.0, 5.0, 5.0));
        h.st.camera.pitch = 1.3;
        h.st.camera.yaw = 0.0;
        h.steps(2);
        h.key(egui::Key::P);
        h.st.surface_curve = true;
        h.steps(1);
        let g = |h: &Harness, x: f32, z: f32| h.screen_of(Vec3::new(x, 0.0, z));
        let prof = [(0.0, 0.0), (10.0, 0.0), (10.0, -6.0), (6.0, -16.0), (7.0, -28.0), (4.0, -33.0), (4.0, -40.0), (0.0, -40.0)];
        for (x, z) in prof {
            let q = g(&h, x, z);
            h.move_to(q);
            h.click(q);
        }
        h.hold(Modifiers::SHIFT);
        for (x, z) in [(10.0, 0.0), (4.0, -40.0)] {
            let q = g(&h, x, z);
            h.move_to(q);
            h.click(q);
        }
        h.hold(Modifiers::NONE);
        h.shot("zero-surf-revolve-1-profile");
        h.key(egui::Key::Enter);
        h.settle();
        let id = h.st.doc.features.last().map(|f| f.id).unwrap();
        h.st.set_curve_make(id, nurbs_app::surfaces::CurveMake::Revolve);
        h.settle();
        let f = h.st.doc.feature(id).cloned().unwrap();
        eprintln!("  revolve: closes {} makes {:?} faces {:?} errors {:?}", f.revolve_closes(), h.st.doc.makes(&f), h.st.model.as_ref().map(|m| m.faces.len()), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-surf-revolve-2-top");
        h.st.camera.yaw = 0.6;
        h.st.camera.pitch = 0.35;
        h.st.camera.frame(Vec3::new(-12.0, -12.0, -42.0), Vec3::new(12.0, 12.0, 2.0));
        h.steps(2);
        h.shot("zero-surf-revolve-3-solid");
        let r = h.st.doc.feature(id).and_then(|f| f.revolve).unwrap();
        h.st.set_revolve(id, nurbs_kernel::Revolve { angle: 200.0, ..r }, "Revolve angle", true);
        h.st.end_live();
        h.settle();
        eprintln!("  200°: faces {:?} errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-surf-revolve-4-part");
        h.st.set_revolve(id, nurbs_kernel::Revolve { angle: 360.0, along_x: true, align: 1 }, "Revolve axis", true);
        h.st.end_live();
        h.settle();
        eprintln!("  axis X at its max side: makes {:?} faces {:?} errors {:?}", h.st.doc.makes(h.st.doc.feature(id).unwrap()), h.st.model.as_ref().map(|m| m.faces.len()), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.st.camera.frame(Vec3::new(-15.0, -45.0, -50.0), Vec3::new(15.0, 45.0, 10.0));
        h.steps(2);
        h.shot("zero-surf-revolve-5-axis-x");
    }

    if want("surf-loft") {
        // Loft: a rounded rectangle on the ground, a circle 20 mm up, a
        // star 40 mm up: picked in that order, Loft (a solid); ruled; the
        // circle made bigger (the loft follows)
        use nurbs_kernel::doc::CurveKind;
        h.reset(Doc::default(), "Loft");
        h.st.camera.frame(Vec3::new(-25.0, 0.0, -25.0), Vec3::new(25.0, 45.0, 25.0));
        h.st.camera.pitch = 0.45;
        h.st.camera.yaw = 0.6;
        h.steps(2);
        let w = |x: f32, z: f32| Vec3::new(x, 0.0, z);
        let mut ids = vec![];
        for (k, a, b) in [(CurveKind::Rectangle, w(-18.0, -12.0), w(18.0, 12.0)), (CurveKind::Circle, w(0.0, 0.0), w(10.0, 0.0)), (CurveKind::Star, w(0.0, 0.0), w(9.0, 0.0))] {
            h.st.arm_curve(k);
            h.steps(1);
            let (pa, pb) = (h.screen_of(a), h.screen_of(b));
            h.drag(pa, pb, 8, false);
            h.steps(1);
            h.press(pb, false);
            h.steps(2);
            ids.push(h.st.doc.features.last().unwrap().id);
        }
        h.st.edit("Lift", |d| {
            if let Some(c) = d.feature_mut(ids[0]).and_then(|f| f.curve.as_mut()) {
                c.corner = 4.0;
            }
            d.feature_mut(ids[1]).unwrap().origin.y = 20.0;
            d.feature_mut(ids[2]).unwrap().origin.y = 40.0;
        });
        h.st.selection = ids.clone();
        h.steps(3);
        h.shot("zero-surf-loft-1-curves");
        h.st.make_loft();
        h.settle();
        let l = h.st.selection[0];
        eprintln!("  loft: makes {:?} faces {:?} errors {:?}", h.st.doc.makes(h.st.doc.feature(l).unwrap()), h.st.model.as_ref().map(|m| m.faces.len()), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-surf-loft-2-solid");
        h.st.set_loft_ruled(l, true);
        h.settle();
        eprintln!("  ruled: faces {:?} errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-surf-loft-3-ruled");
        h.st.set_loft_ruled(l, false);
        h.st.edit("Radius", |d| {
            let f = d.feature_mut(ids[1]).unwrap();
            let (Some(c), nurbs_kernel::Prim::Path { .. }) = (f.curve, f.prim) else { return };
            f.reshape_curve(c, 16.0);
        });
        h.st.selection = vec![l];
        h.settle();
        eprintln!("  circle grown: errors {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-surf-loft-4-follows");
    }

    if want("forms") {
        // Forms (Romain 20 nov., 13 h 05: "le feeling… Nomad ou ZBrush…
        // une move brush"): M, a click on the ground makes a sphere; the
        // Move brush pulls its side out (shot during the stroke: the app's
        // own drawing of it); Mirror X and Move up: two ears; Inflate the
        // front; Shift: Smooth; a plain Cylinder pulled (it becomes a
        // form); More detail and a small Pinch; a Cube added through a form
        h.reset(Doc::default(), "Forms");
        let view = |h: &mut Harness| {
            h.st.camera.yaw = 0.35;
            h.st.camera.pitch = 0.35;
            h.st.camera.frame(Vec3::new(-24.0, 0.0, -20.0), Vec3::new(30.0, 38.0, 20.0));
            h.steps(2);
        };
        view(&mut h);
        h.key(egui::Key::M);
        let o = h.screen_of(Vec3::ZERO);
        h.move_to(o);
        h.click(o);
        h.settle();
        let id = h.st.doc.features.last().map(|f| f.id).unwrap();
        let info = |h: &Harness| {
            let f = h.st.doc.feature(id).unwrap();
            let fm = f.form.as_ref().unwrap();
            format!("{} {:?} {}×{} box {:?} faces {:?} errors {:?}", f.name, f.op, fm.nu, fm.nv, f.prim, h.st.model.as_ref().map(|m| m.faces.len()), h.st.model.as_ref().map(|m| m.errors.clone()))
        };
        eprintln!("  new: {}", info(&h));
        // a point of the sphere (centre 0, 15, 0, radius 15) along a direction
        let on = |h: &Harness, d: Vec3| h.screen_of(Vec3::new(0.0, 15.0, 0.0) + d.normalize() * 14.0);
        let p = on(&h, Vec3::new(0.9, 0.0, 0.45));
        h.move_to(p);
        h.steps(2);
        h.shot("zero-forms-1-sphere");
        // Move: the right side pulled out
        let t0 = Instant::now();
        h.drag(p, p + vec2(150.0, -20.0), 10, false);
        h.steps(1);
        eprintln!("  stroke of 10 moves: {:?}, preview {}", t0.elapsed(), h.st.form_preview().is_some());
        h.shot("zero-forms-2-move-live");
        h.press(p + vec2(150.0, -20.0), false);
        let t0 = Instant::now();
        h.settle();
        eprintln!("  move: {} · fine model after {:?}", info(&h), t0.elapsed());
        h.shot("zero-forms-3-moved");
        // Mirror X: a point up on the left pulled up, both sides follow
        h.st.form.mirror = true;
        h.st.form.radius = 60.0;
        let p = on(&h, Vec3::new(-0.45, 0.8, 0.4));
        h.move_to(p);
        h.steps(2);
        h.shot("zero-forms-4-mirror-ring");
        h.drag(p, p + vec2(-10.0, -90.0), 10, true);
        h.settle();
        eprintln!("  mirror: {}", info(&h));
        h.move_to(p + vec2(-200.0, 200.0));
        h.steps(2);
        h.shot("zero-forms-5-mirror");
        // Inflate the front, then Shift: Smooth where the side was pulled
        h.st.form.mirror = false;
        h.st.form.radius = 70.0;
        h.st.form.brush = nurbs_app::forms::Brush::Inflate;
        let p = on(&h, Vec3::new(-0.2, -0.25, 1.0));
        h.drag(p, p + vec2(0.0, -50.0), 8, true);
        h.settle();
        eprintln!("  inflate: {}", info(&h));
        h.shot("zero-forms-6-inflate");
        // on the pulled side, half-way to its tip (the first stroke's press
        // moved along it, same view)
        let p = on(&h, Vec3::new(0.9, 0.0, 0.45)) + vec2(80.0, -12.0);
        h.st.form.radius = 110.0;
        h.st.form.intensity[2] = 0.8;
        h.hold(Modifiers::SHIFT);
        h.drag(p, p + vec2(0.0, 300.0), 12, true);
        h.hold(Modifiers::NONE);
        h.settle();
        eprintln!("  smooth: {} · undo: {:?}", info(&h), h.st.undo_labels().iter().rev().take(6).collect::<Vec<_>>());
        h.shot("zero-forms-7-smooth");
        // a plain Cylinder beside it: the Move brush makes it a form
        let mut cyl = 0;
        h.st.begin_live("Cylinder");
        h.st.live(|d| cyl = d.add(Op::Add, nurbs_kernel::Prim::Cylinder { radius: 10.0, height: 30.0 }, DVec3::new(48.0, 0.0, 0.0), glam::DQuat::IDENTITY));
        h.st.end_live();
        h.st.camera.frame(Vec3::new(-24.0, 0.0, -20.0), Vec3::new(64.0, 45.0, 20.0));
        h.st.form.brush = nurbs_app::forms::Brush::Move;
        h.st.form.radius = 50.0;
        h.settle();
        let k = h.screen_of(Vec3::new(48.0, 27.0, 9.0));
        h.drag(k, k + vec2(60.0, -50.0), 8, true);
        h.settle();
        let f = h.st.doc.feature(cyl).unwrap();
        eprintln!("  cylinder: form {:?} errors {:?}", f.form.as_ref().map(|m| (m.kind, m.nu, m.nv)), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-forms-8-cylinder");
        // More detail on the sphere, then a small Pinch across its top
        h.st.selection = vec![id];
        h.st.form_detail(id);
        h.settle();
        eprintln!("  detail: {}", info(&h));
        h.st.form.brush = nurbs_app::forms::Brush::Flatten;
        h.st.form.radius = 55.0;
        h.st.form.intensity[3] = 1.0;
        view(&mut h);
        let p = on(&h, Vec3::new(-0.3, 0.0, 1.0));
        h.drag(p, p + vec2(0.0, 160.0), 8, true);
        h.settle();
        eprintln!("  flatten: {}", info(&h));
        h.st.form.brush = nurbs_app::forms::Brush::Pinch;
        h.st.form.radius = 45.0;
        let p = on(&h, Vec3::new(0.0, 0.75, 0.66));
        h.drag(p, p + vec2(0.0, 150.0), 8, true);
        h.settle();
        eprintln!("  pinch: {}", info(&h));
        h.st.camera.yaw = -0.25;
        h.steps(2);
        h.move_to(p + vec2(-300.0, 200.0));
        h.steps(2);
        h.shot("zero-forms-9-detail-flatten-pinch");
        // a Cube added through the form: one part
        h.key(egui::Key::Q);
        let mut b = 0;
        h.st.begin_live("Cube");
        h.st.live(|d| b = d.add(Op::Add, nurbs_kernel::Prim::Box { x: 60.0, y: 8.0, z: 24.0 }, DVec3::new(5.0, 2.0, 0.0), glam::DQuat::IDENTITY));
        h.st.end_live();
        h.st.selection.clear();
        h.st.camera.frame(Vec3::new(-35.0, 0.0, -30.0), Vec3::new(70.0, 45.0, 30.0));
        h.settle();
        eprintln!("  + cube {b}: faces {:?} errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-forms-10-with-cube");
    }

    if want("curves") {
        // the Curves set (Romain 20 nov. 8 h 08, 3ds Max's Shapes): each
        // shape dragged on the ground, then its settings changed
        use nurbs_kernel::doc::CurveKind;
        h.reset(Doc::default(), "Curves");
        h.st.camera.frame(Vec3::new(-65.0, 0.0, -25.0), Vec3::new(115.0, 5.0, 25.0));
        h.st.camera.pitch = 1.1;
        h.steps(2);
        let say = |h: &Harness, what: &str| {
            for f in h.st.doc.features.iter() {
                if let (Some(c), nurbs_kernel::Prim::Path { x, z }) = (f.curve, f.prim) {
                    eprintln!("  {what}: {} {:?} {:.2} x {:.2} at ({:.2}, {:.2}) r {:.2} pts {}", f.name, c.kind, x, z, f.origin.x, f.origin.z, c.radius(x, z), f.profile.as_ref().map(|p| p.points.len()).unwrap_or(0));
                }
            }
        };
        let w = |x: f32, z: f32| Vec3::new(x, 0.0, z);
        for (k, a, b) in [
            (CurveKind::Rectangle, w(-60.0, -10.0), w(-30.0, 10.0)),
            (CurveKind::Circle, w(-10.0, 0.0), w(0.0, 0.0)),
            (CurveKind::Ellipse, w(10.0, -8.0), w(40.0, 8.0)),
            (CurveKind::Polygon, w(60.0, 0.0), w(72.0, 0.0)),
            (CurveKind::Star, w(95.0, 0.0), w(107.0, 0.0)),
        ] {
            h.st.arm_curve(k);
            h.steps(1);
            let (pa, pb) = (h.screen_of(a), h.screen_of(b));
            h.drag(pa, pb, 8, false);
            h.steps(1);
            if k == CurveKind::Polygon {
                h.shot("zero-curves-1-drawing");
            }
            h.press(pb, false);
            h.steps(2);
        }
        say(&h, "drawn");
        h.shot("zero-curves-2-drawn");
        // settings: the rectangle's corners, a pentagon, a star of 6
        let ids: Vec<_> = h.st.doc.features.iter().map(|f| f.id).collect();
        h.st.edit("Corner radius", |d| {
            if let Some(c) = d.feature_mut(ids[0]).and_then(|f| f.curve.as_mut()) {
                c.corner = 4.0;
            }
        });
        h.st.edit("Sides", |d| {
            let f = d.feature_mut(ids[3]).unwrap();
            let (Some(mut c), nurbs_kernel::Prim::Path { x, z }) = (f.curve, f.prim) else { return };
            let r = c.radius(x, z);
            c.sides = 5;
            c.corner = 2.0;
            f.reshape_curve(c, r);
        });
        h.st.edit("Points", |d| {
            let f = d.feature_mut(ids[4]).unwrap();
            let (Some(mut c), nurbs_kernel::Prim::Path { x, z }) = (f.curve, f.prim) else { return };
            let r = c.radius(x, z);
            c.sides = 6;
            c.inner = 0.6;
            c.corner2 = 1.5;
            f.reshape_curve(c, r);
        });
        // the circle stretched: an ellipse
        h.st.edit("Size", |d| {
            if let Some(nurbs_kernel::Prim::Path { x, .. }) = d.feature_mut(ids[1]).map(|f| &mut f.prim) {
                *x = 26.0;
            }
        });
        h.steps(2);
        say(&h, "set");
        h.st.selection = vec![ids[3]];
        h.steps(3);
        h.shot("zero-curves-3-set");
    }

    if want("curves-bool") {
        // step 2 of the Curves set (Romain 20 nov. 8 h 08: "une soustraction
        // des 2"): a rectangle less two circles, kept live
        use nurbs_kernel::doc::{CurveKind, CurveOp};
        h.reset(Doc::default(), "Curves");
        h.st.camera.frame(Vec3::new(-35.0, 0.0, -22.0), Vec3::new(45.0, 5.0, 22.0));
        h.st.camera.pitch = 1.1;
        h.steps(2);
        let w = |x: f32, z: f32| Vec3::new(x, 0.0, z);
        for (k, a, b) in [(CurveKind::Rectangle, w(-25.0, -12.0), w(15.0, 12.0)), (CurveKind::Circle, w(15.0, 0.0), w(25.0, 0.0)), (CurveKind::Circle, w(-10.0, 0.0), w(-4.0, 0.0))] {
            h.st.arm_curve(k);
            h.steps(1);
            let (pa, pb) = (h.screen_of(a), h.screen_of(b));
            h.drag(pa, pb, 8, false);
            h.steps(1);
            h.press(pb, false);
            h.steps(2);
        }
        let ids: Vec<_> = h.st.doc.features.iter().map(|f| f.id).collect();
        // picked by clicks: the rectangle's left side, then Shift: the
        // circles' far sides
        h.click(h.screen_of(w(-25.0, 4.0)));
        h.hold(Modifiers::SHIFT);
        h.click(h.screen_of(w(25.0, 0.0)));
        h.click(h.screen_of(w(-4.0, 0.0)));
        h.hold(Modifiers::NONE);
        h.steps(3);
        eprintln!("  picked {:?} (curves {ids:?})", h.st.selection);
        h.shot("zero-curves-bool-1-picked");
        h.st.make_curve_bool(CurveOp::Subtract);
        h.settle();
        let b = *h.st.selection.first().unwrap();
        let say = |h: &Harness, what: &str| {
            let f = h.st.doc.feature(b).unwrap();
            let lines = h.st.curve_lines(f);
            let (lo, hi) = lines.iter().flatten().fold((glam::DVec3::splat(1e9), glam::DVec3::splat(-1e9)), |(a, c), p| (a.min(*p), c.max(*p)));
            eprintln!("  {what}: {} {:?} {} lines, x {:.2} to {:.2}, z {:.2} to {:.2}, errors {:?}", f.name, f.shape_bool.as_ref().map(|s| s.op), lines.len(), lo.x, hi.x, lo.z, hi.z, h.st.model.as_ref().map(|m| m.errors.clone()));
        };
        say(&h, "subtract");
        h.shot("zero-curves-bool-2-subtract");
        // a click again on its line: the curve under it (the small circle)
        h.click(h.screen_of(w(-4.0, 0.0)));
        eprintln!("  drilled into {:?}", h.st.selection);
        // moved: the result follows
        h.st.edit("Move", |d| {
            if let Some(f) = d.feature_mut(ids[2]) {
                f.origin.z += 4.0;
            }
        });
        h.settle();
        say(&h, "small circle moved");
        h.shot("zero-curves-bool-3-operand");
        for op in [CurveOp::Union, CurveOp::Intersect] {
            h.st.selection = vec![b];
            h.st.set_curve_bool_op(b, op);
            h.settle();
            say(&h, op.label());
            h.shot(&format!("zero-curves-bool-4-{}", op.label().to_lowercase()));
        }
        // the boolean moved with the gizmo: its curves go with it
        h.st.set_curve_bool_op(b, CurveOp::Subtract);
        h.st.selection = vec![b];
        h.st.gizmo = nurbs_app::state::GizmoMode::Move;
        h.steps(2);
        let c = h.st.pivot().expect("a gizmo").as_vec3();
        let s0 = h.screen_of(c);
        let dir = (h.screen_of(c + Vec3::X) - s0).normalized();
        let grab = s0 + dir * 55.0;
        let to = h.screen_of(c + Vec3::X * 20.0) - s0;
        h.drag(grab, grab + to, 10, true);
        h.settle();
        for id in &ids {
            let f = h.st.doc.feature(*id).unwrap();
            eprintln!("  after the move: {} at ({:.2}, {:.2})", f.name, f.origin.x, f.origin.z);
        }
        say(&h, "moved");
        h.shot("zero-curves-bool-5-moved");
        // deleted: its curves go too; undone
        h.st.delete_selected();
        eprintln!("  deleted: {} features left", h.st.doc.features.len());
        h.st.undo_doc();
        h.settle();
        eprintln!("  undone: {} features", h.st.doc.features.len());
    }

    if want("curves-extrude") {
        // step 3 of the Curves set: a Subtract pulled up beside a block
        // (Add), a circle drawn on the block's top pulled down (Carve), a
        // star pulled up then given more points
        use nurbs_kernel::doc::{CurveKind, CurveOp};
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 40.0 }, glam::DVec3::ZERO, glam::DQuat::IDENTITY);
        h.reset(d, "Curves");
        h.st.camera.frame(Vec3::new(-30.0, 0.0, -25.0), Vec3::new(95.0, 25.0, 25.0));
        h.steps(2);
        h.settle();
        let w = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let draw = |h: &mut Harness, k: CurveKind, a: Vec3, b: Vec3| -> nurbs_kernel::FeatureId {
            h.st.arm_curve(k);
            h.steps(1);
            let (pa, pb) = (h.screen_of(a), h.screen_of(b));
            h.drag(pa, pb, 8, false);
            h.steps(1);
            h.press(pb, false);
            h.steps(2);
            *h.st.selection.first().unwrap()
        };
        let pull = |h: &mut Harness, base: Vec3, up: Vec3, by: f32| {
            let s0 = h.screen_of(base);
            h.move_to(s0);
            h.hold(Modifiers::CTRL);
            h.key(egui::Key::E);
            h.hold(Modifiers::NONE);
            let to = h.screen_of(base + up * by);
            for i in 1..=8 {
                h.move_to(s0 + (to - s0) * (i as f32 / 8.0));
            }
            h.steps(2);
        };
        let r = draw(&mut h, CurveKind::Rectangle, w(35.0, 0.0, -12.0), w(65.0, 0.0, 12.0));
        let c = draw(&mut h, CurveKind::Circle, w(65.0, 0.0, 0.0), w(73.0, 0.0, 0.0));
        h.st.selection = vec![r, c];
        h.st.make_curve_bool(CurveOp::Subtract);
        let b = h.st.selection[0];
        h.settle();
        pull(&mut h, w(54.0, 0.0, 0.0), Vec3::Y, 15.0);
        h.settle();
        h.shot("zero-curves-extrude-1-pulling");
        h.click(h.st.view().project(w(54.0, 15.0, 0.0)).map(|p| pos2(p[0], p[1])).unwrap());
        h.settle();
        // the circle on the block's top, Carve, down 8
        h.st.mode = Op::Carve;
        let hole = draw(&mut h, CurveKind::Circle, w(0.0, 20.0, 0.0), w(8.0, 20.0, 0.0));
        h.settle();
        pull(&mut h, w(0.0, 20.0, 0.0), -Vec3::Y, 8.0);
        h.st.finish_curve_pull();
        h.settle();
        h.st.mode = Op::Add;
        // a star pulled 6 up, then 7 points
        // with the Extrude tool (no Extrude of its own on a curve, 20 nov.
        // 9 h 25): a press on its line, dragged up
        let star = draw(&mut h, CurveKind::Star, w(95.0, 0.0, 0.0), w(107.0, 0.0, 0.0));
        h.st.selection.clear();
        h.st.arm_push(PushKind::Extrude, None);
        h.steps(2);
        let tip = h.st.curve_lines(h.st.doc.feature(star).unwrap()).concat().into_iter().max_by(|a, b| a.distance(DVec3::new(95.0, 0.0, 0.0)).total_cmp(&b.distance(DVec3::new(95.0, 0.0, 0.0)))).unwrap().as_vec3();
        let (s0, s1) = (h.screen_of(tip), h.screen_of(tip + Vec3::Y * 6.0));
        h.drag(s0, s1, 10, true);
        h.settle();
        eprintln!("  star by the Extrude tool: {:?} tool {:?}", h.st.doc.feature(star).map(|f| f.prim), h.st.tool);
        h.st.edit("Points", |d| {
            let f = d.feature_mut(star).unwrap();
            let (Some(mut c), nurbs_kernel::Prim::Profile { x, z, .. }) = (f.curve, f.prim) else { return };
            let r = c.radius(x, z);
            c.sides = 7;
            f.reshape_curve(c, r);
        });
        h.settle();
        for id in [b, hole, star] {
            let f = h.st.doc.feature(id).unwrap();
            eprintln!("  {} {:?} {:?} axis {:?} at {:?}", f.name, f.op, f.prim, f.axis(), f.origin);
        }
        eprintln!("  errors {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
        h.st.selection = vec![b];
        h.steps(3);
        h.shot("zero-curves-extrude-2-made");
        // the circle of the Subtract grown: the solid follows
        h.st.edit("Radius", |d| {
            let f = d.feature_mut(c).unwrap();
            let (Some(cv), nurbs_kernel::Prim::Path { x, z }) = (f.curve, f.prim) else { return };
            let _ = (x, z);
            f.reshape_curve(cv, 10.0);
        });
        h.settle();
        h.st.selection = vec![hole];
        h.steps(3);
        h.shot("zero-curves-extrude-3-live");
        h.st.selection.clear();
        h.move_to(pos2(1500.0, 120.0));
        h.steps(3);
        h.shot("zero-curves-extrude-4-plain");
    }

    if want("pivot-d") {
        // Romain (20 nov. 7 h 50): D held + click puts the pivot on a
        // point, the move then carries that point onto the points
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, glam::DVec3::ZERO, glam::DQuat::IDENTITY);
        let b = d.add(Op::Add, Prim::Box { x: 10.0, y: 20.0, z: 30.0 }, glam::DVec3::new(69.63, 0.0, 0.0), glam::DQuat::IDENTITY);
        h.reset(d, "Cube");
        h.st.camera.frame(Vec3::new(-25.0, 0.0, -20.0), Vec3::new(80.0, 25.0, 20.0));
        h.steps(2);
        h.settle();
        let arrow = |h: &mut Harness, axis: Vec3, by: f32, name: &str| {
            let c = h.st.pivot().expect("a gizmo").as_vec3();
            let s0 = h.screen_of(c);
            let dir = (h.screen_of(c + axis) - s0).normalized();
            let grab = s0 + dir * 55.0;
            let to = h.screen_of(c + axis * by) - s0;
            h.drag(grab, grab + to, 10, false);
            h.steps(3);
            eprintln!("  snapped {:?}", h.st.snap_shown.as_ref().map(|s| (s.kind, s.pos, s.source)));
            h.shot(name);
            h.press(grab + to, false);
            h.steps(2);
            h.settle();
        };
        // Object mode: the small block, its pivot on its top front -X
        // corner, along -X onto the big one's top front +X corner
        h.click(h.screen_of(Vec3::new(69.63, 20.0, 0.0)));
        h.key_hold(egui::Key::D, true);
        h.move_to(h.screen_of(Vec3::new(64.63, 20.0, 15.0)) + egui::vec2(3.0, 2.0));
        h.steps(2);
        h.shot("zero-pivot-d-1-hover");
        h.click(h.screen_of(Vec3::new(64.63, 20.0, 15.0)) + egui::vec2(3.0, 2.0));
        h.key_hold(egui::Key::D, false);
        h.steps(2);
        eprintln!("  pivot {:?} custom {}", h.st.pivot(), h.st.custom_pivot_on());
        h.shot("zero-pivot-d-2-set");
        arrow(&mut h, Vec3::X, -47.0, "zero-pivot-d-3-move");
        eprintln!("  block at x {:?} (want 25), pivot {:?}", h.st.doc.feature(b).map(|f| f.origin.x), h.st.pivot());
        // Face mode: the small block's top, its pivot on its top front +X
        // corner, down along Y onto the middle of the vertical edge below
        // it (y 10)
        h.key(egui::Key::Escape);
        h.key(egui::Key::F11);
        h.steps(30);
        h.click(h.screen_of(Vec3::new(25.0, 20.0, 0.0)));
        eprintln!("  faces {:?}", h.st.face_sel.iter().map(|r| r.tag).collect::<Vec<_>>());
        h.key_hold(egui::Key::D, true);
        // 15 points off the corner: the pivot jumps onto it
        h.click(h.screen_of(Vec3::new(30.0, 20.0, 15.0)) + egui::vec2(12.0, 9.0));
        h.key_hold(egui::Key::D, false);
        eprintln!("  pivot {:?} custom {}", h.st.pivot(), h.st.custom_pivot_on());
        arrow(&mut h, Vec3::Y, -10.5, "zero-pivot-d-4-face");
        eprintln!(
            "  face moved by {:?} (want y -10), pivot {:?} custom {}, errors {:?}",
            h.st.doc.features.last().and_then(|f| f.tweak.as_ref()).map(|t| t.delta),
            h.st.pivot(),
            h.st.custom_pivot_on(),
            h.st.model.as_ref().map(|m| m.errors.clone())
        );
    }

    if want("knife-diag") {
        // Romain's flank (20 nov.): a block with filleted top edges, a
        // diagonal knife cut from the flank's top corner; the cut picked in
        // Edge mode and deleted; a vertical cut moved along X
        use nurbs_kernel::{FeatureId, Knife, KnifePoint};
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 60.0, y: 40.0, z: 27.4 }, glam::DVec3::ZERO, glam::DQuat::IDENTITY);
        let t = |f: FeatureId, sub: u16| FaceTag { feature: f, sub }.pack();
        let r = d.add_round(6.6, false, vec![EdgeRef { faces: [t(b, 2), t(b, 4)], at: glam::DVec3::new(0.0, 40.0, 13.7) }, EdgeRef { faces: [t(b, 2), t(b, 5)], at: glam::DVec3::new(0.0, 40.0, -13.7) }]);
        let kx = |faces: [i64; 2], at: glam::DVec3| KnifePoint { edge: EdgeRef { faces, at }, f: (30.0 - at.x.abs()) / 60.0, at };
        d.add_knife(Knife { pts: vec![kx([t(b, 4), t(r, 0)], glam::DVec3::new(-30.0, 33.4, 13.7)), kx([t(b, 4), t(b, 3)], glam::DVec3::new(5.0, 0.0, 13.7))], segs: vec![[0, 1]], remove_faces: vec![], remove_edges: vec![] }, "Knife");
        h.reset(d, "Flank");
        h.st.camera.frame(Vec3::new(-32.0, 0.0, -15.0), Vec3::new(32.0, 42.0, 15.0));
        h.steps(2);
        h.settle();
        h.key(egui::Key::F10);
        for at in [Vec3::new(-12.5, 16.7, 13.7), Vec3::new(-20.0, 26.2, 13.7), Vec3::new(0.0, 4.8, 13.7)] {
            let p = h.screen_of(at);
            let e = h.st.pick_edge([p.x, p.y]);
            h.click(p);
            h.steps(2);
            eprintln!("  click at {at:?}: pick {:?} ref {:?}, picked {}", e, e.and_then(|i| h.st.edge_ref(i)).map(|r| r.faces), h.st.edge_sel.len());
        }
        h.shot("zero-knife-diag-1-picked");
        h.key(egui::Key::Delete);
        h.settle();
        eprintln!("  deleted: steps {:?} errors {:?}", h.st.doc.features.iter().map(|f| f.name.clone()).collect::<Vec<_>>(), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-knife-diag-2-deleted");
    }

    if want("comp-delete") {
        // Del on picked components (19 nov. 23 h 22): a knife line taken
        // out (its two faces one again), a face taken out (a hole)
        let say = |h: &Harness, what: &str| {
            let steps: Vec<String> = h.st.doc.features.iter().map(|f| f.name.clone()).collect();
            eprintln!("  {what}: steps {:?} faces {} errors {:?}", steps, h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
        };
        h.reset(test_part(), "Test part");
        h.settle();
        say(&h, "part");
        h.key(egui::Key::S);
        h.hold(Modifiers::CTRL);
        let c2 = h.screen_of(Vec3::new(-15.0, 0.6, 15.0));
        h.move_to(c2);
        h.steps(2);
        h.click(c2);
        h.settle();
        h.hold(Modifiers::NONE);
        say(&h, "face cut");
        h.key(egui::Key::Escape);
        h.key(egui::Key::F10);
        h.click(h.screen_of(Vec3::new(-15.0, 10.0, 15.0)));
        h.steps(2);
        eprintln!("  edges picked {}", h.st.edge_sel.len());
        h.key(egui::Key::Delete);
        h.settle();
        say(&h, "edge deleted");
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(10.0, 10.0, 15.0)));
        h.steps(2);
        eprintln!("  faces picked {}", h.st.face_sel.len());
        h.key(egui::Key::Delete);
        h.settle();
        say(&h, "face deleted");
        h.shot("zero-comp-delete");
        // a cube's edge: its faces are not on one surface
        h.key(egui::Key::F10);
        h.click(h.screen_of(Vec3::new(20.0, 10.0, -15.0)));
        h.steps(2);
        h.key(egui::Key::Delete);
        h.settle();
        say(&h, "corner edge");
    }

    if want("comp-bend") {
        // a face of more than four sides bent by a vertex move (19 nov.): a
        // cube's top front edge chamfered (its +X side a pentagon), the
        // bottom front corner of that side pulled out with the gizmo; then
        // an L sketch's outer top corner raised (its top: six sides)
        let pull = |h: &mut Harness, axis: usize, mm: f32| {
            let c = h.st.pivot().expect("a gizmo").as_vec3();
            let s0 = h.screen_of(c);
            let dir = (h.screen_of(c + Vec3::AXES[axis]) - s0).normalized();
            let grab = s0 + dir * 55.0;
            let by = h.screen_of(c + Vec3::AXES[axis] * mm) - s0;
            h.drag(grab, grab + by, 10, true);
            h.settle();
        };
        let say = |h: &Harness| eprintln!("  faces {} errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Box { x: 20.0, y: 20.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let t = |sub: u16| FaceTag { feature: c, sub }.pack();
        d.add_round(4.0, true, vec![EdgeRef { faces: [t(2), t(4)], at: DVec3::new(0.0, 20.0, 10.0) }]);
        h.reset(d, "Chamfered cube");
        h.st.camera.frame(Vec3::new(-12.0, 0.0, -12.0), Vec3::new(16.0, 22.0, 14.0));
        h.steps(2);
        h.settle();
        h.key(egui::Key::Num1);
        let corner = h.screen_of(Vec3::new(10.0, 0.0, 10.0));
        h.click(corner);
        h.steps(2);
        h.shot("zero-comp-bend-1-picked");
        pull(&mut h, 0, 5.0);
        say(&h);
        h.key(egui::Key::Escape);
        h.st.select(None, false);
        h.steps(2);
        h.shot("zero-comp-bend-2-pentagon");
        // from its side: the bent face
        h.st.camera.orbit(-260.0, 40.0);
        h.st.camera.frame(Vec3::new(-12.0, 0.0, -12.0), Vec3::new(16.0, 22.0, 14.0));
        h.steps(2);
        h.settle();
        h.shot("zero-comp-bend-2b-pentagon-side");
        let mut d = Doc::default();
        let l = d.add(Op::Add, Prim::Profile { x: 30.0, y: 10.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(l).unwrap().profile = Some(nurbs_kernel::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, -0.2], [-0.2, -0.2], [-0.2, 0.5], [-0.5, 0.5]], smooth: vec![false; 6], corner: vec![], tension: vec![], open: false, spline: false });
        h.reset(d, "L sketch");
        h.st.camera.frame(Vec3::new(-16.0, 0.0, -16.0), Vec3::new(16.0, 16.0, 16.0));
        h.steps(2);
        h.settle();
        h.key(egui::Key::Num1);
        let corner = h.screen_of(Vec3::new(15.0, 10.0, -15.0));
        h.click(corner);
        h.steps(2);
        pull(&mut h, 1, 6.0);
        say(&h);
        h.key(egui::Key::Escape);
        h.st.select(None, false);
        h.st.camera.orbit(0.0, -120.0);
        h.st.camera.frame(Vec3::new(-16.0, 0.0, -16.0), Vec3::new(16.0, 16.0, 16.0));
        h.steps(2);
        h.settle();
        h.shot("zero-comp-bend-3-sketch-top");
    }
    if want("comp-tilt") {
        // a face with a hole, a boss or a pocket tilted by an edge move (19
        // nov.): their walls run on to the new plane. The test part's top
        // left (the hole) raised on its -X edge; a block with a boss and a
        // square pocket raised on its +X edge, then lowered
        let pull = |h: &mut Harness, axis: usize, mm: f32| {
            let c = h.st.pivot().expect("a gizmo").as_vec3();
            let s0 = h.screen_of(c);
            let dir = (h.screen_of(c + Vec3::AXES[axis]) - s0).normalized();
            let grab = s0 + dir * 55.0;
            let by = h.screen_of(c + Vec3::AXES[axis] * mm) - s0;
            h.drag(grab, grab + by, 10, true);
            h.settle();
        };
        let say = |h: &Harness| eprintln!("  faces {} errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.reset(test_part(), "Test part");
        h.st.camera.frame(Vec3::new(-22.0, 0.0, -16.0), Vec3::new(22.0, 30.0, 16.0));
        h.steps(2);
        h.settle();
        h.key(egui::Key::Num2);
        let edge = h.screen_of(Vec3::new(-20.0, 20.0, 6.0));
        h.click(edge);
        h.steps(2);
        h.shot("zero-comp-tilt-1-picked");
        pull(&mut h, 1, 6.0);
        say(&h);
        h.key(egui::Key::Escape);
        h.st.select(None, false);
        h.steps(2);
        h.shot("zero-comp-tilt-2-hole");
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Add, Prim::Cylinder { radius: 6.0, height: 8.0 }, DVec3::new(-9.0, 20.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Box { x: 10.0, y: 6.0, z: 12.0 }, DVec3::new(9.0, 14.0, 0.0), DQuat::IDENTITY);
        let _ = b;
        h.reset(d, "Boss and pocket");
        h.st.camera.frame(Vec3::new(-22.0, 0.0, -16.0), Vec3::new(22.0, 30.0, 16.0));
        h.steps(2);
        h.settle();
        h.key(egui::Key::Num2);
        let edge = h.screen_of(Vec3::new(20.0, 20.0, 6.0));
        h.click(edge);
        h.steps(2);
        pull(&mut h, 1, 6.0);
        say(&h);
        h.key(egui::Key::Escape);
        h.st.select(None, false);
        h.steps(2);
        h.shot("zero-comp-tilt-3-boss-pocket");
        h.st.undo_doc();
        h.steps(2);
        h.settle();
        h.key(egui::Key::Num2);
        let edge = h.screen_of(Vec3::new(20.0, 20.0, 6.0));
        h.click(edge);
        h.steps(2);
        pull(&mut h, 1, -3.0);
        say(&h);
        h.key(egui::Key::Escape);
        h.st.select(None, false);
        h.steps(2);
        h.shot("zero-comp-tilt-4-lowered");
    }
    if want("comp-fillet") {
        // a face moved where fillets made before run along it (19 nov.):
        // moved before them, they made again. A block with its top edges
        // filleted (tension 0.7): the top pulled up; then a block with a
        // hole whose mouth is chamfered, its top tilted by an edge
        let pull = |h: &mut Harness, axis: usize, mm: f32| {
            let c = h.st.pivot().expect("a gizmo").as_vec3();
            let s0 = h.screen_of(c);
            let dir = (h.screen_of(c + Vec3::AXES[axis]) - s0).normalized();
            let grab = s0 + dir * 55.0;
            let by = h.screen_of(c + Vec3::AXES[axis] * mm) - s0;
            h.drag(grab, grab + by, 10, true);
            h.settle();
        };
        let say = |h: &Harness| eprintln!("  faces {} errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let t = |sub: u16| FaceTag { feature: b, sub }.pack();
        let r = d.add_round(5.0, false, vec![
            EdgeRef { faces: [t(2), t(0)], at: DVec3::new(20.0, 20.0, 0.0) },
            EdgeRef { faces: [t(2), t(1)], at: DVec3::new(-20.0, 20.0, 0.0) },
            EdgeRef { faces: [t(2), t(4)], at: DVec3::new(0.0, 20.0, 15.0) },
            EdgeRef { faces: [t(2), t(5)], at: DVec3::new(0.0, 20.0, -15.0) },
        ]);
        d.feature_mut(r).unwrap().round.as_mut().unwrap().tension = 0.7;
        h.reset(d, "Filleted block");
        h.st.camera.frame(Vec3::new(-22.0, 0.0, -16.0), Vec3::new(22.0, 30.0, 16.0));
        h.steps(2);
        h.settle();
        h.key(egui::Key::Num3);
        let top = h.screen_of(Vec3::new(2.0, 20.0, 2.0));
        h.click(top);
        h.steps(2);
        h.shot("zero-comp-fillet-1-picked");
        pull(&mut h, 1, 8.0);
        say(&h);
        h.key(egui::Key::Escape);
        h.st.select(None, false);
        h.steps(2);
        h.shot("zero-comp-fillet-2-top-up");
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let hole = d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-5.0, -5.0, 0.0), DQuat::IDENTITY);
        let t = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
        d.add_round(1.5, true, vec![EdgeRef { faces: [t(b, 2), t(hole, 0)], at: DVec3::new(0.0, 20.0, 0.0) }]);
        h.reset(d, "Chamfered hole");
        h.st.camera.frame(Vec3::new(-22.0, 0.0, -16.0), Vec3::new(22.0, 30.0, 16.0));
        h.steps(2);
        h.settle();
        h.key(egui::Key::Num2);
        let edge = h.screen_of(Vec3::new(6.0, 20.0, 15.0));
        h.click(edge);
        h.steps(2);
        pull(&mut h, 1, 6.0);
        say(&h);
        h.key(egui::Key::Escape);
        h.st.select(None, false);
        h.steps(2);
        h.shot("zero-comp-fillet-3-chamfered-hole");
    }
    if want("comp-curved") {
        // a flat face moved along its normal with curved faces round it:
        // the boss's top up (its side follows), the block's top up and
        // down (the hole's wall, the boss's side, the slot's wall follow)
        h.reset(test_part(), "Test part");
        let pull = |h: &mut Harness, axis: usize, mm: f32| {
            let c = h.st.pivot().expect("a gizmo").as_vec3();
            let s0 = h.screen_of(c);
            let dir = (h.screen_of(c + Vec3::AXES[axis]) - s0).normalized();
            let grab = s0 + dir * 55.0;
            let by = h.screen_of(c + Vec3::AXES[axis] * mm) - s0;
            h.drag(grab, grab + by, 10, true);
            h.settle();
        };
        let say = |h: &Harness| eprintln!("  faces {} errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.key(egui::Key::Num3);
        let top = h.screen_of(Vec3::new(10.0, 30.0, 2.0));
        h.click(top);
        h.steps(2);
        h.shot("zero-comp-curved-1-picked");
        pull(&mut h, 1, 6.0);
        say(&h);
        h.shot("zero-comp-curved-2-boss-up");
        h.key(egui::Key::Escape);
        let left = h.screen_of(Vec3::new(-15.0, 20.0, 8.0));
        h.click(left);
        h.steps(2);
        pull(&mut h, 1, 4.0);
        say(&h);
        h.shot("zero-comp-curved-3-top-up");
        h.key(egui::Key::Escape);
        let right = h.screen_of(Vec3::new(17.0, 20.0, 10.0));
        h.click(right);
        h.steps(2);
        pull(&mut h, 1, -3.0);
        say(&h);
        h.key(egui::Key::Escape);
        h.shot("zero-comp-curved-4-top-down");
        let made = h.st.doc.features.iter().filter_map(|f| f.tweak.as_ref().map(|t| (f.name.clone(), t.delta))).collect::<Vec<_>>();
        eprintln!("  moves {made:?}");
    }

    if want("knife-curved") {
        // the Knife across the boss's round side: top circle to foot
        // circle, twice (each its own cut); the line follows the side
        h.reset(test_part(), "Test part");
        h.key(egui::Key::K);
        let on = |y: f32, a: f32| Vec3::new(10.0 + 7.0 * a.to_radians().cos(), y, 7.0 * a.to_radians().sin());
        for (a, b) in [(150.0, 95.0), (75.0, 20.0)] {
            let p = h.screen_of(on(30.0, a));
            h.move_to(p);
            h.steps(2);
            h.click(p);
            h.settle();
            let q = h.screen_of(on(20.0, b));
            h.move_to(q);
            h.steps(2);
            if a == 150.0 {
                h.shot("zero-knife-curved-1-line");
            }
            h.click(q);
            h.settle();
            h.key(egui::Key::Enter);
            h.settle();
        }
        eprintln!("  faces {} errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0), h.st.model.as_ref().map(|m| m.errors.clone()));
        for f in h.st.doc.features.iter().filter(|f| f.cut.is_some()) {
            let k = f.cut.as_ref().unwrap();
            eprintln!("  {} {} points {:?} lines", f.name, k.pts.len(), k.segs);
        }
        h.key(egui::Key::Escape);
        h.shot("zero-knife-curved-2-done");
        // Face mode: the piece between the two lines picked
        h.key(egui::Key::Num3);
        let m = h.screen_of(on(25.0, 85.0));
        h.move_to(m);
        h.steps(2);
        h.click(m);
        h.settle();
        h.shot("zero-knife-curved-3-piece");
    }

    if want("snap") {
        // snapping to the part's points of interest, Plasticity style
        h.reset(test_part(), "Test part");
        let say = |h: &Harness, what: &str| {
            let s = h.st.snap_shown.as_ref().map(|s| format!("{:?} at {:.2?}", s.kind, s.pos.to_array()));
            eprintln!("  {what}: {}", s.unwrap_or_else(|| "nothing".into()));
        };
        eprintln!("  {} points of interest", h.st.snaps.len());
        // the Box tool armed, the pointer near the block's top corner
        h.st.tool = Tool::Create(PrimKind::Box);
        let corner = h.screen_of(Vec3::new(20.0, 20.0, 15.0)) + egui::vec2(4.0, -3.0);
        h.move_to(corner);
        h.steps(2);
        h.shot("snap/snap-1-corner");
        // near the boss's top centre
        let top = h.screen_of(Vec3::new(10.0, 30.0, 0.0)) + egui::vec2(-5.0, 4.0);
        h.move_to(top);
        h.steps(2);
        h.shot("snap/snap-2-center");
        // on the ground in front of the block, lined up with its corner
        let ahead = h.screen_of(Vec3::new(20.3, 0.0, 24.0));
        h.move_to(ahead);
        h.steps(2);
        h.shot("snap/snap-3-aligned");
        say(&h, "aligned");
        // a cylinder from the boss's centre, its radius to the boss's edge
        h.st.tool = Tool::Create(PrimKind::Cylinder);
        let rim = h.screen_of(Vec3::new(17.0, 30.0, 0.0));
        h.drag(top, rim, 10, false);
        h.steps(2);
        say(&h, "radius");
        h.shot("snap/snap-4-radius");
        h.press(rim, false);
        h.step();
        let up = h.screen_of(Vec3::new(10.0, 38.0, 0.0));
        h.move_to(up);
        h.settle();
        h.click(up);
        h.settle();
        let f = h.st.doc.features.last().unwrap();
        eprintln!("  new cylinder {:?} at {:.3?}", f.prim, f.origin.to_array());
        // a box on the ground beside the block, as high as the block
        h.st.tool = Tool::Create(PrimKind::Box);
        let (a, b) = (h.screen_of(Vec3::new(26.0, 0.0, -10.0)), h.screen_of(Vec3::new(36.0, 0.0, 6.0)));
        h.drag(a, b, 10, true);
        h.steps(2);
        let to = h.screen_of(Vec3::new(20.0, 20.0, -15.0)) + egui::vec2(3.0, 3.0);
        h.move_to(to);
        h.settle();
        say(&h, "height");
        h.shot("snap/snap-5-height");
        h.click(to);
        h.settle();
        let f = h.st.doc.features.last().unwrap();
        eprintln!("  new box {:?} at {:.3?}", f.prim, f.origin.to_array());
        // the boss moved along X onto the hole's axis
        h.st.tool = Tool::Select;
        let boss = h.screen_of(Vec3::new(10.0, 25.0, 6.9));
        h.click(boss);
        h.steps(2);
        let pivot = h.st.pivot().unwrap();
        let v = h.st.view();
        let k = v.pixel_size_at((pivot.as_vec3() - v.eye).length()) * 78.0;
        let tip = h.screen_of(pivot.as_vec3() + Vec3::X * k * 0.7);
        let hole = h.screen_of(Vec3::new(-10.0, 20.0, 0.0)) + egui::vec2(3.0, 2.0);
        h.drag(tip, hole, 12, false);
        h.settle();
        say(&h, "move");
        h.shot("snap/snap-6-move");
        h.press(hole, false);
        h.settle();
        let f = h.st.doc.features.iter().find(|f| f.name == "Boss").unwrap();
        eprintln!("  boss now at {:.3?}", f.origin.to_array());
        h.st.selection.clear();
        h.steps(2);
        h.shot("snap/snap-7-done");
    }
}

/// The three tabs (Romain, 20 nov.: Forme, Sculpt, Poly) in one scene: the
/// test part in Forme, a clay sphere sculpted on top of it, a Poly cube
/// beside it extruded, inset and cut by an edge loop.
/// The SDF tab (Romain 20 nov., 23 h 47: the SDF engine back, green): a
/// cube, a sphere blended on it and moved by the gizmo (live), a cylinder
/// carved through, a capsule and a torus, sizes and fillet from the window.
fn sdf_tab(h: &mut Harness) {
    use egui::Key;
    use nurbs_app::state::{GizmoMode, Handle};
    h.reset(Doc::default(), "SDF");
    h.key(Key::F4);
    h.steps(3);
    h.shot("sdf-01-tab");
    // B: a new SDF object, a 20 mm cube standing on the ground
    h.key(Key::B);
    h.st.sdf_settle();
    h.steps(2);
    h.st.sdf_edit_node("Fillet", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::new(16.0, 8.0, 12.0);
            s.round = 1.5;
        }
    });
    h.st.sdf_settle();
    h.st.frame_current();
    h.settle();
    h.st.camera.dist *= 2.2;
    h.steps(3);
    h.shot("sdf-02-cube");
    // O: a sphere on top of it, blended 4 mm
    h.st.sdf.blend = 4.0;
    h.key(Key::O);
    h.st.sdf_settle();
    h.steps(2);
    h.shot("sdf-03-sphere-blend");
    // the gizmo moves the sphere along X: live (coarse), then fine
    let pv = h.st.sdf_pivot().unwrap().as_vec3();
    let a = h.screen_of(pv);
    let b = h.screen_of(pv + Vec3::new(9.0, 0.0, 0.0));
    h.st.set_gizmo(GizmoMode::Move);
    h.st.begin_drag(Handle::Axis(0), [a.x, a.y]);
    for i in 1..=8 {
        let q = a + (b - a) * (i as f32 / 8.0);
        h.st.update_sdf_drag([q.x, q.y]);
        h.steps(1);
    }
    h.shot("sdf-04-move-live");
    eprintln!("sphere at {:?} (from {pv:?})", h.st.sdf_pivot());
    h.st.end_sdf_drag();
    h.st.sdf_settle();
    h.steps(2);
    h.shot("sdf-05-moved");
    // X then C: a cylinder carved from the sphere's top, down through the cube
    h.st.sdf.mode = forme_core::CombineMode::Carve;
    h.st.sdf.blend = 1.0;
    h.key(Key::C);
    h.st.sdf_edit_node("Size", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::new(4.0, 30.0, 0.0);
        }
    });
    h.st.sdf_settle();
    h.steps(2);
    h.shot("sdf-06-carve");
    // the cube picked by a click on its side, a capsule and a torus added
    let cube = h.screen_of(Vec3::new(-12.0, 8.0, 12.0));
    h.click(cube);
    h.steps(2);
    eprintln!("picked: {:?}", h.st.sdf_sel);
    h.key(Key::A);
    h.st.sdf.blend = 2.0;
    h.key(Key::T);
    h.st.sdf_edit_node("Move", |n| n.transform.pos = Vec3::new(-8.0, 16.0, 0.0));
    h.st.sdf_settle();
    h.steps(2);
    h.shot("sdf-07-torus");
    h.st.set_gizmo(GizmoMode::Rotate);
    h.key(Key::P);
    h.st.sdf_edit_node("Move", |n| {
        n.transform.pos = Vec3::new(0.0, 8.0, 12.0);
        n.transform.rot = glam::Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    });
    h.st.sdf_settle();
    h.steps(2);
    h.shot("sdf-08-capsule");
    // the window: the picked shape's settings and the list
    h.st.set_gizmo(GizmoMode::Scale);
    h.steps(2);
    h.shot("sdf-09-cage");
    // the whole object in the Forme tab, beside nothing: a mesh like any
    h.key(Key::F1);
    h.steps(2);
    h.shot("sdf-10-forme-tab");
    // undo all the way back, redo
    let n = h.st.undo_labels().len();
    eprintln!("undo list: {:?}", h.st.undo_labels());
    for _ in 0..3 {
        h.st.undo();
    }
    h.st.sdf_settle();
    h.steps(2);
    h.shot("sdf-11-undo3");
    let _ = n;
    // the light theme
    h.key(Key::F4);
    h.st.settings.light = true;
    h.steps(3);
    h.shot("sdf-12-light");
}

/// The SDF tab's shape settings from the engine: Groove / Tongue, Mirror,
/// Repeat in a line and round; the shapes in the outliner.
fn sdf_more(h: &mut Harness) {
    use egui::Key;
    use forme_core::{Axis, CombineMode, Mirror, Repeat};
    h.reset(Doc::default(), "SDF more");
    h.key(Key::F4);
    h.steps(2);
    // a slab
    h.key(Key::B);
    h.st.sdf_edit_node("Size", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::new(30.0, 4.0, 20.0);
            s.round = 1.0;
        }
        n.transform.pos = Vec3::new(0.0, 4.0, 0.0);
    });
    h.st.sdf_settle();
    h.st.frame_current();
    h.settle();
    h.st.camera.dist *= 1.6;
    h.steps(2);
    // a groove: a cylinder lying across it, its surface cuts a line
    h.st.sdf.blend = 0.0;
    h.key(Key::C);
    h.st.sdf_edit_node("Groove", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::new(12.0, 40.0, 0.0);
        }
        n.transform.pos = Vec3::new(0.0, 0.0, 0.0);
        n.transform.rot = glam::Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        n.combine.mode = CombineMode::Groove;
        n.combine.depth = 1.0;
        n.combine.width = 0.8;
        n.combine.blend = 0.0;
    });
    h.st.sdf_settle();
    h.steps(2);
    h.shot("sdfx-01-groove");
    // a boss off the middle, mirrored across X
    h.st.sdf.mode = CombineMode::Add;
    h.st.sdf.blend = 2.0;
    h.key(Key::O);
    h.st.sdf_edit_node("Mirror", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::splat(4.0);
        }
        n.transform.pos = Vec3::new(18.0, 8.0, 8.0);
        n.mirror = Mirror { x: true, y: false, z: false };
    });
    h.st.sdf_settle();
    h.steps(2);
    h.shot("sdfx-02-mirror");
    // a row of holes: one carved cylinder repeated 5 times along X
    h.st.sdf.mode = CombineMode::Carve;
    h.st.sdf.blend = 0.5;
    h.key(Key::C);
    h.st.sdf_edit_node("Repeat", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::new(1.5, 10.0, 0.0);
        }
        n.transform.pos = Vec3::new(-20.0, 4.0, -12.0);
        n.transform.rot = glam::Quat::IDENTITY;
        n.repeat = Repeat::Linear { count: 5, step: Vec3::new(10.0, 0.0, 0.0) };
    });
    h.st.sdf_settle();
    h.steps(2);
    h.shot("sdfx-03-repeat-line");
    // a tongue ring round a capsule repeated 8 times round Y
    h.st.sdf.mode = CombineMode::Add;
    h.st.sdf.blend = 1.0;
    h.key(Key::P);
    h.st.sdf_edit_node("Repeat", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::new(1.2, 3.0, 0.0);
        }
        n.transform.pos = Vec3::new(0.0, 9.0, 9.0);
        n.repeat = Repeat::Radial { count: 8, axis: Axis::Y };
    });
    h.st.sdf_settle();
    h.steps(2);
    h.shot("sdfx-04-repeat-round");
    eprintln!("sdfx: {} triangles", h.st.doc.features.iter().filter_map(|f| f.mesh.as_ref()).map(|m| m.faces.len()).sum::<usize>());
    h.key(Key::F);
    h.settle();
    h.shot("sdfx-05-frame-shape");
}

/// One base for every object (Romain, 21 nov., 0 h 39): the NURBS part,
/// a clay, a Poly cube and an SDF object given materials and a look the
/// same way, listed in one outliner, aligned one onto another.
fn common_meshes(h: &mut Harness) {
    use egui::Key;
    h.reset(test_part(), "Common meshes");
    // a clay, a Poly cube, an SDF object beside the part
    h.key(Key::F2);
    h.st.camera.target = Vec3::new(-60.0, 0.0, 0.0);
    h.key(Key::O);
    h.settle();
    let clay = h.st.selection.first().copied();
    h.key(Key::F3);
    h.st.camera.target = Vec3::new(60.0, 0.0, 0.0);
    h.key(Key::B);
    h.settle();
    let poly = h.st.selection.first().copied();
    h.key(Key::F4);
    h.st.camera.target = Vec3::new(0.0, 0.0, 60.0);
    h.st.sdf_sel = None;
    h.st.selection.clear();
    h.key(Key::B);
    h.st.sdf.blend = 3.0;
    h.key(Key::O);
    h.st.sdf_settle();
    let sdf = h.st.selection.first().copied();
    eprintln!("common: clay {clay:?} poly {poly:?} sdf {sdf:?}");
    h.key(Key::F1);
    h.st.sdf_sel = None;
    // materials: one ball each, the same call as for the part
    for (id, m) in [(clay, 1u32), (poly, 3), (sdf, 4)] {
        if let Some(id) = id {
            h.st.selection = vec![id];
            h.st.apply_material(m);
        }
    }
    h.st.selection.clear();
    h.st.camera.target = Vec3::ZERO;
    h.st.frame_all();
    h.settle();
    h.shot("commonm-01-materials");
    // the clay's ball made see-through and glossy: the clay takes it
    let m = clay.and_then(|id| h.st.doc.feature(id)).map(|f| f.material).unwrap_or(1);
    h.st.edit_look(m, "Transparency", |l| {
        l.transparency = 0.55;
        l.specular = 0.9;
    });
    h.st.end_look();
    h.steps(3);
    h.shot("commonm-02-look-on-clay");
    // Align: the Poly cube onto the SDF object's Max X (3ds Max's Align)
    if let (Some(p), Some(s)) = (poly, sdf) {
        h.st.selection = vec![p];
        h.st.begin_align();
        let c = h.st.doc.feature(s).map(|f| f.origin.as_vec3()).unwrap_or(Vec3::ZERO);
        let at = h.screen_of(c + Vec3::Y * 4.0);
        h.st.align_pick([at.x, at.y]);
        if let Some(a) = h.st.align.as_mut() {
            a.axes = [true, false, true];
            a.cur = nurbs_app::align::AlignPoint::Min;
            a.tgt = nurbs_app::align::AlignPoint::Max;
        }
        h.st.align_update();
        h.steps(3);
        h.shot("commonm-03-align-window");
        h.st.align_ok();
        eprintln!("common: poly now at {:?}, sdf at {:?}", h.st.doc.feature(p).map(|f| f.origin), h.st.doc.feature(s).map(|f| f.origin));
        h.steps(2);
        h.shot("commonm-04-aligned");
    }
    // the outliner with the SDF object's shapes under it
    if let Some(s) = sdf {
        h.st.selection = vec![s];
    }
    h.steps(3);
    h.shot("commonm-05-outliner");
}

/// The base shared by the tabs (Romain, 21 nov.: "tout feature commune…
/// doit marcher sur une base commune"): an SDF object next to the NURBS
/// test part, in the outliner's SDF group, its shapes in the Sub Outliner
/// and the history strip (marker, on / off), a shape's own material, the
/// materials' shader settings, the marking menu in the SDF tab.
fn common_base(h: &mut Harness) {
    use egui::Key;
    use nurbs_app::common::Item;
    h.reset(test_part(), "Common");
    h.key(Key::F4);
    h.steps(2);
    h.key(Key::B);
    h.st.sdf.blend = 3.0;
    h.key(Key::O);
    h.st.sdf.mode = forme_core::CombineMode::Carve;
    h.st.sdf.blend = 1.0;
    h.key(Key::C);
    h.st.sdf_edit_node("Size", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::new(4.0, 30.0, 0.0);
        }
    });
    let id = h.st.sdf_target().expect("an SDF object");
    h.st.edit("Move", |d| d.feature_mut(id).unwrap().origin.x += 55.0);
    h.st.sdf_settle();
    h.st.frame_all();
    h.settle();
    let items = h.st.element_items(id);
    eprintln!("SDF object {id}: {} shapes {:?}, marker {}", items.len(), items, h.st.item_marker(id));
    assert_eq!(items.len(), 3);
    h.shot("common-01-sdf-items");
    // the sphere picked, Orange on it alone
    if let Item::Shape(_, n) = items[1] {
        h.st.select_item(Item::Shape(id, n));
    }
    h.st.apply_material(2);
    h.st.sdf_settle();
    h.steps(2);
    eprintln!("sphere's material {}, mats {:?}", h.st.item_material(items[1]), h.st.sdf_mats.get(&id).map(|m| m.iter().filter(|x| **x == 3).count()));
    h.shot("common-02-shape-material");
    // the shader settings of Green, edited: the NURBS block wears it too
    h.st.edit("Material", |d| d.features[0].material = 1);
    h.st.edit_look(1, "Material color", |l| {
        l.color = [0.22, 0.48, 0.78];
        l.specular = 0.8;
    });
    h.st.end_look();
    h.steps(3);
    h.shot("common-03-shader-settings");
    // the history marker back to the cube alone, then all back
    h.st.select_item(Item::Feature(id));
    h.st.set_item_marker(id, 1);
    h.st.sdf_settle();
    h.steps(2);
    h.shot("common-04-marker");
    h.st.set_item_marker(id, 3);
    // the carve turned off from the Sub Outliner's switch
    h.st.toggle_item(items[2]);
    h.st.sdf_settle();
    h.steps(2);
    h.shot("common-05-carve-off");
    h.st.toggle_item(items[2]);
    h.st.sdf_settle();
    // the marking menu on the sphere (SDF tab), held open
    let pv = h.st.item_points(items[1]);
    let c = pv.iter().fold(glam::DVec3::ZERO, |a, b| a + *b) / pv.len().max(1) as f64;
    let at = h.screen_of(c.as_vec3());
    h.rpress(at, true);
    h.steps(16);
    eprintln!("marking menu on {:?}", h.st.marking.menu.as_ref().map(|m| m.ctx));
    h.shot("common-06-marking");
    h.rpress(at, false);
    h.steps(2);
    // the NURBS block: its own items, the same lists
    h.key(Key::F1);
    h.st.select_item(Item::Feature(h.st.doc.features[0].id));
    h.steps(3);
    h.shot("common-07-nurbs");
}

fn multi(h: &mut Harness) {
    use egui::Key;
    h.reset(test_part(), "Robot");
    h.shot("multi-1-forme");
    // --- Sculpt: F2, a clay sphere on the block, strokes
    h.key(Key::F2);
    h.steps(2);
    h.st.camera.target = Vec3::new(-10.0, 33.0, 0.0);
    h.key(Key::O);
    h.settle();
    h.st.frame_current();
    h.settle();
    h.shot("multi-2-sculpt-clay");
    let c = Vec3::new(-10.0, 33.0, 0.0);
    let centre = h.screen_of(c);
    let v = h.st.view();
    let k = 1.0 / v.pixel_size_at((c - v.eye).length());
    eprintln!("clay radius on screen: {:.0} points", 10.0 * k);
    h.st.sculpt.radius = (10.0 * k * 0.28).max(12.0);
    h.st.sculpt.strength = 0.8;
    // Draw: a ridge across the front
    h.move_to(centre + vec2(-0.45, 0.1) * 10.0 * k);
    h.steps(2);
    h.shot("multi-3-brush-ring");
    let along = |h: &mut Harness, a: (f32, f32), b: (f32, f32), n: usize| {
        let pa = centre + vec2(a.0, a.1) * 10.0 * k;
        let pb = centre + vec2(b.0, b.1) * 10.0 * k;
        h.drag(pa, pb, n, true);
        h.steps(2);
    };
    for _ in 0..2 {
        along(h, (-0.6, 0.15), (0.6, 0.15), 24);
    }
    // Ctrl: dig eyes
    h.hold(Modifiers::CTRL);
    along(h, (-0.32, -0.25), (-0.22, -0.25), 6);
    h.hold(Modifiers::NONE);
    h.shot("multi-4-sculpt-draw");
    // Grab: pull a horn up from the top
    h.key(Key::G);
    along(h, (0.15, -0.75), (0.35, -1.25), 12);
    // Inflate the cheeks, Pinch the ridge, Smooth (Shift)
    h.key(Key::I);
    along(h, (-0.5, 0.45), (-0.3, 0.5), 10);
    h.key(Key::P);
    along(h, (-0.5, 0.15), (0.5, 0.15), 18);
    h.hold(Modifiers::SHIFT);
    along(h, (-0.2, 0.6), (0.3, 0.6), 10);
    h.hold(Modifiers::NONE);
    h.key(Key::X);
    h.shot("multi-5-sculpt-more");
    eprintln!("undo after the strokes: {:?}", h.st.undo_labels().iter().rev().take(8).collect::<Vec<_>>());
    // --- Poly: F3, a cube beside the part
    h.key(Key::F3);
    h.steps(2);
    h.st.camera.target = Vec3::new(40.0, 10.0, 0.0);
    h.key(Key::B);
    h.steps(2);
    h.st.frame_current();
    h.settle();
    h.shot("multi-6-poly-cube");
    // Face mode, the top face picked, Ctrl + E pulled up, a click
    h.key(Key::Num3);
    let top = h.screen_of(Vec3::new(40.0, 20.0, 0.0));
    h.move_to(top);
    h.steps(2);
    h.click(top);
    h.steps(2);
    h.shot("multi-7-poly-face");
    h.hold(Modifiers::CTRL);
    h.key(Key::E);
    h.hold(Modifiers::NONE);
    let up = top + vec2(0.0, -70.0);
    for i in 1..=10 {
        h.move_to(top + (up - top) * (i as f32 / 10.0));
    }
    h.steps(2);
    h.shot("multi-8-poly-extrude");
    h.click(up);
    h.steps(2);
    // Inset (I): the pointer from outside towards the face's centre (the
    // cube's top where the extrusion left it)
    let top_y = |h: &Harness| h.st.doc.features.iter().find(|f| f.is_mesh() && f.name == "Cube").map(|f| f.bounds().1.y as f32).unwrap_or(20.0);
    let cap = h.screen_of(Vec3::new(40.0, top_y(h), 0.0));
    h.move_to(cap + vec2(90.0, 0.0));
    h.key(Key::I);
    for i in 1..=8 {
        h.move_to(cap + vec2(90.0 - 45.0 * i as f32 / 8.0, 0.0));
    }
    h.click(cap + vec2(45.0, 0.0));
    h.steps(2);
    // and extruded again, up
    let cap = h.screen_of(Vec3::new(40.0, top_y(h), 0.0));
    h.hold(Modifiers::CTRL);
    h.key(Key::E);
    h.hold(Modifiers::NONE);
    for i in 1..=8 {
        h.move_to(cap + vec2(0.0, -45.0 * i as f32 / 8.0));
    }
    h.click(cap + vec2(0.0, -45.0));
    h.steps(2);
    h.shot("multi-9-poly-inset-extrude");
    // Edge loop (Ctrl + R) across the cube's sides
    h.hold(Modifiers::CTRL);
    h.key(Key::R);
    h.hold(Modifiers::NONE);
    let side = h.screen_of(Vec3::new(50.0, 10.0, 0.0));
    let e = h.screen_of(Vec3::new(50.0, 10.0, 10.0));
    let at = side + (e - side) * 0.92;
    h.move_to(at);
    h.steps(2);
    h.shot("multi-10-poly-loop-preview");
    h.click(at);
    h.steps(2);
    // a point moved up (Vertex mode, the move gizmo)
    h.key(Key::W);
    h.key(Key::Num2);
    h.shot("multi-11-poly-loop");
    // smooth preview (Alt + S), Object mode
    h.key(Key::Num4);
    h.hold(Modifiers::ALT);
    h.key(Key::S);
    h.hold(Modifiers::NONE);
    h.steps(2);
    h.shot("multi-12-poly-smooth");
    // --- the whole scene, in each tab
    h.st.selection.clear();
    h.st.frame_all();
    h.settle();
    h.key(Key::F1);
    h.steps(3);
    h.shot("multi-13-scene-forme");
    h.key(Key::F2);
    h.st.select_tool();
    h.steps(3);
    h.shot("multi-14-scene-sculpt");
    h.key(Key::F3);
    h.steps(3);
    h.shot("multi-15-scene-poly");
    // the Poly cube selected in Object mode, moved by the gizmo with the
    // part's snapping, as any shape
    let body = h.screen_of(Vec3::new(40.0, 10.0, 10.0));
    h.click(body);
    h.steps(2);
    h.shot("multi-16-poly-selected");
    let names: Vec<String> = h.st.doc.features.iter().map(|f| format!("{} ({})", f.name, f.kind_label())).collect();
    eprintln!("scene: {names:?}");
    eprintln!("model faces {:?}, kernel errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()), h.st.model.as_ref().map(|m| m.errors.clone()));
    // light theme, Sculpt
    h.st.settings.light = true;
    h.key(Key::F2);
    h.steps(3);
    h.shot("multi-17-light-sculpt");
    h.st.settings.light = false;
}

/// The Poly tab's Bevel (Romain 20 nov., 15 h 40: "le bevel comme Maya,
/// avec chamfer et bevel, et surtout en travaillant bien les coins"): a
/// cube's every edge, the pointer widening it, 3 rounded segments, its
/// corner close up; the 4 top edges (mitred corners); a chamfer of 1;
/// a cylinder's top face.
fn poly_bevel(h: &mut Harness) {
    use egui::Key;
    h.reset(Doc::default(), "Bevel");
    h.key(Key::F3);
    h.steps(2);
    h.st.camera.target = Vec3::new(0.0, 10.0, 0.0);
    h.key(Key::B);
    h.steps(2);
    h.st.camera.yaw = 0.6;
    h.st.camera.pitch = 0.45;
    h.st.frame_current();
    h.settle();
    h.shot("pbevel-0-cube");
    // the whole cube (Object mode): Ctrl + B, the pointer to the right
    let c = h.screen_of(Vec3::new(0.0, 10.0, 0.0));
    let start = c + vec2(160.0, 60.0);
    h.move_to(start);
    h.hold(Modifiers::CTRL);
    h.key(Key::B);
    h.hold(Modifiers::NONE);
    h.steps(2);
    h.shot("pbevel-1-chamfer-live");
    for i in 1..=10 {
        h.move_to(start + vec2(4.0 * i as f32, 0.0));
    }
    h.key(Key::CloseBracket);
    h.key(Key::CloseBracket);
    h.key(Key::C);
    h.steps(2);
    h.shot("pbevel-2-round-live");
    h.click(start + vec2(40.0, 0.0));
    h.steps(3);
    let b = h.st.poly_bevel.clone();
    eprintln!("bevel: width {:?} made {:?} segments {:?} faces {:?}", b.as_ref().map(|b| b.width), b.as_ref().map(|b| b.made), b.as_ref().map(|b| b.segments), b.as_ref().map(|b| b.faces.len()));
    h.shot("pbevel-3-kept");
    // its corner, close, seen along the cube's diagonal (no gizmo, nothing
    // tinted: the faces as they are)
    h.key(Key::Q);
    h.st.poly_sel = None;
    h.move_to(pos2(1200.0, 700.0)); // over the help box: no face hovered
    h.st.camera.target = Vec3::new(7.0, 17.0, 7.0);
    h.st.camera.yaw = 0.785;
    h.st.camera.pitch = 0.62;
    h.st.camera.dist *= 0.55;
    h.steps(3);
    h.shot("pbevel-4-corner");
    // 4 segments, then 5 (odd: a small polygon in the middle)
    if let Some(b) = h.st.poly_bevel.clone() {
        for (segs, chamfer, name) in [(4, false, "pbevel-5-corner-4"), (5, false, "pbevel-6-corner-5"), (1, true, "pbevel-7-corner-chamfer"), (3, true, "pbevel-7b-corner-chamfer-3")] {
            h.st.set_poly_bevel(b.width, segs, chamfer, true);
            h.st.end_live();
            h.st.poly_sel = None;
            h.steps(2);
            h.shot(name);
        }
    }
    h.key(Key::W);
    eprintln!("undo: {:?}", h.st.undo_labels().iter().rev().take(6).collect::<Vec<_>>());
    // back to the cube, its 4 top edges (Edge mode)
    while h.st.undo_labels().last().map(|l| l.starts_with("Bevel")).unwrap_or(false) {
        h.st.undo();
        h.steps(1);
    }
    h.st.camera.target = Vec3::new(0.0, 10.0, 0.0);
    h.st.frame_current();
    h.st.camera.yaw = 0.6;
    h.st.camera.pitch = 0.45;
    h.settle();
    h.key(Key::Num2);
    let mids = [Vec3::new(0.0, 20.0, 10.0), Vec3::new(10.0, 20.0, 0.0), Vec3::new(0.0, 20.0, -10.0), Vec3::new(-10.0, 20.0, 0.0)];
    for (i, m) in mids.iter().enumerate() {
        let p = h.screen_of(*m);
        if i > 0 {
            h.hold(Modifiers::SHIFT);
        }
        h.move_to(p);
        h.click(p);
    }
    h.hold(Modifiers::NONE);
    eprintln!("picked edges: {:?}", h.st.poly_sel.as_ref().map(|s| s.edges.clone()));
    h.shot("pbevel-8-top-edges");
    h.move_to(start);
    h.hold(Modifiers::CTRL);
    h.key(Key::B);
    h.hold(Modifiers::NONE);
    h.key(Key::C);
    h.key(Key::CloseBracket);
    h.key(Key::CloseBracket);
    for i in 1..=8 {
        h.move_to(start + vec2(1.5 * i as f32, 0.0));
    }
    h.steps(2);
    h.shot("pbevel-9a-top-live");
    h.click(start + vec2(12.0, 0.0));
    h.steps(2);
    let b = h.st.poly_bevel.clone();
    eprintln!("mitre: width {:?} made {:?} segments {:?}", b.as_ref().map(|b| b.width), b.as_ref().map(|b| b.made), b.as_ref().map(|b| b.segments));
    h.shot("pbevel-9-top-mitre");
    // a cylinder beside: its top face's edges, rounded
    h.st.camera.target = Vec3::new(35.0, 10.0, 0.0);
    h.key(Key::Num4);
    h.key(Key::C);
    h.steps(2);
    h.st.camera.target = Vec3::new(17.0, 10.0, 0.0);
    h.st.camera.dist *= 1.5;
    h.steps(2);
    h.key(Key::Num3);
    let top = h.screen_of(Vec3::new(35.0, 20.0, 0.0));
    h.move_to(top);
    h.steps(2);
    h.click(top);
    h.steps(2);
    eprintln!("cylinder: selection {:?} mode {:?} picked faces {:?}", h.st.selection, h.st.sel_mode, h.st.poly_sel.as_ref().map(|s| (s.id, s.faces.clone())));
    let s0 = h.screen_of(Vec3::new(50.0, 10.0, 0.0));
    h.move_to(s0);
    h.hold(Modifiers::CTRL);
    h.key(Key::B);
    h.hold(Modifiers::NONE);
    h.key(Key::CloseBracket);
    h.key(Key::CloseBracket);
    if h.st.poly_bevel.as_ref().map(|b| b.chamfer).unwrap_or(false) {
        h.key(Key::C);
    }
    for i in 1..=6 {
        h.move_to(s0 + vec2(1.5 * i as f32, 0.0));
    }
    h.click(s0 + vec2(9.0, 0.0));
    h.steps(2);
    h.shot("pbevel-10-cylinder");
    // the smooth preview over the bevelled cube (the bevel holds its edges)
    h.key(Key::Num4);
    h.st.selection.clear();
    h.steps(2);
    h.shot("pbevel-11-both");
}

/// The Poly tab's Cut (Maya's Multi-Cut, K): points on edges joined across
/// the faces, a vertex taken, Ctrl + click an edge loop at a quarter.
fn poly_cut(h: &mut Harness) {
    use egui::Key;
    h.reset(Doc::default(), "Cut");
    h.key(Key::F3);
    h.steps(2);
    h.st.camera.target = Vec3::new(0.0, 10.0, 0.0);
    h.key(Key::B);
    h.steps(2);
    h.st.camera.yaw = 0.6;
    h.st.camera.pitch = 0.45;
    h.st.frame_current();
    h.settle();
    h.key(Key::K);
    // the front face (z = +10): its top edge at a third, its bottom edge at
    // two thirds; on round the right face to its far top corner
    let a = h.screen_of(Vec3::new(-10.0 + 20.0 / 3.0, 20.0, 10.0));
    h.move_to(a);
    h.steps(2);
    h.shot("pcut-1-hover");
    h.click(a);
    let b = h.screen_of(Vec3::new(10.0 - 20.0 / 3.0 - 1.0, 0.0, 10.0));
    h.move_to(b);
    h.steps(2);
    h.shot("pcut-2-line");
    h.click(b);
    // the front right vertical edge, then the right face's far top corner
    let c = h.screen_of(Vec3::new(10.0, 6.0, 10.0));
    h.click(c);
    let d = h.screen_of(Vec3::new(10.0, 20.0, -10.0));
    h.move_to(d);
    h.steps(2);
    h.shot("pcut-3-to-vertex");
    h.click(d);
    h.key(Key::Enter);
    h.steps(2);
    eprintln!("cut: tool {:?}, picked {:?}, undo {:?}", h.st.tool, h.st.poly_sel.as_ref().map(|s| s.edges.clone()), h.st.undo_labels().iter().rev().take(3).collect::<Vec<_>>());
    h.shot("pcut-4-done");
    // Ctrl + click on the left part of the top's back edge: a loop at a quarter
    h.key(Key::K);
    let e = h.screen_of(Vec3::new(-5.0, 20.0, -10.0));
    h.hold(Modifiers::CTRL);
    h.move_to(e);
    h.steps(2);
    h.shot("pcut-5-loop-preview");
    h.click(e);
    h.hold(Modifiers::NONE);
    h.key(Key::Enter);
    h.steps(2);
    h.shot("pcut-6-loop");
}

/// The Sculpt tab's Clay Buildup (V; Romain 20 nov., 15 h 40): square
/// strokes of clay, layered, beside the Clay brush's.
fn buildup(h: &mut Harness) {
    use egui::Key;
    h.reset(Doc::default(), "Buildup");
    h.key(Key::F2);
    h.steps(2);
    h.st.camera.target = Vec3::new(0.0, 10.0, 0.0);
    h.key(Key::B);
    h.settle();
    h.st.camera.yaw = 0.0;
    h.st.camera.pitch = 0.0;
    h.st.frame_current();
    h.settle();
    let c = Vec3::new(0.0, 10.0, 10.0);
    let centre = h.screen_of(c);
    let v = h.st.view();
    let k = 1.0 / v.pixel_size_at((c - v.eye).length());
    h.st.sculpt.radius = (10.0 * k * 0.22).max(12.0);
    h.st.sculpt.strength = 0.7;
    h.st.sculpt.mirror = false;
    h.key(Key::V);
    h.move_to(centre + vec2(-0.5, -0.45) * 10.0 * k);
    h.steps(2);
    h.shot("buildup-1-square");
    let along = |h: &mut Harness, a: (f32, f32), b: (f32, f32), n: usize| {
        let pa = centre + vec2(a.0, a.1) * 10.0 * k;
        let pb = centre + vec2(b.0, b.1) * 10.0 * k;
        h.drag(pa, pb, n, true);
        h.steps(2);
    };
    // three layers along the top, two across, one diagonal
    for _ in 0..3 {
        along(h, (-0.65, -0.45), (0.65, -0.45), 26);
    }
    for _ in 0..2 {
        along(h, (-0.45, -0.2), (-0.45, 0.6), 18);
    }
    along(h, (0.0, 0.0), (0.55, 0.55), 18);
    // Clay (C) below for comparison
    h.key(Key::C);
    along(h, (-0.1, 0.55), (0.65, 0.55), 18);
    h.key(Key::V);
    h.move_to(centre + vec2(0.3, 0.2) * 10.0 * k);
    h.steps(2);
    h.shot("buildup-2-strokes");
    h.st.camera.yaw = 0.7;
    h.st.camera.pitch = 0.5;
    h.steps(3);
    h.shot("buildup-3-side");
}

/// The Poly tab's smoothing angle (Maya's normal angle; Romain 20 nov.,
/// 17 h 15): a cube bevelled round in 3 rows, a cylinder and a sphere,
/// every edge hard, then at 30° (the rows, the side, the sphere smooth;
/// the cube's faces and the caps hard).
fn poly_angle(h: &mut Harness) {
    use egui::Key;
    h.reset(Doc::default(), "Smooth angle");
    h.key(Key::F3);
    h.steps(2);
    h.st.camera.target = Vec3::new(0.0, 10.0, 0.0);
    h.key(Key::B);
    h.steps(2);
    let cube = h.st.selection.clone();
    h.st.camera.yaw = 0.6;
    h.st.camera.pitch = 0.45;
    h.st.frame_current();
    h.settle();
    let start = h.screen_of(Vec3::new(0.0, 10.0, 0.0)) + vec2(160.0, 60.0);
    h.move_to(start);
    h.hold(Modifiers::CTRL);
    h.key(Key::B);
    h.hold(Modifiers::NONE);
    h.key(Key::C);
    h.key(Key::CloseBracket);
    h.key(Key::CloseBracket);
    h.move_to(start + vec2(3.0, 0.0));
    h.click(start + vec2(3.0, 0.0));
    h.steps(2);
    if let Some(b) = h.st.poly_bevel.clone() {
        h.st.set_poly_bevel(3.0, 3, false, true);
        h.st.end_live();
        eprintln!("bevel {} mm, {} segments, round {}", b.width, b.segments, !b.chamfer);
    }
    h.key(Key::Num4);
    h.st.camera.target = Vec3::new(28.0, 10.0, 0.0);
    h.key(Key::C);
    h.steps(2);
    h.st.camera.target = Vec3::new(56.0, 10.0, 0.0);
    h.key(Key::O);
    h.steps(2);
    h.st.selection.clear();
    h.st.poly_sel = None;
    h.key(Key::Q);
    h.st.camera.target = Vec3::new(33.0, 8.0, 0.0);
    h.st.camera.yaw = 0.35;
    h.st.camera.pitch = 0.42;
    h.st.camera.dist = 128.0;
    h.move_to(pos2(1200.0, 700.0));
    h.steps(3);
    let set = |h: &mut Harness, a: f32| {
        h.st.edit("Smooth angle", |d| {
            for f in d.features.iter_mut() {
                if let Some(m) = f.mesh.as_mut() {
                    m.make_mut().angle = a;
                }
            }
        });
        h.st.mesh_gen += 1;
        h.steps(2);
    };
    set(h, 0.0);
    h.shot("pangle-1-hard");
    set(h, 30.0);
    h.shot("pangle-2-angle-30");
    // the cube's window: Smooth angle, Hard / 30° / Soft
    h.st.selection = cube;
    h.steps(14);
    h.shot("pangle-3-window");
}

/// The change of tab animated (Romain 20 nov., 17 h 15): Forme → Sculpt →
/// Poly, a shot every two frames while the colours fade, the pill slides and
/// the tab's tools come in.
fn tab_anim(h: &mut Harness) {
    use egui::Key;
    h.reset(test_part(), "Tabs");
    h.st.settings.tab_anim = true;
    h.move_to(pos2(1200.0, 700.0));
    h.steps(2);
    h.shot("tabanim-00");
    let mut n = 1;
    for k in [Key::F2, Key::F3, Key::F1] {
        h.key(k);
        for _ in 0..11 {
            h.shot(&format!("tabanim-{n:02}"));
            n += 1;
            h.steps(2);
        }
        h.steps(4);
        h.shot(&format!("tabanim-{n:02}"));
        n += 1;
    }
}


/// The linked surface (Romain 20 nov., 18 h 12: "une surface… peut avoir les
/// trois états à la fois"): a path of four points made a NURBS surface, its
/// contour waved (Vertex mode, the gizmo), switched to Poly, four faces
/// extruded, back to NURBS (the extrusion rides on it, shown, not
/// editable), a corner raised, a Carve cylinder through the middle (a hole
/// in both states, the extrusion cut in half), back to Poly, the cylinder
/// moved.
fn linked(h: &mut Harness) {
    use egui::Key;
    use nurbs_app::linked::cvs_world;
    use nurbs_kernel::patch::{is_corner, PatchState, CONTOUR};
    h.reset(Doc::default(), "Linked surface");
    let view = |h: &mut Harness| {
        h.st.camera.yaw = 0.55;
        h.st.camera.pitch = 0.6;
        h.st.camera.target = Vec3::new(16.0, 2.0, 4.0);
        h.st.camera.dist = 175.0;
        h.steps(2);
    };
    view(h);
    // 1. the Path tool: four clicks, the first one again closes it
    h.key(Key::P);
    let g = |h: &Harness, x: f32, z: f32| h.screen_of(Vec3::new(x, 0.0, z));
    let first = g(h, -35.0, 25.0);
    h.click(first);
    h.click(g(h, 35.0, 25.0));
    h.click(g(h, 22.0, -25.0));
    h.click(g(h, -22.0, -25.0));
    h.move_to(first);
    h.steps(2);
    h.click(first);
    h.settle();
    let Some(path) = h.st.doc.features.iter().rev().find(|f| f.is_path()).map(|f| f.id) else {
        eprintln!("linked: no path made");
        return;
    };
    h.st.tool = Tool::Select;
    h.st.selection = vec![path];
    h.move_to(pos2(1200.0, 700.0));
    h.steps(14);
    h.shot("lsurf-01-path");
    // 2. its window's Linked surface button, clicked
    let made = |h: &Harness| h.st.single().and_then(|f| nurbs_app::linked::patch_of(f)).is_some();
    h.move_to(pos2(1206.0, 552.0));
    h.steps(2);
    h.click(pos2(1206.0, 552.0));
    h.steps(3);
    if made(h) {
        eprintln!("linked: made by its window's button");
    } else {
        eprintln!("linked: the button was not hit, made directly");
        if !h.st.make_linked_surface() {
            eprintln!("linked: not made");
            return;
        }
    }
    let id = h.st.selection[0];
    h.steps(14);
    h.shot("lsurf-02-surface");
    // 3. the contour waved: Vertex mode, a point clicked, the gizmo's Y arrow
    h.key(Key::Num1);
    h.key(Key::W);
    h.steps(2);
    let cv = |h: &Harness| cvs_world(h.st.doc.feature(id).unwrap()).unwrap();
    let lift = |h: &mut Harness, k: usize, d: f32| {
        let c = cv(h)[k];
        h.click(h.screen_of(c));
        h.steps(2);
        let s0 = h.screen_of(c);
        let dir = (h.screen_of(c + Vec3::Y * 10.0) - s0).normalized();
        let a = s0 + dir * 50.0;
        let b = h.screen_of(c + Vec3::Y * d) + dir * 50.0;
        h.move_to(a);
        h.steps(2);
        h.drag(a, b, 12, true);
        h.steps(2);
        let after = cv(h)[k];
        eprintln!("linked: point {k} moved {:.2} mm up", after.y - c.y);
    };
    let c = cv(h);
    let mut tangents: Vec<usize> = CONTOUR.iter().copied().filter(|k| !is_corner(*k)).collect();
    tangents.sort_by(|a, b| c[*b].z.total_cmp(&c[*a].z));
    let (f1, f2, back) = (tangents[0], tangents[1], tangents[tangents.len() - 1]);
    lift(h, f1, 12.0);
    lift(h, f2, -9.0);
    lift(h, back, 8.0);
    h.move_to(pos2(1200.0, 700.0));
    h.steps(4);
    h.shot("lsurf-03-wave");
    h.key(Key::Num4);
    h.st.selection = vec![id];
    h.steps(14);
    h.shot("lsurf-04-wave-object");
    // 4. POLY in its window, clicked: the Poly tab, the layer as a mesh
    h.move_to(pos2(993.0, 455.0));
    h.steps(2);
    h.click(pos2(993.0, 455.0));
    h.steps(3);
    let now = nurbs_app::linked::state_of(h.st.doc.feature(id).unwrap());
    if now == Some(PatchState::Poly) {
        eprintln!("linked: POLY clicked in its window");
    } else {
        eprintln!("linked: POLY not hit ({now:?}), switched directly");
        h.st.set_patch_state(id, PatchState::Poly);
    }
    h.steps(14);
    h.shot("lsurf-05-poly");
    // 5. Face mode: four faces (two by two, from the middle), Ctrl + E
    h.key(Key::Num3);
    h.steps(2);
    let f = h.st.doc.feature(id).unwrap().clone();
    let pts = nurbs_app::meshes::world_points(&f);
    let m = f.mesh.clone().unwrap();
    let bind = m.patch.as_ref().unwrap().bind.clone();
    let count = |k: usize| {
        let mut v: Vec<i32> = bind.iter().map(|b| (b[k] * 1000.0).round() as i32).collect();
        v.sort();
        v.dedup();
        v.len() - 1
    };
    let (nu, nv) = (count(0), count(1));
    let du = 1.0 / nu as f32;
    let dv = 1.0 / nv as f32;
    // the grid's faces two by two from the middle (j * nu + i)
    let mut picks = vec![];
    for j in [nv / 2, nv / 2 + 1] {
        for i in [nu / 2, nu / 2 + 1] {
            let q = &m.faces[j * nu + i];
            picks.push((j * nu + i, q.iter().map(|x| pts[*x as usize]).sum::<Vec3>() / q.len() as f32));
        }
    }
    let _ = (du, dv);
    eprintln!("linked: grid {nu} x {nv}, {} faces picked", picks.len());
    for (n, (_, c)) in picks.iter().enumerate() {
        if n > 0 {
            h.hold(Modifiers::SHIFT);
        }
        h.click(h.screen_of(*c));
        h.hold(Modifiers::NONE);
    }
    let mid = picks.iter().map(|p| p.1).sum::<Vec3>() / picks.len().max(1) as f32;
    let at = h.screen_of(mid);
    h.move_to(at);
    h.steps(2);
    h.shot("lsurf-06-faces");
    h.hold(Modifiers::CTRL);
    h.key(Key::E);
    h.hold(Modifiers::NONE);
    let up = h.screen_of(mid + Vec3::Y * 7.0);
    for k in 1..=8 {
        h.move_to(at + (up - at) * (k as f32 / 8.0));
    }
    h.steps(2);
    h.click(up);
    h.settle();
    h.move_to(pos2(1200.0, 700.0));
    h.steps(4);
    h.shot("lsurf-07-extruded");
    // 6. NURBS again: the extrusion rides on the surface, shown, not editable
    h.st.set_patch_state(id, PatchState::Nurbs);
    h.key(Key::Num4);
    h.st.selection = vec![id];
    h.steps(14);
    h.shot("lsurf-08-nurbs");
    // 7. the contour moved: a corner up (its tangent points with it), a
    //    side's point down; the Poly layer follows
    h.key(Key::Num1);
    h.st.selection = vec![id];
    h.steps(2);
    let c = cv(h);
    // the corner nearest the extrusion
    let corner = [0usize, 3, 12, 15].into_iter().min_by(|a, b| c[*a].distance(mid).total_cmp(&c[*b].distance(mid))).unwrap();
    lift(h, corner, 16.0);
    h.move_to(pos2(1200.0, 700.0));
    h.steps(4);
    h.shot("lsurf-09-contour");
    h.key(Key::Num4);
    h.st.selection = vec![id];
    h.steps(14);
    h.shot("lsurf-10-follows");
    // 8. a Cylinder in Carve through the middle and the extrusion (the
    //    tool on the ground, then its base put under the surface and its
    //    height through it, as the Shape window does)
    let f = h.st.doc.feature(id).unwrap().clone();
    let p = f.mesh.as_ref().unwrap().patch.clone().unwrap();
    let cvm = p.cvs(nurbs_app::meshes::size(&f));
    let x = nurbs_app::linked::to_world(&f);
    // the patch's middle along u (the extrusion's -u side), the
    // extrusion's middle along v: it takes the extrusion's -u half
    let (bu, bv) = ((nu / 2) as f32 * du, (nv / 2 + 1) as f32 * dv);
    let centre = x.transform_point3(nurbs_kernel::patch::eval(&cvm, bu, bv).0);
    let r = 1.25 * (x.transform_point3(nurbs_kernel::patch::eval(&cvm, 0.5 + du, 0.5).0) - x.transform_point3(nurbs_kernel::patch::eval(&cvm, 0.5, 0.5).0)).length();
    h.st.selection.clear();
    h.st.mode = Op::Carve;
    h.st.tool = Tool::Create(PrimKind::Cylinder);
    // pressed on the surface (its plane there, square to Y), dragged out to
    // its radius, then its height: a Carve made there goes through it
    let a = h.screen_of(centre);
    let b = h.screen_of(centre + Vec3::new(r, 0.0, 0.0));
    h.move_to(a);
    h.steps(2);
    h.drag(a, b, 8, true);
    let top = h.screen_of(centre + Vec3::new(0.0, 20.0, 0.0));
    h.move_to(top);
    h.settle();
    h.click(top);
    h.settle();
    h.st.tool = Tool::Select;
    h.st.mode = Op::Add;
    let Some(cyl) = h.st.doc.features.iter().rev().find(|f| matches!(f.prim, Prim::Cylinder { .. })).map(|f| f.id) else {
        eprintln!("linked: no cylinder");
        return;
    };
    let cf = h.st.doc.feature(cyl).unwrap();
    eprintln!("linked: cylinder {:?} {:?} at {:?}", cf.op, cf.prim, cf.origin);
    h.st.selection = vec![cyl];
    h.key(Key::Num4);
    h.st.selection = vec![cyl];
    h.steps(14);
    h.shot("lsurf-11-carve");
    h.st.selection = vec![id];
    h.steps(14);
    h.shot("lsurf-12-carve-nurbs");
    // 9. POLY again: the hole and the extrusion cut in half, a mesh
    h.st.set_patch_state(id, PatchState::Poly);
    h.steps(14);
    h.shot("lsurf-13-carve-poly");
    h.key(Key::Num3);
    h.steps(2);
    h.move_to(h.screen_of(mid + Vec3::new(0.0, 0.0, 0.0)));
    h.steps(4);
    h.shot("lsurf-14-carve-poly-faces");
    // 10. the cylinder moved along X: the hole moves, in both states
    h.key(Key::Num4);
    h.st.edit("Move", |d| {
        if let Some(c) = d.feature_mut(cyl) {
            c.origin.x -= 14.0;
        }
    });
    h.st.selection = vec![id];
    h.move_to(pos2(1200.0, 700.0));
    h.steps(14);
    h.shot("lsurf-15-carve-moved-poly");
    h.st.set_patch_state(id, PatchState::Nurbs);
    h.key(Key::Num4);
    h.st.selection = vec![id];
    h.steps(14);
    h.shot("lsurf-16-carve-moved-nurbs");
    // 11. close, nothing selected: the hole and the halved extrusion in
    // both states, the cylinder back under the extrusion
    h.st.edit("Move", |d| {
        if let Some(c) = d.feature_mut(cyl) {
            c.origin.x += 14.0;
        }
    });
    h.st.selection.clear();
    h.st.camera.yaw = 0.35;
    h.st.camera.pitch = 0.75;
    h.st.camera.target = mid + Vec3::new(4.0, 0.0, 0.0);
    h.st.camera.dist = 95.0;
    h.move_to(pos2(1500.0, 120.0));
    h.steps(14);
    h.shot("lsurf-17-close-nurbs");
    h.st.set_patch_state(id, PatchState::Poly);
    h.st.selection.clear();
    h.steps(14);
    h.shot("lsurf-18-close-poly");
    if std::env::var("FORME_LSURF_CORNER").is_ok() {
        h.st.camera.target = centre + Vec3::new(2.0, 0.0, r);
        h.st.camera.dist = 30.0;
        h.steps(6);
        h.shot("lsurf-19-corner-poly");
        h.st.camera.yaw = 0.35 + 0.9;
        h.steps(6);
        h.shot("lsurf-19-corner-poly-side");
        h.st.camera.yaw = 0.35;
        h.st.set_patch_state(id, PatchState::Nurbs);
        h.st.selection.clear();
        h.steps(10);
        h.shot("lsurf-19-corner-nurbs");
    }
    h.st.set_patch_state(id, PatchState::Nurbs);
    h.st.selection.clear();
    h.steps(4);
    let f = h.st.doc.feature(id).unwrap();
    let m = f.mesh.as_ref().unwrap();
    eprintln!("linked: layer {} points, {} faces, {} off the surface; state {:?}", m.verts.len(), m.faces.len(), m.patch.as_ref().unwrap().off_surface().iter().filter(|o| **o).count(), m.patch.as_ref().unwrap().state);
    // 12. scaled 1.5 times in Object mode, then a contour point moved: the
    // extrusion keeps its new height (bound again where it is)
    let tall = |h: &Harness| h.st.doc.feature(id).unwrap().mesh.as_ref().unwrap().patch.as_ref().unwrap().bind.iter().map(|b| b[4]).fold(0.0f32, f32::max);
    let before = tall(h);
    h.st.edit("Scale", |d| {
        if let Some(g) = d.feature_mut(id) {
            if let Prim::Box { x, y, z } = &mut g.prim {
                *x *= 1.5;
                *y *= 1.5;
                *z *= 1.5;
            }
        }
    });
    h.st.move_patch_points(id, &[0], Vec3::new(0.0, 2.0, 0.0));
    eprintln!("linked: extrusion {:.2} mm, scaled 1.5 then a point moved: {:.2} mm", before, tall(h));
    h.st.undo();
    h.st.undo();
}

/// Linked objects (Romain, 20 nov., 23 h: "pousser le Link surface à
/// cylindre et cube… faire aussi le lien avec le Sculpt mode"): a Cube and a
/// Cylinder made linked objects; the cube's top corner raised in NURBS, the
/// cylinder made a barrel; the cube's top extruded in Poly; strokes on it in
/// Sculpt; back in NURBS the clay shown and riding on a corner moved again;
/// the cylinder sculpted and its top ring widened.
fn linked_objects(h: &mut Harness) {
    use egui::Key;
    use nurbs_app::linked::{cvs_world, patch_of};
    use nurbs_kernel::patch::PatchState;
    let mut doc = Doc::default();
    let cube = doc.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, glam::DVec3::new(-35.0, 0.0, 0.0), glam::DQuat::IDENTITY);
    let cyl = doc.add(Op::Add, Prim::Cylinder { radius: 12.0, height: 30.0 }, glam::DVec3::new(30.0, 0.0, 0.0), glam::DQuat::IDENTITY);
    h.reset(doc, "Linked objects");
    let wide = |h: &mut Harness| {
        h.st.camera.yaw = 0.55;
        h.st.camera.pitch = 0.5;
        h.st.camera.target = Vec3::new(-2.0, 14.0, 0.0);
        h.st.camera.dist = 190.0;
        h.steps(2);
    };
    wide(h);
    h.settle();
    h.shot("lobj-01-shapes");
    h.st.selection = vec![cube];
    h.move_to(pos2(1500.0, 120.0));
    h.steps(14);
    h.shot("lobj-02-cube-window");
    let lc = if h.st.make_linked_solid() { h.st.selection[0] } else {
        eprintln!("lobj: cube not made");
        return;
    };
    h.st.selection = vec![cyl];
    h.steps(2);
    let ly = if h.st.make_linked_solid() { h.st.selection[0] } else {
        eprintln!("lobj: cylinder not made");
        return;
    };
    for id in [lc, ly] {
        let f = h.st.doc.feature(id).unwrap();
        let p = patch_of(f).unwrap();
        let m = f.mesh.as_ref().unwrap();
        eprintln!("lobj: {}: {} patches, {} contour points, layer {} points {} faces", f.name, p.count(), p.contour().len(), m.verts.len(), m.faces.len());
    }
    h.st.selection.clear();
    h.settle();
    h.shot("lobj-03-linked");
    // the cube's contour (Vertex mode)
    h.st.selection = vec![lc];
    h.key(Key::Num1);
    h.st.selection = vec![lc];
    h.steps(4);
    h.shot("lobj-04-cube-contour");
    let near = |h: &Harness, id: nurbs_kernel::FeatureId, w: Vec3| -> usize {
        let c = cvs_world(h.st.doc.feature(id).unwrap()).unwrap();
        let p = patch_of(h.st.doc.feature(id).unwrap()).unwrap();
        p.contour().into_iter().min_by(|a, b| c[*a].distance(w).total_cmp(&c[*b].distance(w))).unwrap()
    };
    let k = near(h, lc, Vec3::new(-15.0, 20.0, 15.0));
    h.st.patch_sel = Some(nurbs_app::linked::PatchSel { id: lc, cvs: vec![k] });
    h.st.move_patch_points(lc, &[k], Vec3::new(0.0, 12.0, 0.0));
    h.steps(4);
    h.shot("lobj-05-cube-corner");
    // the cylinder a barrel: the side's seam tangents out, the top ring in
    h.key(Key::Num4);
    h.st.selection = vec![ly];
    h.steps(2);
    {
        let c = cvs_world(h.st.doc.feature(ly).unwrap()).unwrap();
        let p = patch_of(h.st.doc.feature(ly).unwrap()).unwrap().clone();
        let axis = Vec3::new(30.0, 0.0, 0.0);
        let mid: Vec<usize> = p.contour().into_iter().filter(|k| c[*k].y > 2.0 && c[*k].y < 28.0).collect();
        eprintln!("lobj: {} tangent points up the side", mid.len());
        for k in mid {
            let out = Vec3::new(c[k].x - axis.x, 0.0, c[k].z - axis.z).normalize() * 6.0;
            h.st.move_patch_points(ly, &[k], out);
        }
        let corners: Vec<usize> = p.contour().into_iter().filter(|k| p.corner(*k) && c[*k].y > 29.0).collect();
        for k in corners {
            let inn = -Vec3::new(c[k].x - axis.x, 0.0, c[k].z - axis.z).normalize() * 4.0;
            h.st.move_patch_points(ly, &[k], inn);
        }
    }
    h.st.selection = vec![ly];
    h.key(Key::Num1);
    h.st.selection = vec![ly];
    h.steps(4);
    h.shot("lobj-06-cyl-barrel");
    h.key(Key::Num4);
    h.st.selection.clear();
    h.steps(4);
    h.shot("lobj-07-both");
    // the cube to Poly: its top's middle face extruded 8 mm
    h.st.selection = vec![lc];
    h.st.set_patch_state(lc, PatchState::Poly);
    h.key(Key::Num3);
    h.st.selection = vec![lc];
    h.steps(2);
    let f = h.st.doc.feature(lc).unwrap().clone();
    let pts = nurbs_app::meshes::world_points(&f);
    let m = f.mesh.clone().unwrap();
    let top_mid = Vec3::new(-35.0, 20.0, 0.0);
    let (fi, fc) = m
        .faces
        .iter()
        .enumerate()
        .map(|(i, q)| (i, q.iter().map(|x| pts[*x as usize]).sum::<Vec3>() / q.len() as f32))
        .filter(|(_, c)| c.y > 19.0)
        .min_by(|a, b| a.1.distance(top_mid).total_cmp(&b.1.distance(top_mid)))
        .unwrap();
    eprintln!("lobj: top face {fi} at {fc}");
    h.click(h.screen_of(fc));
    h.steps(2);
    let at = h.screen_of(fc);
    h.move_to(at);
    h.hold(Modifiers::CTRL);
    h.key(Key::E);
    h.hold(Modifiers::NONE);
    let up = h.screen_of(fc + Vec3::Y * 8.0);
    for k in 1..=8 {
        h.move_to(at + (up - at) * (k as f32 / 8.0));
    }
    h.steps(2);
    h.click(up);
    h.settle();
    h.move_to(pos2(1500.0, 120.0));
    h.steps(4);
    eprintln!("lobj: cube layer now {} faces", h.st.doc.feature(lc).unwrap().mesh.as_ref().unwrap().faces.len());
    h.shot("lobj-08-cube-poly");
    // Sculpt: the Poly layer as clay; Draw strokes across its front
    h.st.set_patch_state(lc, PatchState::Sculpt);
    h.steps(2);
    let f = h.st.doc.feature(lc).unwrap().clone();
    eprintln!("lobj: clay {} points, {} faces, tool {:?}", f.mesh.as_ref().unwrap().verts.len(), f.mesh.as_ref().unwrap().faces.len(), h.st.tool);
    h.st.camera.target = Vec3::new(-35.0, 14.0, 10.0);
    h.st.camera.dist = 110.0;
    h.steps(2);
    let front = Vec3::new(-35.0, 10.0, 15.0);
    let v = h.st.view();
    let kpx = 1.0 / v.pixel_size_at((front - v.eye).length());
    h.st.sculpt.radius = 5.0 * kpx;
    h.st.sculpt.strength = 0.9;
    h.st.sculpt.mirror = false;
    for (a, b) in [(Vec3::new(-50.0, 12.0, 15.0), Vec3::new(-20.0, 12.0, 15.0)), (Vec3::new(-48.0, 6.0, 15.0), Vec3::new(-22.0, 6.0, 15.0))] {
        for _ in 0..3 {
            let (pa, pb) = (h.screen_of(a), h.screen_of(b));
            h.drag(pa, pb, 24, true);
            h.steps(2);
        }
    }
    // a dent with Ctrl
    h.hold(Modifiers::CTRL);
    let (pa, pb) = (h.screen_of(Vec3::new(-40.0, 15.0, 15.0)), h.screen_of(Vec3::new(-30.0, 15.0, 15.0)));
    h.drag(pa, pb, 10, true);
    h.hold(Modifiers::NONE);
    h.steps(2);
    h.move_to(pos2(1500.0, 120.0));
    h.steps(4);
    eprintln!("lobj: undo after the strokes: {:?}", h.st.undo_labels().iter().rev().take(4).collect::<Vec<_>>());
    h.shot("lobj-09-cube-sculpt");
    // NURBS: the clay over the surface; another top corner raised, the
    // clay rides on it
    h.st.set_patch_state(lc, PatchState::Nurbs);
    h.key(Key::Num4);
    h.st.selection = vec![lc];
    h.steps(4);
    h.shot("lobj-10-cube-nurbs-clay");
    let shown = |h: &Harness, id: nurbs_kernel::FeatureId| nurbs_app::linked::sculpt_shown(h.st.doc.feature(id).unwrap()).is_some();
    eprintln!("lobj: clay shown in NURBS: {}", shown(h, lc));
    let k = near(h, lc, Vec3::new(-55.0, 20.0, 15.0));
    h.st.move_patch_points(lc, &[k], Vec3::new(-6.0, 10.0, 6.0));
    h.key(Key::Num1);
    h.st.selection = vec![lc];
    h.st.patch_sel = Some(nurbs_app::linked::PatchSel { id: lc, cvs: vec![k] });
    h.steps(4);
    h.shot("lobj-11-clay-follows");
    h.key(Key::Num4);
    h.st.selection.clear();
    h.steps(4);
    h.shot("lobj-12-clay-follows-object");
    // back in Poly: the layer, the clay kept; Sculpt again: it comes back
    h.st.set_patch_state(lc, PatchState::Poly);
    h.st.selection.clear();
    h.steps(4);
    h.shot("lobj-13-cube-poly-again");
    h.st.set_patch_state(lc, PatchState::Sculpt);
    h.st.selection.clear();
    h.steps(4);
    h.shot("lobj-14-cube-sculpt-again");
    h.st.set_patch_state(lc, PatchState::Nurbs);
    // the cylinder: a ring of clay round its side, then its top ring wider
    wide(h);
    h.st.camera.target = Vec3::new(30.0, 15.0, 0.0);
    h.st.camera.dist = 120.0;
    h.st.selection = vec![ly];
    h.st.set_patch_state(ly, PatchState::Sculpt);
    h.steps(2);
    // a ring of strokes round the side facing the eye (points inside the
    // clay: their place on screen is on its front)
    let c0 = Vec3::new(30.0, 15.0, 0.0);
    let eye = h.st.view().eye;
    let face = Vec3::new(eye.x - c0.x, 0.0, eye.z - c0.z).normalize();
    let side = Vec3::Y.cross(face);
    let v = h.st.view();
    h.st.sculpt.radius = 4.0 / v.pixel_size_at((c0 - v.eye).length());
    for (a, b) in [(-0.9f32, 0.9f32)] {
        let r = 9.0;
        for _ in 0..3 {
            let pts: Vec<Pos2> = (0..=16).map(|i| {
                let t = a + (b - a) * i as f32 / 16.0;
                h.screen_of(c0 + (face * t.cos() + side * t.sin()) * r)
            }).collect();
            h.press(pts[0], true);
            for p in &pts[1..] {
                h.move_to(*p);
            }
            h.press(pts[pts.len() - 1], false);
            h.steps(2);
        }
    }
    h.move_to(pos2(1500.0, 120.0));
    h.steps(4);
    h.shot("lobj-15-cyl-sculpt");
    h.st.set_patch_state(ly, PatchState::Nurbs);
    {
        let c = cvs_world(h.st.doc.feature(ly).unwrap()).unwrap();
        let p = patch_of(h.st.doc.feature(ly).unwrap()).unwrap().clone();
        let axis = Vec3::new(30.0, 0.0, 0.0);
        let corners: Vec<usize> = p.contour().into_iter().filter(|k| p.corner(*k) && c[*k].y > 29.0).collect();
        for k in corners {
            let out = Vec3::new(c[k].x - axis.x, 0.0, c[k].z - axis.z).normalize() * 9.0 + Vec3::Y * 4.0;
            h.st.move_patch_points(ly, &[k], out);
        }
    }
    h.st.selection = vec![ly];
    h.key(Key::Num4);
    h.st.selection = vec![ly];
    h.steps(4);
    eprintln!("lobj: cylinder clay shown in NURBS: {}", shown(h, ly));
    h.shot("lobj-16-cyl-clay-follows");
    wide(h);
    h.st.selection.clear();
    h.steps(4);
    h.shot("lobj-17-all");
    // undo all the way back and forth: the states come back as they were
    let n = h.st.undo_labels().len();
    for _ in 0..n {
        h.st.undo();
    }
    h.steps(2);
    for _ in 0..n {
        h.st.redo();
    }
    h.steps(4);
    eprintln!("lobj: undone and redone {n} steps; clay shown {} {}", shown(h, lc), shown(h, ly));
    h.shot("lobj-18-redone");
}

/// The linked surface on a busier part (Romain, 20 nov., 20 h 58: "Test
/// plus complexe comme modèle"): a twisted surface, in Poly a rib bevelled
/// round, a stepped boss, a pocket, an island, an edge loop over them; back
/// in NURBS its contour pulled far; a slot and two holes carved on it.
fn linked_complex(h: &mut Harness) {
    use egui::Key;
    use nurbs_app::linked::cvs_world;
    use nurbs_app::poly::PolySel;
    use nurbs_kernel::patch::{is_corner, PatchState, CONTOUR};
    h.reset(Doc::default(), "Linked surface, complex");
    let view = |h: &mut Harness| {
        h.st.camera.yaw = 0.55;
        h.st.camera.pitch = 0.62;
        h.st.camera.target = Vec3::new(0.0, 4.0, 0.0);
        h.st.camera.dist = 255.0;
        h.steps(2);
    };
    let top = |h: &mut Harness| {
        h.st.camera.yaw = 0.0;
        h.st.camera.pitch = 1.35;
        h.st.camera.target = Vec3::new(0.0, 0.0, 15.0);
        h.st.camera.dist = 300.0;
        h.steps(2);
    };
    let park = |h: &mut Harness, n: usize| {
        h.move_to(pos2(1200.0, 700.0));
        h.steps(n);
    };
    top(h);
    // 1. a trapezoid Path, 120 x 84 (drawn from above: it all fits there)
    h.key(Key::P);
    let g = |h: &Harness, x: f32, z: f32| h.screen_of(Vec3::new(x, 0.0, z));
    let first = g(h, -60.0, 42.0);
    h.click(first);
    h.click(g(h, 60.0, 42.0));
    h.click(g(h, 46.0, -42.0));
    h.click(g(h, -46.0, -42.0));
    h.move_to(first);
    h.steps(2);
    h.click(first);
    h.settle();
    let Some(path) = h.st.doc.features.iter().rev().find(|f| f.is_path()).map(|f| f.id) else {
        eprintln!("lsurfx: no path made");
        return;
    };
    h.st.tool = Tool::Select;
    h.st.selection = vec![path];
    if !h.st.make_linked_surface() {
        let f = h.st.doc.feature(path).unwrap();
        eprintln!("lsurfx: not made: {:?} {:?}", f.prim, f.profile);
        return;
    }
    let id = h.st.selection[0];
    view(h);
    // 2. twisted: the front side's tangents up and down, the back's the
    //    other way, a side's up (Vertex mode, the gizmo's Y arrow)
    h.key(Key::Num1);
    h.key(Key::W);
    h.steps(2);
    let cv = |h: &Harness| cvs_world(h.st.doc.feature(id).unwrap()).unwrap();
    let lift = |h: &mut Harness, k: usize, d: f32| {
        let c = cv(h)[k];
        h.click(h.screen_of(c));
        h.steps(2);
        let s0 = h.screen_of(c);
        let dir = (h.screen_of(c + Vec3::Y * 10.0) - s0).normalized();
        let a = s0 + dir * 50.0;
        let b = h.screen_of(c + Vec3::Y * d) + dir * 50.0;
        h.move_to(a);
        h.steps(2);
        h.drag(a, b, 12, true);
        h.steps(2);
        let after = cv(h)[k];
        eprintln!("lsurfx: point {k} moved {:.2} mm up", after.y - c.y);
    };
    let c = cv(h);
    let mut tangents: Vec<usize> = CONTOUR.iter().copied().filter(|k| !is_corner(*k)).collect();
    tangents.sort_by(|a, b| c[*b].z.total_cmp(&c[*a].z));
    let front = {
        let mut f = vec![tangents[0], tangents[1]];
        f.sort_by(|a, b| c[*a].x.total_cmp(&c[*b].x));
        f
    };
    let back = {
        let n = tangents.len();
        let mut f = vec![tangents[n - 1], tangents[n - 2]];
        f.sort_by(|a, b| c[*a].x.total_cmp(&c[*b].x));
        f
    };
    let side = tangents[2..tangents.len() - 2].iter().copied().min_by(|a, b| c[*a].x.total_cmp(&c[*b].x)).unwrap();
    lift(h, front[0], 16.0);
    lift(h, front[1], -10.0);
    lift(h, back[0], -8.0);
    lift(h, back[1], 14.0);
    lift(h, side, 10.0);
    h.key(Key::Num4);
    h.st.selection = vec![id];
    park(h, 14);
    h.shot("lsurfx-01-twisted");
    // 3. Poly: the layer's grid read once (its faces' places on screen
    //    from above stay while they are not touched)
    h.st.set_patch_state(id, PatchState::Poly);
    h.steps(4);
    let f = h.st.doc.feature(id).unwrap().clone();
    let pts = nurbs_app::meshes::world_points(&f);
    let m = f.mesh.clone().unwrap();
    let bind = m.patch.as_ref().unwrap().bind.clone();
    let count = |k: usize| {
        let mut v: Vec<i32> = bind.iter().map(|b| (b[k] * 1000.0).round() as i32).collect();
        v.sort();
        v.dedup();
        v.len() - 1
    };
    let (nu, nv) = (count(0), count(1));
    eprintln!("lsurfx: grid {nu} x {nv}");
    let face_c = |i: usize, j: usize| {
        let q = &m.faces[j * nu + i];
        q.iter().map(|x| pts[*x as usize]).sum::<Vec3>() / q.len() as f32
    };
    // Face mode, faces clicked from above (Shift adds)
    let pick = |h: &mut Harness, cells: &[(usize, usize)]| {
        top(h);
        h.key(Key::Num3);
        h.steps(2);
        for (n, (i, j)) in cells.iter().enumerate() {
            if n > 0 {
                h.hold(Modifiers::SHIFT);
            }
            h.click(h.screen_of(face_c(*i, *j)));
            h.hold(Modifiers::NONE);
        }
        let got = h.st.poly_sel.as_ref().map(|s| s.faces.len()).unwrap_or(0);
        eprintln!("lsurfx: {} of {} faces picked", got, cells.len());
        view(h);
    };
    let centre_of = |cells: &[(usize, usize)]| cells.iter().map(|(i, j)| face_c(*i, *j)).sum::<Vec3>() / cells.len() as f32;
    // Ctrl + E, the pointer along the normal by `d`, a click
    let extrude = |h: &mut Harness, at3: Vec3, d: f32| {
        let at = h.screen_of(at3);
        h.move_to(at);
        h.steps(2);
        h.hold(Modifiers::CTRL);
        h.key(Key::E);
        h.hold(Modifiers::NONE);
        let to = h.screen_of(at3 + Vec3::Y * d);
        for k in 1..=8 {
            h.move_to(at + (to - at) * (k as f32 / 8.0));
        }
        h.steps(2);
        let label = h.st.gizmo_readout.clone();
        h.click(to);
        h.settle();
        eprintln!("lsurfx: {label:?}");
    };
    // I, the pointer from 60 to 36 points off the faces' centre, a click
    let inset = |h: &mut Harness, at3: Vec3| {
        let c = h.screen_of(at3);
        h.move_to(c + vec2(60.0, 0.0));
        h.steps(2);
        h.key(Key::I);
        for k in 1..=6 {
            h.move_to(c + vec2(60.0 - 4.0 * k as f32, 0.0));
        }
        h.steps(2);
        let label = h.st.gizmo_readout.clone();
        h.click(c + vec2(36.0, 0.0));
        h.settle();
        eprintln!("lsurfx: {label:?}");
    };
    let jm = nv / 2;
    // the rib: the middle row, its ends left flat
    let rib: Vec<(usize, usize)> = (2..nu - 2).map(|i| (i, jm)).collect();
    let boss: Vec<(usize, usize)> = vec![(2, 1), (3, 1), (2, 2), (3, 2)];
    let pocket: Vec<(usize, usize)> = vec![(nu - 4, 1), (nu - 3, 1), (nu - 4, 2), (nu - 3, 2)];
    let island: Vec<(usize, usize)> = vec![(nu / 2 - 1, nv - 3), (nu / 2, nv - 3), (nu / 2 - 1, nv - 2), (nu / 2, nv - 2)];
    let (rib_c, boss_c, pocket_c, island_c) = (centre_of(&rib), centre_of(&boss), centre_of(&pocket), centre_of(&island));
    // their places on the surface (u, v), to find them once it moved
    let uv_of = |cells: &[(usize, usize)]| {
        let mut t = [0.0f32; 2];
        let mut n = 0.0;
        for (i, j) in cells {
            for v in &m.faces[j * nu + i] {
                t[0] += bind[*v as usize][0];
                t[1] += bind[*v as usize][1];
                n += 1.0;
            }
        }
        [t[0] / n, t[1] / n]
    };
    let (island_uv, pocket_uv, boss_uv) = (uv_of(&island), uv_of(&pocket), uv_of(&boss));
    // the loop's edge: face (0, 1)'s border at u = 0 (the ring runs along u
    // through row 1, over the boss and into the pocket)
    let loop_at = {
        let q = &m.faces[nu];
        let mut vs: Vec<u32> = q.clone();
        vs.sort_by(|a, b| bind[*a as usize][0].total_cmp(&bind[*b as usize][0]));
        let e = (pts[vs[0] as usize] + pts[vs[1] as usize]) * 0.5;
        e + (face_c(0, 1) - e) * 0.2
    };
    park(h, 12);
    h.shot("lsurfx-02-poly");
    // 4. the rib: extruded 6 mm, its top's rim bevelled round (3 segments)
    pick(h, &rib);
    extrude(h, rib_c, 6.0);
    park(h, 6);
    h.shot("lsurfx-03-rib");
    let tops = h.st.poly_sel.as_ref().map(|s| s.faces.clone()).unwrap_or_default();
    let mut rim: Vec<(u32, u32)> = vec![];
    {
        let f = h.st.doc.feature(id).unwrap();
        let m = f.mesh.as_ref().unwrap();
        let mut seen: std::collections::HashMap<(u32, u32), u32> = Default::default();
        for t in &tops {
            let q = &m.faces[*t as usize];
            for k in 0..q.len() {
                let (a, b) = (q[k], q[(k + 1) % q.len()]);
                *seen.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        rim.extend(seen.into_iter().filter(|(_, n)| *n == 1).map(|(e, _)| e));
        rim.sort();
    }
    eprintln!("lsurfx: rib top {} faces, rim {} edges", tops.len(), rim.len());
    h.st.sel_mode = SelMode::Edge;
    h.st.poly_sel = Some(PolySel { id, edges: rim, ..Default::default() });
    let start = h.screen_of(rib_c + Vec3::Y * 6.0) + vec2(120.0, 80.0);
    h.move_to(start);
    h.steps(2);
    h.hold(Modifiers::CTRL);
    h.key(Key::B);
    h.hold(Modifiers::NONE);
    h.key(Key::CloseBracket);
    h.key(Key::CloseBracket);
    h.key(Key::C);
    let (w0, mpp) = h.st.poly_bevel.as_ref().map(|b| (b.w0, b.mm_per_pt)).unwrap_or((0.5, 0.2));
    let to = start + vec2(((1.6 - w0) / mpp).max(0.0), 0.0);
    for k in 1..=6 {
        h.move_to(start + (to - start) * (k as f32 / 6.0));
    }
    h.steps(2);
    h.shot("lsurfx-04-rib-bevel-live");
    h.click(to);
    h.steps(3);
    let b = h.st.poly_bevel.clone();
    eprintln!("lsurfx: bevel made {:?} segments {:?} chamfer {:?} faces {:?}", b.as_ref().map(|b| b.made), b.as_ref().map(|b| b.segments), b.as_ref().map(|b| b.chamfer), b.as_ref().map(|b| b.faces.len()));
    h.st.poly_sel = None;
    // 5. the stepped boss: 4 faces out 5 mm, their top inset, out 3 more
    pick(h, &boss);
    extrude(h, boss_c, 5.0);
    inset(h, boss_c + Vec3::Y * 5.0);
    extrude(h, boss_c + Vec3::Y * 5.0, 3.0);
    // 6. the pocket: inset, then 4 mm in
    pick(h, &pocket);
    inset(h, pocket_c);
    extrude(h, pocket_c, -4.0);
    // 7. the island: out 4 mm
    pick(h, &island);
    extrude(h, island_c, 4.0);
    h.st.poly_sel = None;
    park(h, 6);
    h.shot("lsurfx-05-details");
    // 8. an edge loop (Ctrl + R) along row 1: over the boss, into the pocket
    top(h);
    h.key(Key::Num2);
    h.hold(Modifiers::CTRL);
    h.key(Key::R);
    h.hold(Modifiers::NONE);
    let la = h.screen_of(loop_at);
    h.move_to(la);
    h.steps(3);
    view(h);
    h.move_to(h.screen_of(loop_at));
    h.steps(3);
    h.shot("lsurfx-06-loop-preview");
    let faces_before = h.st.doc.feature(id).unwrap().mesh.as_ref().unwrap().faces.len();
    h.click(h.screen_of(loop_at));
    h.steps(3);
    h.key(Key::Q);
    h.st.tool = Tool::Select;
    let faces_after = h.st.doc.feature(id).unwrap().mesh.as_ref().unwrap().faces.len();
    eprintln!("lsurfx: loop: {faces_before} -> {faces_after} faces");
    park(h, 6);
    h.shot("lsurfx-07-loop");
    h.st.poly_sel = None;
    h.key(Key::Num4);
    h.st.selection.clear();
    park(h, 6);
    h.shot("lsurfx-08-poly-done");
    // 9. NURBS: the layer bound again, its details riding on the surface
    h.st.set_patch_state(id, PatchState::Nurbs);
    h.key(Key::Num4);
    h.st.selection = vec![id];
    park(h, 14);
    h.shot("lsurfx-09-nurbs");
    {
        let f = h.st.doc.feature(id).unwrap();
        let m = f.mesh.as_ref().unwrap();
        let p = m.patch.as_ref().unwrap();
        eprintln!("lsurfx: layer {} points, {} faces, {} off the surface, bound off {:.4}", m.verts.len(), m.faces.len(), p.off_surface().iter().filter(|o| **o).count(), nurbs_app::linked::bound_off(f));
    }
    // 10. the contour pulled far: a front corner up 24, the opposite back
    //     corner down 14, a back tangent out 18 in plan (scripted), the
    //     front's middle up again
    h.key(Key::Num1);
    h.key(Key::W);
    h.st.selection = vec![id];
    h.steps(2);
    let c = cv(h);
    let corners = [0usize, 3, 12, 15];
    // the front-left corner (the front-right one sits under the help box)
    let fr = corners.into_iter().max_by(|a, b| (c[*a].z - c[*a].x).total_cmp(&(c[*b].z - c[*b].x))).unwrap();
    let bl = corners.into_iter().min_by(|a, b| (c[*a].z + c[*a].x).total_cmp(&(c[*b].z + c[*b].x))).unwrap();
    lift(h, fr, 22.0);
    lift(h, bl, -14.0);
    h.st.move_patch_points(id, &[back[1]], Vec3::new(0.0, 0.0, -18.0));
    lift(h, front[0], 8.0);
    park(h, 4);
    h.shot("lsurfx-10-contour");
    h.key(Key::Num4);
    h.st.selection = vec![id];
    park(h, 14);
    h.shot("lsurfx-11-follows");
    h.st.selection.clear();
    park(h, 10);
    h.shot("lsurfx-12-follows-plain");
    // 11. carved on the surface: a slot across the rib (Cube), a hole through
    //     the island, one through the pocket's floor (Cylinders)
    let surface_at = |h: &Harness, p: Vec3| {
        // the layer's point under p seen from above
        let f = h.st.doc.feature(id).unwrap();
        let pts = nurbs_app::meshes::world_points(f);
        let near = pts.iter().copied().min_by(|a, b| (a.x - p.x).hypot(a.z - p.z).total_cmp(&(b.x - p.x).hypot(b.z - p.z))).unwrap();
        Vec3::new(p.x, near.y, p.z)
    };
    h.st.selection.clear();
    h.st.mode = Op::Carve;
    let make = |h: &mut Harness, kind: PrimKind, a3: Vec3, b3: Vec3, up: f32| {
        h.st.tool = Tool::Create(kind);
        let a = h.screen_of(a3);
        let b = h.screen_of(b3);
        h.move_to(a);
        h.steps(2);
        h.drag(a, b, 10, true);
        let t = h.screen_of(b3 + Vec3::Y * up);
        h.move_to(t);
        h.settle();
        h.click(t);
        h.settle();
        h.st.tool = Tool::Select;
        let f = h.st.doc.features.last().unwrap();
        eprintln!("lsurfx: {:?} {:?} {:?} at {:?}", f.name, f.op, f.prim, f.origin);
    };
    // the rib's middle, now where the surface took it
    let rib_now = {
        let f = h.st.doc.feature(id).unwrap();
        let m = f.mesh.as_ref().unwrap();
        let p = m.patch.as_ref().unwrap();
        let cvm = p.cvs(nurbs_app::meshes::size(f));
        let x = nurbs_app::linked::to_world(f);
        let u = 0.5 + 0.5 / nu as f32;
        let v = (jm as f32 + 0.5) / nv as f32;
        let mid = x.transform_point3(nurbs_kernel::patch::eval(&cvm, u, v).0);
        let across = x.transform_point3(nurbs_kernel::patch::eval(&cvm, u, v + 2.2 / nv as f32).0);
        (mid, across)
    };
    let on_now = |h: &Harness, u: f32, v: f32| {
        let f = h.st.doc.feature(id).unwrap();
        let p = f.mesh.as_ref().unwrap().patch.as_ref().unwrap();
        let cvm = p.cvs(nurbs_app::meshes::size(f));
        nurbs_app::linked::to_world(f).transform_point3(nurbs_kernel::patch::eval(&cvm, u, v).0)
    };
    let dir = (rib_now.1 - rib_now.0).normalize();
    let a3 = surface_at(h, rib_now.0 - dir * 16.0 - Vec3::new(5.0, 0.0, 0.0));
    let b3 = Vec3::new(a3.x + 10.0, a3.y, a3.z + 32.0 * dir.z.signum());
    make(h, PrimKind::Box, a3, b3, 28.0);
    let island_now = on_now(h, island_uv[0], island_uv[1]);
    make(h, PrimKind::Cylinder, island_now, island_now + Vec3::new(4.5, 0.0, 0.0), 26.0);
    let pocket_now = on_now(h, pocket_uv[0], pocket_uv[1]);
    make(h, PrimKind::Cylinder, pocket_now, pocket_now + Vec3::new(3.0, 0.0, 0.0), 26.0);
    h.st.mode = Op::Add;
    h.st.selection.clear();
    park(h, 14);
    h.shot("lsurfx-13-carved-nurbs");
    h.st.set_patch_state(id, PatchState::Poly);
    h.st.selection.clear();
    park(h, 14);
    h.shot("lsurfx-14-carved-poly");
    // 12. close-ups, nothing selected, both states
    let close = |h: &mut Harness, at: Vec3, yaw: f32, pitch: f32, dist: f32| {
        h.st.camera.yaw = yaw;
        h.st.camera.pitch = pitch;
        h.st.camera.target = at;
        h.st.camera.dist = dist;
        h.steps(10);
    };
    let rib_close = surface_at(h, rib_now.0);
    let boss_now = on_now(h, boss_uv[0], boss_uv[1]);
    for (name, at, yaw, pitch, dist) in [("rib", rib_close, 0.7f32, 0.55f32, 90.0f32), ("boss", boss_now, -0.6, 0.6, 70.0), ("pocket", pocket_now, 0.4, 0.85, 60.0), ("island", island_now, 2.6, 0.6, 70.0)] {
        h.st.set_patch_state(id, PatchState::Poly);
        h.st.selection.clear();
        close(h, at, yaw, pitch, dist);
        h.shot(&format!("lsurfx-15-{name}-poly"));
        h.st.set_patch_state(id, PatchState::Nurbs);
        h.st.selection.clear();
        h.steps(8);
        h.shot(&format!("lsurfx-16-{name}-nurbs"));
    }
    // 13. from below and from the back side
    close(h, Vec3::new(0.0, 4.0, 0.0), 3.6, -0.35, 240.0);
    h.shot("lsurfx-17-below-nurbs");
    close(h, Vec3::new(0.0, 4.0, 0.0), 3.4, 0.5, 250.0);
    h.shot("lsurfx-18-back-nurbs");
    h.st.set_patch_state(id, PatchState::Poly);
    h.st.selection.clear();
    h.steps(8);
    h.shot("lsurfx-18-back-poly");
    eprintln!("lsurfx: undo {:?}", h.st.undo_labels().iter().rev().take(12).collect::<Vec<_>>());
}

/// The Studio interface (Settings › Interface, the mock-up D3 of 22 nov.):
/// on the iPad, the scene on the left, the inspector on the right, the
/// dock at the bottom; a shape picked by the Pencil (its sizes on the
/// part, the action pill), two edges and their Round, the Soft (SDF) tab,
/// the light theme, then the PC.
fn ipad_studio(h: &mut Harness) {
    use egui::{Id, LayerId, Order};
    use nurbs_app::touch::Phase;
    let (w0, p0) = (h.size, h.ppp);
    h.size = [1366.0, 1024.0];
    h.ppp = 2.0;
    fn tap(h: &mut Harness, p: Pos2, pencil: bool) {
        h.touch(31, Phase::Start, p, pencil);
        h.touch(31, Phase::End, p, pencil);
        h.steps(3);
    }
    fn widgets(h: &Harness, area: &str, order: Order) -> Vec<Rect> {
        let layer = LayerId::new(order, Id::new(area));
        h.ctx.viewport(|v| {
            let w = &v.this_pass.widgets;
            let mut out: Vec<Rect> = w.get_layer(layer).filter(|w| w.sense.senses_click() && w.enabled).map(|w| w.rect).collect();
            if out.is_empty() {
                out = v.prev_pass.widgets.get_layer(layer).filter(|w| w.sense.senses_click() && w.enabled).map(|w| w.rect).collect();
            }
            out
        })
    }
    let mut d = test_part();
    let boss = d.features.iter().find(|f| f.name == "Boss").unwrap().id;
    let tag = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
    d.add_round(1.5, false, vec![EdgeRef { faces: [tag(boss, 0), tag(boss, 1)], at: DVec3::new(17.0, 30.0, 0.0) }]);
    h.reset(d, "Test part");
    h.st.touch.on = true;
    h.st.manual_only = true;
    h.st.settings.studio = true;
    h.steps(3);
    h.st.frame_all();
    h.settle();
    h.shot("ipad-studio/studio-01-layout");
    // the Pencil taps the block: the inspector, its sizes on the part, the pill
    let block_at = h.screen_of(Vec3::new(14.0, 6.0, 15.0));
    tap(h, block_at, true);
    h.settle();
    eprintln!("studio: Pencil tap on the block, selection {:?}", h.st.selection);
    h.steps(4);
    h.shot("ipad-studio/studio-02-shape");
    // a + on the inspector's first size (40 × 40 steps)
    let plus: Vec<Rect> = widgets(h, "s-insp", Order::Middle).into_iter().filter(|r| (r.width() - 40.0).abs() < 0.6 && (r.height() - 40.0).abs() < 0.6).collect();
    eprintln!("studio: {} − / + buttons in the inspector", plus.len());
    if plus.len() >= 2 {
        let before = h.st.doc.feature(h.st.selection.first().copied().unwrap_or(boss)).map(|f| f.prim.clone());
        tap(h, plus[1].center(), true);
        h.settle();
        eprintln!("studio: + on Width: {:?} -> {:?}", before, h.st.selection.first().and_then(|id| h.st.doc.feature(*id)).map(|f| f.prim.clone()));
    }
    // the dock: its buttons (66 × 72)
    let dock: Vec<Rect> = widgets(h, "s-dock", Order::Middle).into_iter().filter(|r| (r.width() - 66.0).abs() < 0.6 && (r.height() - 72.0).abs() < 0.6).collect();
    eprintln!("studio: {} dock buttons", dock.len());
    // two edges picked by the Pencil (Edge mode), their Round
    h.st.selection.clear();
    h.st.set_sel_mode(SelMode::Edge);
    h.steps(2);
    for p in [Vec3::new(14.0, 20.0, 15.0), Vec3::new(20.0, 20.0, 8.0)] {
        let at = h.screen_of(p);
        h.hold(Modifiers::SHIFT);
        tap(h, at, true);
        h.hold(Modifiers::NONE);
    }
    eprintln!("studio: mode {:?}, {} edges picked", h.st.sel_mode, h.st.edge_sel.len());
    h.steps(4);
    h.shot("ipad-studio/studio-03-edges");
    // the pill's Round: the fillet, its size in the inspector
    let pill: Vec<Rect> = widgets(h, "s-pill", Order::Middle).into_iter().filter(|r| (r.height() - 44.0).abs() < 0.6).collect();
    eprintln!("studio: {} buttons in the pill", pill.len());
    if let Some(r) = pill.first().copied() {
        tap(h, r.center(), true);
        h.settle();
    }
    eprintln!("studio: after Round, {} features, selection {:?}", h.st.doc.features.len(), h.st.selection);
    h.steps(4);
    h.shot("ipad-studio/studio-04-round");
    // the dock's fly-out (Shapes)
    if let Some(r) = h.ctx.data(|d| d.get_temp::<Rect>(Id::new("s-shapes-btn"))) {
        h.st.selection.clear();
        h.st.edge_sel.clear();
        h.st.set_sel_mode(SelMode::Object);
        tap(h, r.center(), true);
        h.steps(8);
        h.shot("ipad-studio/studio-05-shapes");
        tap(h, r.center(), true);
        h.steps(2);
    }
    // the Soft (SDF) tab: a rounded box, a ball blended on it, a tube carved
    h.reset(Doc::default(), "Soft part");
    h.st.touch.on = true;
    h.st.manual_only = true;
    h.st.settings.studio = true;
    h.steps(3);
    let tabs: Vec<Rect> = widgets(h, "s-tabs", Order::Middle).into_iter().filter(|r| (r.width() - 113.0).abs() < 0.6).collect();
    eprintln!("studio: {} tabs", tabs.len());
    if tabs.len() == 2 {
        tap(h, tabs[1].center(), true);
    }
    h.steps(3);
    eprintln!("studio: workspace {:?}", h.st.workspace);
    h.key(egui::Key::B);
    h.st.sdf_edit_node("Size", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::new(24.0, 8.0, 16.0);
            s.round = 1.5;
        }
        n.transform.pos = Vec3::new(0.0, 8.0, 0.0);
    });
    h.st.sdf.blend = 9.0;
    h.key(egui::Key::O);
    h.st.sdf_edit_node("Size", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::splat(9.0);
        }
        n.transform.pos = Vec3::new(0.0, 17.0, 0.0);
    });
    h.st.sdf.mode = forme_core::CombineMode::Carve;
    h.st.sdf.blend = 1.0;
    h.key(egui::Key::C);
    h.st.sdf_edit_node("Size", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::new(4.0, 30.0, 0.0);
        }
        n.transform.pos = Vec3::new(0.0, 10.0, 0.0);
        n.transform.rot = glam::Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    });
    h.st.sdf.mode = forme_core::CombineMode::Add;
    h.st.sdf_settle();
    h.st.frame_all();
    h.settle();
    h.steps(8);
    // the Pencil taps the ball
    let ball = h.screen_of(Vec3::new(4.0, 24.0, 4.0));
    tap(h, ball, true);
    h.settle();
    h.steps(6);
    eprintln!("studio: Soft, picked {:?}", h.st.sdf_sel);
    h.shot("ipad-studio/studio-06-soft");
    // the light theme (the mock-up D1's look), Soft then Crisp
    h.st.settings.light = true;
    h.steps(6);
    h.shot("ipad-studio/studio-07-soft-light");
    let tabs: Vec<Rect> = widgets(h, "s-tabs", Order::Middle).into_iter().filter(|r| (r.width() - 113.0).abs() < 0.6).collect();
    if tabs.len() == 2 {
        tap(h, tabs[0].center(), true);
    }
    h.reset(test_part(), "Test part");
    h.st.touch.on = true;
    h.st.manual_only = true;
    h.st.settings.studio = true;
    h.st.settings.light = true;
    h.steps(3);
    h.st.frame_all();
    h.settle();
    let block_at = h.screen_of(Vec3::new(14.0, 6.0, 15.0));
    tap(h, block_at, true);
    h.settle();
    h.steps(4);
    h.shot("ipad-studio/studio-08-crisp-light");
    // the PC: the same layout, a mouse
    h.st.settings.light = false;
    h.st.touch.on = false;
    h.size = w0;
    h.ppp = p0;
    h.steps(4);
    h.st.frame_all();
    h.settle();
    h.steps(4);
    h.shot("ipad-studio/studio-09-pc");
    h.st.settings.studio = false;
    h.st.manual_only = false;
    h.steps(2);
}

/// The mock-up D3's model in the app (Romain, 22 nov., 9 h 55: "ajouter ce
/// material vert… et aussi refaire le même modèle pour voir"): the Bean
/// lamp, an SDF object of a ball, a bean blended on at 9 mm and a hole
/// carved in the ball's top, in the seventh material (Mint); beside it the
/// Lantern (a block and 5 operations) and the Stand (a tube and 2), as in
/// the mock-up's scene list. The Studio interface on the iPad, dark, the
/// Soft tab, the bean picked by the Pencil.
fn studio_lamp(h: &mut Harness) {
    use forme_core::{CombineMode, NodeSpec, PrimKind as SdfPrim, Shape};
    use nurbs_app::touch::Phase;
    use nurbs_kernel::mesh::{MeshKind, SdfModel};
    let (w0, p0) = (h.size, h.ppp);
    h.size = [1366.0, 1024.0];
    h.ppp = 2.0;
    let mut d = Doc::default();
    // the lamp
    let ball_c = Vec3::new(0.0, 28.0, 0.0);
    let bean_u = Vec3::new(0.14, 0.0, -0.99).normalize();
    let bean_a = Vec3::new(39.0, 13.0, -9.0);
    let bean_half = 18.0;
    let lamp = d.add_mesh(MeshKind::Sdf, &[], vec![], DVec3::ZERO, DQuat::IDENTITY, "Bean lamp");
    let mut bean_node = 0;
    {
        let f = d.feature_mut(lamp).unwrap();
        f.origin = DVec3::ZERO;
        f.material = 6;
        let mut model = SdfModel::new();
        let sc = &mut model.scene;
        let root = sc.root;
        let mut ball = Shape::new(SdfPrim::Sphere);
        ball.size = Vec3::splat(28.0);
        sc.add(root, NodeSpec::shape("Ball", ball).at(ball_c));
        let mut bean = Shape::new(SdfPrim::Capsule);
        bean.size = Vec3::new(13.0, bean_half, 0.0);
        bean_node = sc.add(root, NodeSpec::shape("Bean", bean).at(bean_a + bean_u * bean_half).rot(glam::Quat::from_rotation_arc(Vec3::Y, bean_u)).mode(CombineMode::Add).blend(9.0));
        let mut hole = Shape::new(SdfPrim::Cylinder);
        hole.size = Vec3::new(10.0, 11.0, 0.0);
        // its rim rounded (Romain 22 nov., 10 h 32: "un fillet au niveau du trou")
        sc.add(root, NodeSpec::shape("Hole", hole).at(Vec3::new(3.0, 47.0, 3.0)).mode(CombineMode::Carve).blend(4.0));
        f.mesh.as_mut().unwrap().make_mut().sdf = Some(model);
    }
    // the Lantern: a block, its upright edges and its top rounded, three
    // sides sunk in (5 operations)
    let lo = DVec3::new(-250.0, 0.0, 60.0);
    let lantern = d.add(Op::Add, Prim::Box { x: 30.0, y: 46.0, z: 30.0 }, lo, DQuat::IDENTITY);
    d.feature_mut(lantern).unwrap().name = "Lantern".into();
    let tag = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
    let at = |x: f64, y: f64, z: f64| lo + DVec3::new(x, y, z);
    d.add_round(
        4.0,
        false,
        vec![
            EdgeRef { faces: [tag(lantern, 0), tag(lantern, 4)], at: at(15.0, 23.0, 15.0) },
            EdgeRef { faces: [tag(lantern, 0), tag(lantern, 5)], at: at(15.0, 23.0, -15.0) },
            EdgeRef { faces: [tag(lantern, 1), tag(lantern, 4)], at: at(-15.0, 23.0, 15.0) },
            EdgeRef { faces: [tag(lantern, 1), tag(lantern, 5)], at: at(-15.0, 23.0, -15.0) },
        ],
    );
    d.add_round(2.0, false, vec![EdgeRef { faces: [tag(lantern, 2), tag(lantern, 4)], at: at(0.0, 46.0, 15.0) }]);
    for (sub, p) in [(4u16, at(0.0, 22.0, 15.0)), (0, at(15.0, 22.0, 0.0)), (1, at(-15.0, 22.0, 0.0))] {
        d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: tag(lantern, sub), at: p }, inset: 5.0, depth: -3.0 });
    }
    // the Stand: a disc, its rim rounded and a post pulled up from its top
    let so = DVec3::new(-220.0, 0.0, 140.0);
    let stand = d.add(Op::Add, Prim::Cylinder { radius: 18.0, height: 6.0 }, so, DQuat::IDENTITY);
    d.feature_mut(stand).unwrap().name = "Stand".into();
    d.add_round(2.0, false, vec![EdgeRef { faces: [tag(stand, 0), tag(stand, 1)], at: so + DVec3::new(18.0, 6.0, 0.0) }]);
    d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: tag(stand, 1), at: so + DVec3::new(0.0, 6.0, 0.0) }, inset: 10.0, depth: 14.0 });
    h.reset(d, "Bean lamp");
    h.st.touch.on = true;
    h.st.manual_only = true;
    h.st.settings.studio = true;
    h.st.set_workspace(nurbs_app::workspace::Workspace::Sdf);
    h.st.sdf_settle();
    h.steps(3);
    h.settle();
    // the whole scene first
    h.st.frame_all();
    h.settle();
    h.steps(6);
    h.shot("ipad-studio/lamp-01-scene");
    // the mock-up's view: the lamp alone, from the front right, above, the
    // ball where the mock-up has it (its centre at 590, 470, 140 across)
    h.st.camera.target = Vec3::new(22.0, 18.0, -14.0);
    h.st.camera.yaw = 0.45;
    h.st.camera.pitch = 0.47;
    h.st.camera.dist = 400.0;
    h.steps(2);
    for _ in 0..4 {
        let p = h.screen_of(ball_c);
        let (right, up) = (h.st.camera.dir().cross(Vec3::Y).normalize(), Vec3::Y);
        let (pr, pu) = (h.screen_of(ball_c - right), h.screen_of(ball_c + up));
        let (jr, ju) = (pr - p, pu - p);
        let e = pos2(590.0, 470.0) - p;
        // e = a jr + b ju
        let det = jr.x * ju.y - jr.y * ju.x;
        let (a, b) = ((e.x * ju.y - e.y * ju.x) / det, (jr.x * e.y - jr.y * e.x) / det);
        h.st.camera.target -= -right * a + up * b;
        h.steps(1);
    }
    eprintln!("lamp: ball at {:?}, camera {:?}", h.screen_of(ball_c), h.st.camera.target);
    h.steps(6);
    h.shot("ipad-studio/lamp-02-view");
    // the Pencil taps the bean: its outline, the gizmo, the blend, the pill
    let bean_at = h.screen_of(bean_a + bean_u * (bean_half + 8.0) + Vec3::Y * 12.0);
    h.touch(31, Phase::Start, bean_at, true);
    h.touch(31, Phase::End, bean_at, true);
    h.steps(3);
    h.settle();
    h.steps(6);
    eprintln!("lamp: picked {:?} (bean {bean_node})", h.st.sdf_sel);
    h.shot("ipad-studio/lamp-03-bean");
    // the light theme
    h.st.settings.light = true;
    h.steps(6);
    h.shot("ipad-studio/lamp-04-light");
    h.st.settings.light = false;
    h.st.settings.studio = false;
    h.st.manual_only = false;
    h.st.touch.on = false;
    h.size = w0;
    h.ppp = p0;
    h.steps(2);
}

/// The iPad (12.9", 1366 × 1024 points, 2 pixels each), the layout of the
/// mock-up of 21 nov. (`ui::tablet`): the Pencil and the fingers as
/// main.rs hands them over, the islands found by their areas' widgets.
fn ipad(h: &mut Harness) {
    use egui::{Id, Key, LayerId, Order};
    use nurbs_app::touch::Phase;
    let (w0, p0) = (h.size, h.ppp);
    h.size = [1366.0, 1024.0];
    h.ppp = 2.0;
    // a tap (the Pencil or a finger), and the clickable widgets of an area
    // (in the order they were laid out)
    fn tap(h: &mut Harness, p: Pos2, pencil: bool) {
        h.touch(31, Phase::Start, p, pencil);
        h.touch(31, Phase::End, p, pencil);
        h.steps(3);
    }
    fn widgets(h: &Harness, area: &str, order: Order) -> Vec<Rect> {
        let layer = LayerId::new(order, Id::new(area));
        h.ctx.viewport(|v| {
            let w = &v.this_pass.widgets;
            let mut out: Vec<Rect> = w.get_layer(layer).filter(|w| w.sense.senses_click() && w.enabled).map(|w| w.rect).collect();
            if out.is_empty() {
                out = v.prev_pass.widgets.get_layer(layer).filter(|w| w.sense.senses_click() && w.enabled).map(|w| w.rect).collect();
            }
            out
        })
    }
    fn sized(rs: &[Rect], w: f32, hh: f32) -> Vec<Rect> {
        rs.iter().copied().filter(|r| (r.width() - w).abs() < 0.6 && (r.height() - hh).abs() < 0.6).collect()
    }
    let mut d = test_part();
    let boss = d.features.iter().find(|f| f.name == "Boss").unwrap().id;
    let tag = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
    d.add_round(1.5, false, vec![EdgeRef { faces: [tag(boss, 0), tag(boss, 1)], at: DVec3::new(17.0, 30.0, 0.0) }]);
    h.reset(d, "Test part");
    h.st.touch.on = true;
    h.st.manual_only = true;
    h.steps(3);
    // framed in the iPad's view (the reset framed it in the PC's)
    h.st.frame_all();
    h.settle();
    h.shot("ipad/ipad-01-layout");
    // the Pencil taps the boss: its action bar, its window, its row
    let boss_at = h.screen_of(Vec3::new(10.0, 30.0, 0.0));
    tap(h, boss_at, true);
    h.settle();
    eprintln!("ipad: Pencil tap on the boss, selection {:?}", h.st.selection);
    h.shot("ipad/ipad-02-shape");
    // its Radius typed on the keypad: 8.5
    let fields = sized(&widgets(h, "op-pop", Order::Foreground), 96.0, 36.0);
    eprintln!("ipad: {} value fields in the operation window", fields.len());
    if let Some(f) = fields.iter().min_by(|a, b| a.min.y.total_cmp(&b.min.y)).copied() {
        tap(h, f.center(), true);
        let kp = h.ctx.memory(|m| m.area_rect(Id::new("keypad-area")));
        eprintln!("ipad: keypad {:?}", kp.map(|r| r.min));
        if let Some(k) = kp {
            // the keys from the bottom: row 4 Cancel / Done, row 3 . 0 ⌫, …
            let key = |row: usize, col: usize| pos2(k.min.x + 12.0 + 36.0 + col as f32 * 80.0, k.max.y - 12.0 - 25.0 - (4 - row) as f32 * 58.0);
            tap(h, key(2, 2), true); // nothing yet: replaced… then 8
            tap(h, key(3, 2), true); // ⌫
            tap(h, key(0, 1), true); // 8
            tap(h, key(3, 0), true); // .
            tap(h, key(1, 1), true); // 5
            h.shot("ipad/ipad-03-keypad");
            tap(h, pos2(k.max.x - 12.0 - 60.0, k.max.y - 12.0 - 25.0), true); // Done
            h.settle();
            let r = h.st.doc.feature(boss).map(|f| match f.prim {
                Prim::Cylinder { radius, .. } => radius,
                _ => 0.0,
            });
            eprintln!("ipad: Boss radius after the keypad {r:?} (8.5 asked)");
        }
    }
    // the − / + steps: one + on the height
    let plus = h.ctx.viewport(|v| v.this_pass.widgets.get_layer(LayerId::new(Order::Foreground, Id::new("op-pop"))).filter(|w| w.sense.senses_click() && (w.rect.width() - 40.0).abs() < 0.6 && (w.rect.height() - 36.0).abs() < 0.6).map(|w| w.rect).collect::<Vec<_>>());
    if plus.len() >= 4 {
        let before = h.st.doc.feature(boss).map(|f| f.prim.clone());
        tap(h, plus[3].center(), true);
        h.settle();
        eprintln!("ipad: + on the second row: {:?} -> {:?}", before, h.st.doc.feature(boss).map(|f| f.prim.clone()));
    }
    h.shot("ipad/ipad-04-stepped");
    // its position and rotation, unfolded at the bottom of the sidebar
    if let Some(r) = sized(&widgets(h, "t-side", Order::Middle), 316.0 - 24.0, 48.0).first().copied() {
        tap(h, r.center(), true);
    }
    h.steps(3);
    h.shot("ipad/ipad-05-position");
    if let Some(r) = sized(&widgets(h, "t-side", Order::Middle), 316.0 - 24.0, 48.0).first().copied() {
        tap(h, r.center(), true);
    }
    // Edge mode from the top island, two edges picked by the Pencil
    let modes = sized(&widgets(h, "t-modes", Order::Middle), 64.0, 46.0);
    eprintln!("ipad: {} mode buttons", modes.len());
    if modes.len() == 4 {
        tap(h, modes[1].center(), true);
    }
    h.st.selection.clear();
    h.steps(2);
    for p in [Vec3::new(14.0, 20.0, 15.0), Vec3::new(20.0, 20.0, 8.0)] {
        let at = h.screen_of(p);
        h.hold(Modifiers::SHIFT);
        tap(h, at, true);
        h.hold(Modifiers::NONE);
    }
    eprintln!("ipad: mode {:?}, {} edges picked", h.st.sel_mode, h.st.edge_sel.len());
    h.steps(3);
    h.shot("ipad/ipad-06-edges");
    // the action bar's Fillet
    let acts: Vec<Rect> = widgets(h, "t-actions", Order::Middle).into_iter().filter(|r| (r.height() - 52.0).abs() < 0.6).collect();
    eprintln!("ipad: {} buttons in the action bar", acts.len());
    if let Some(r) = acts.first().copied() {
        tap(h, r.center(), true);
        h.settle();
    }
    eprintln!("ipad: after Fillet, {} features, selection {:?}", h.st.doc.features.len(), h.st.selection);
    h.steps(3);
    h.shot("ipad/ipad-07-fillet");
    // Object mode, the sidebar folded
    let modes = sized(&widgets(h, "t-modes", Order::Middle), 64.0, 46.0);
    if modes.len() == 4 {
        tap(h, modes[3].center(), true);
    }
    h.st.selection.clear();
    h.st.panel_visible = false;
    h.steps(3);
    h.shot("ipad/ipad-08-folded");
    h.st.panel_visible = true;
    // the view menu, the settings
    if let Some(r) = h.ctx.data(|d| d.get_temp::<Rect>(Id::new("t-view-btn"))) {
        tap(h, r.center(), true);
        h.steps(8);
        h.shot("ipad/ipad-09-view-menu");
        tap(h, r.center(), true);
    }
    if let Some(r) = h.ctx.data(|d| d.get_temp::<Rect>(Id::new("t-settings-btn"))) {
        tap(h, r.center(), true);
        h.steps(8);
        h.shot("ipad/ipad-10-settings");
        tap(h, r.center(), true);
    }
    // the dock's fly-outs: Shapes (the 3ds Max shapes, Surface, Slot) and
    // More (Shell, Mirror, Array, Align)
    for (btn, name) in [("t-curves-btn", "ipad/ipad-10b-shapes"), ("t-more-btn", "ipad/ipad-10c-more")] {
        if let Some(r) = h.ctx.data(|d| d.get_temp::<Rect>(Id::new(btn))) {
            tap(h, r.center(), true);
            h.steps(8);
            h.shot(name);
            tap(h, r.center(), true);
            h.steps(2);
        } else {
            eprintln!("ipad: no {btn}");
        }
    }
    // the Touch & Pencil sheet ("?")
    let help = pos2(h.size[0] - 16.0 - 30.0, h.size[1] - 16.0 - 30.0);
    tap(h, help, true);
    h.steps(8);
    h.shot("ipad/ipad-11-gestures");
    tap(h, help, true);
    h.steps(2);
    // a finger held still on the block: the marking menu
    let block = h.screen_of(Vec3::new(14.0, 6.0, 15.0));
    h.touch(7, Phase::Start, block, false);
    for _ in 0..40 {
        h.step();
    }
    h.touch(7, Phase::Move, block + vec2(4.0, -70.0), false);
    h.steps(20);
    h.shot("ipad/ipad-12-marking");
    h.touch(7, Phase::Cancel, block, false);
    h.steps(3);
    h.key(Key::Escape);
    // gestures: a double tap frames what is under it
    let c = pos2((h.st.view_rect[0] + h.st.view_rect[2]) * 0.5, h.st.view_rect[3] - 120.0);
    let d0 = h.st.camera.dist;
    h.st.camera.dist *= 2.0;
    let d1 = h.st.camera.dist;
    tap(h, c, false);
    tap(h, c, false);
    h.settle();
    eprintln!("ipad: double tap in empty space: distance {d0:.1} -> {d1:.1} -> {:.1} (framed all)", h.st.camera.dist);
    // the palm: the Pencil down, a finger dragged in the view turns nothing
    let yaw = h.st.camera.yaw;
    let pen = h.screen_of(Vec3::new(-14.0, 20.0, 5.0));
    h.touch(40, Phase::Start, pen, true);
    let palm = pos2(c.x - 150.0, c.y - 40.0);
    h.touch(41, Phase::Start, palm, false);
    for i in 1..=10 {
        h.touch(41, Phase::Move, palm + vec2(-12.0 * i as f32, 0.0), false);
    }
    h.touch(41, Phase::End, palm + vec2(-120.0, 0.0), false);
    h.touch(40, Phase::End, pen, true);
    h.steps(3);
    eprintln!("ipad: palm while the Pencil is down: yaw {yaw:.3} -> {:.3} (unchanged)", h.st.camera.yaw);
    // Pencil only: a finger tap on the boss picks nothing
    h.st.selection.clear();
    h.st.settings.pencil_only = true;
    h.steps(30);
    let boss_at = h.screen_of(Vec3::new(10.0, 30.0, 0.0));
    tap(h, boss_at, false);
    h.steps(30);
    eprintln!("ipad: Pencil only, a finger tap on the boss: selection {:?} (empty)", h.st.selection);
    h.st.settings.pencil_only = false;
    tap(h, boss_at, false);
    eprintln!("ipad: a finger tap on the boss: selection {:?}", h.st.selection);
    // two-finger tap: undo
    let before = h.st.doc.features.len();
    h.touch_drag(&[(8, c), (9, c + vec2(80.0, 0.0))], &[c, c + vec2(80.0, 0.0)], 1, true);
    h.settle();
    eprintln!("ipad: two-finger tap (undo), features {before} -> {}, selection {:?} (the tap's pick undone)", h.st.doc.features.len(), h.st.selection);
    // left-handed: the dock on the left, the scene list and Shift / Ctrl
    // on the right
    h.st.settings.left_handed = true;
    h.steps(3);
    let boss_at = h.screen_of(Vec3::new(10.0, 30.0, 0.0));
    tap(h, boss_at, true);
    h.settle();
    h.steps(8);
    h.shot("ipad/ipad-13-left-handed");
    h.st.settings.left_handed = false;
    // the SDF tab: a sphere, a box blended on it, a cylinder carved
    h.reset(Doc::default(), "SDF part");
    h.st.touch.on = true;
    h.st.manual_only = true;
    h.steps(3);
    let tabs = sized(&widgets(h, "t-view", Order::Middle), 70.0, 46.0);
    eprintln!("ipad: {} tabs", tabs.len());
    if tabs.len() == 2 {
        tap(h, tabs[1].center(), true);
    }
    h.steps(3);
    eprintln!("ipad: workspace {:?}", h.st.workspace);
    h.key(Key::B);
    h.st.sdf_edit_node("Size", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::new(24.0, 8.0, 16.0);
            s.round = 1.5;
        }
        n.transform.pos = Vec3::new(0.0, 8.0, 0.0);
    });
    h.st.sdf.blend = 3.0;
    h.key(Key::O);
    h.st.sdf_edit_node("Size", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::splat(9.0);
        }
        n.transform.pos = Vec3::new(0.0, 17.0, 0.0);
    });
    h.st.sdf.mode = forme_core::CombineMode::Carve;
    h.st.sdf.blend = 1.0;
    h.key(Key::C);
    h.st.sdf_edit_node("Size", |n| {
        if let Some(s) = n.shape_mut() {
            s.size = Vec3::new(4.0, 30.0, 0.0);
        }
        n.transform.pos = Vec3::new(0.0, 10.0, 0.0);
        n.transform.rot = glam::Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    });
    h.st.sdf_settle();
    h.st.frame_all();
    h.settle();
    h.steps(8);
    h.shot("ipad/ipad-14-sdf");
    h.size = w0;
    h.ppp = p0;
}
