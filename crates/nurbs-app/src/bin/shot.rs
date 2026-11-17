#![allow(deprecated)]
//! forme-nurbs-shot: runs the real app headless (egui + wgpu, a software
//! GPU is fine) with the kernel process, plays scenarios with pointer
//! events and saves PNGs.
//!
//! cargo run --release -p nurbs-app --bin forme-nurbs-shot -- OUT_DIR [scenario prefix…]
//! (FORME_NURBS_KERNEL=path/to/forme-nurbs-kernel when it is not next to it)

use egui::{pos2, Event, Modifiers, PointerButton, Pos2, RawInput, Rect};
use glam::{DQuat, DVec3, Vec3};
use nurbs_app::draw::Drawer;
use nurbs_app::state::{AppState, SelMode, Tool};
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
        raw
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
