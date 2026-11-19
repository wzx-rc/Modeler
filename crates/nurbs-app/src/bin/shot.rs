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
        self.ctx = egui::Context::default();
        theme::install(&self.ctx);
        self.pending_tex = Default::default();
        self.drawer = Drawer::new(&self.gpu.device, FMT);
        self.steps(2);
        self.settle();
        self.st.frame_all();
        self.steps(2);
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
            if !self.st.building() {
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args.first().cloned().unwrap_or_else(|| "shots".into());
    let only: Vec<String> = args.iter().skip(1).cloned().collect();
    let want = |name: &str| only.is_empty() || only.iter().any(|o| name.starts_with(o.as_str()));
    let mut h = Harness::new(&out);

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
        d.feature_mut(id).unwrap().profile = Some(nurbs_kernel::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.0], [0.0, 0.0], [0.0, 0.5], [-0.5, 0.5]], smooth: vec![false; 6], corner: vec![], tension: vec![], open: false });
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
        // iPad Pro 12.9" (1366 × 1024 points, 2 pixels each) with the app's
        // iPad zoom 0.85: the layout as on the device
        use nurbs_app::touch::Phase;
        let (w, h0, ppp) = (h.size, h.ppp, 0.85f32);
        h.size = [1366.0 / ppp, 1024.0 / ppp];
        h.ppp = 2.0 * ppp;
        h.reset(test_part(), "Test part");
        h.st.touch.on = true;
        h.steps(3);
        h.shot("ipad/ipad-1-layout");
        // a finger tap on the boss selects it
        let boss = h.screen_of(Vec3::new(10.0, 30.0, 0.0));
        h.touch(1, Phase::Start, boss, false);
        h.touch(1, Phase::End, boss, false);
        h.steps(3);
        h.shot("ipad/ipad-2-tap-select");
        // one finger orbits
        let c = pos2(h.size[0] * 0.6, h.size[1] * 0.75);
        h.touch_drag(&[(2, c)], &[c + vec2(-180.0, -60.0)], 12, true);
        h.settle();
        h.shot("ipad/ipad-3-orbit");
        // two fingers: pinch out (zoom in) and pan
        let (a, b) = (c + vec2(-60.0, 0.0), c + vec2(60.0, 0.0));
        h.touch_drag(&[(3, a), (4, b)], &[a + vec2(-90.0, -40.0), b + vec2(90.0, -40.0)], 12, true);
        h.settle();
        h.shot("ipad/ipad-4-pinch");
        // Shift held by a finger, Ctrl latched by a tap
        let key = |h: &Harness, k: nurbs_app::touch::BarKey| h.st.touch.keys.iter().find(|(_, x)| *x == k).map(|(r, _)| r.center()).unwrap();
        let shift = key(&h, nurbs_app::touch::BarKey::Shift);
        let ctrl = key(&h, nurbs_app::touch::BarKey::Ctrl);
        h.touch(5, Phase::Start, ctrl, false);
        h.touch(5, Phase::End, ctrl, false);
        h.touch(6, Phase::Start, shift, false);
        h.steps(2);
        h.shot("ipad/ipad-5-keys");
        h.touch(6, Phase::End, shift, false);
        h.touch(5, Phase::Start, ctrl, false);
        h.touch(5, Phase::End, ctrl, false);
        // a finger held still on the block: the marking menu, then a flick
        h.st.selection.clear();
        let block = h.screen_of(Vec3::new(14.0, 6.0, 15.0));
        h.touch(7, Phase::Start, block, false);
        for _ in 0..40 {
            h.step();
        }
        h.touch(7, Phase::Move, block + vec2(4.0, -70.0), false);
        h.steps(20);
        h.shot("ipad/ipad-6-marking");
        h.touch(7, Phase::Cancel, block + vec2(0.0, 0.0), false);
        h.steps(3);
        // the Pencil on the left panel's slider and in the view
        h.key(egui::Key::Escape);
        let fillet = pos2(120.0, 200.0);
        let _ = fillet;
        // two-finger tap: undo
        let before = h.st.doc.features.len();
        h.touch_drag(&[(8, c), (9, c + vec2(80.0, 0.0))], &[c, c + vec2(80.0, 0.0)], 1, true);
        eprintln!("ipad: two-finger tap, features {before} -> {}", h.st.doc.features.len());
        h.size = w;
        h.ppp = h0;
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
        let pivot = h.st.pivot().unwrap().as_vec3();
        let v = h.st.view();
        let k = v.pixel_size_at((pivot - v.eye).length()) * 78.0;
        let (a, b) = (h.screen_of(pivot + Vec3::Y * k), h.screen_of(pivot + Vec3::Y * k * 1.5));
        h.move_to(a);
        h.steps(2);
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
        h.st.undo();
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
