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
        // the camera's flight ended (a scenario's own camera set next is
        // not taken back by it)
        self.settle();
    }

    fn raw(&mut self) -> RawInput {
        let mut raw = RawInput {
            // the window in points: smaller as the interface's size grows
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(self.size[0], self.size[1]) / self.ctx.zoom_factor())),
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
        // the Settings' Interface size, as main.rs applies it
        let want = self.st.settings.ui_scale.clamp(0.35, 1.75);
        if (self.ctx.zoom_factor() - want).abs() > 1e-4 {
            self.ctx.set_zoom_factor(want);
        }
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

    /// A key as winit gives it to egui on another layout: its physical
    /// place (`phys`), the logical key egui made of it (the physical one
    /// when the character has no egui key: AZERTY's & é "), the text typed.
    fn key_phys(&mut self, key: egui::Key, phys: egui::Key, text: &str) {
        self.pending.push(Event::Key { key, physical_key: Some(phys), pressed: true, repeat: false, modifiers: self.mods });
        self.pending.push(Event::Text(text.to_string()));
        self.step();
        self.pending.push(Event::Key { key, physical_key: Some(phys), pressed: false, repeat: false, modifiers: self.mods });
        self.step();
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
        self.drawer.render(&self.gpu.device, &self.gpu.queue, &view, [w, h], out.pixels_per_point, &self.st, &paint, &mut tex);
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
        self.drawer.render(&self.gpu.device, &self.gpu.queue, &view, [w, h], out.pixels_per_point, &self.st, &paint, &mut tex);
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

/// A shape added and named (the `object` scenario).
fn named(d: &mut Doc, op: Op, prim: Prim, origin: DVec3, rot: DQuat, name: &str) -> u32 {
    let id = d.add(op, prim, origin, rot);
    d.feature_mut(id).unwrap().name = name.into();
    id
}

/// The model's edges between a face of `a` and a face of `b` (feature,
/// sub), as a click would name them (`object`).
fn edges_between(h: &Harness, a: (u32, u16), b: (u32, u16)) -> Vec<EdgeRef> {
    let Some(m) = h.st.model.as_ref() else { return vec![] };
    let tag = |i: i32| m.faces.get(i as usize).and_then(|f| f.tag);
    let (ta, tb) = (FaceTag { feature: a.0, sub: a.1 }, FaceTag { feature: b.0, sub: b.1 });
    let mut out = vec![];
    for e in &m.edges {
        let (x, y) = (tag(e.faces[0]), tag(e.faces[1]));
        let hit = (x == Some(ta) && y == Some(tb)) || (x == Some(tb) && y == Some(ta));
        if hit && !e.pts.is_empty() {
            let p = e.pts[e.pts.len() / 2];
            out.push(EdgeRef { faces: [ta.pack(), tb.pack()], at: DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64) });
        }
    }
    out
}

/// One modelling step as the app makes it (one edit, one undo step),
/// timed until the fine model is shown (`object`).
fn timed<T: Default>(h: &mut Harness, log: &mut Vec<String>, label: &str, f: impl FnOnce(&mut Doc) -> T) -> T {
    let mut r = T::default();
    let t0 = Instant::now();
    h.st.edit(label, |d| r = f(d));
    h.settle();
    let wall = t0.elapsed().as_secs_f64() * 1e3;
    let m = h.st.model.as_ref();
    let errs = m.map(|m| m.errors.clone()).unwrap_or_default();
    let line = format!("{:<34} {:>6.0} ms  kernel {:>6.0} ms  {:>3} features  {:>4} faces  {:>6} triangles  {} errors {}", label, wall, m.map_or(0.0, |m| m.ms), h.st.doc.features.len(), m.map_or(0, |m| m.faces.len()), m.map_or(0, |m| m.triangles.len()), errs.len(), if errs.is_empty() { String::new() } else { format!("{errs:?}") });
    eprintln!("  {line}");
    log.push(line);
    r
}

/// A Library sample made on a plate (60 × 10 × 60, its top at y = 10),
/// then saved from its shapes (`library`).
fn make_sample(h: &mut Harness, name: &str, f: impl FnOnce(&mut Harness, u32) -> Vec<u32>) {
    let mut log = vec![];
    h.reset(Doc::default(), name);
    let plate = timed(h, &mut log, "Plate", |d| named(d, Op::Add, Prim::Box { x: 60.0, y: 10.0, z: 60.0 }, DVec3::ZERO, DQuat::IDENTITY, "Plate"));
    let ids = f(h, plate);
    h.st.sel_mode = SelMode::Object;
    h.st.selection = ids;
    h.settle();
    let errs = h.st.model.as_ref().map(|m| m.errors.clone()).unwrap_or_default();
    let i = h.st.save_sample(Some(name));
    let s = i.and_then(|i| h.st.library.samples.get(i));
    eprintln!("  sample {name}: {:?} features, {} triangles, {} edges, errors {errs:?}", s.map(|s| s.counts()), s.map_or(0, |s| s.tris.len()), s.map_or(0, |s| s.edges.len()));
}

/// `library` (Romain 24 nov., 8 h 21: "un nouveau composant la Library…
/// sauvegarder de petits samples et ensuite les placer en interactif sur
/// les surfaces comme des boulons"): four samples made on a plate and
/// saved (a socket bolt, a domed rivet, a vent whose rim fillet names the
/// plate, a boss with its foot fillet and its hole), the Library island,
/// then each placed with clicks on a part's faces: on a hole's centre, on
/// a slanted face, on a side, on a round face, turned with ].
fn library(h: &mut Harness, out: &str) {
    let docs = format!("{out}/docs");
    let _ = std::fs::remove_dir_all(format!("{docs}/library"));
    std::env::set_var("FORME_DOCS", &docs);
    let up = DQuat::IDENTITY;
    let mut log: Vec<String> = vec![];
    make_sample(h, "Socket bolt", |h, plate| {
        let mut log = vec![];
        let head = timed(h, &mut log, "Head", |d| named(d, Op::Add, Prim::Cylinder { radius: 4.0, height: 3.0 }, DVec3::new(0.0, 10.0, 0.0), up, "Head"));
        let hex: Vec<[f64; 2]> = (0..6).map(|k| { let a = std::f64::consts::TAU * k as f64 / 6.0; [0.5 * a.cos(), 0.5 * a.sin() / 0.866] }).collect();
        let sock = timed(h, &mut log, "Socket", |d| {
            let id = named(d, Op::Carve, Prim::Profile { x: 3.4, y: 1.8, z: 2.94 }, DVec3::new(0.0, 11.3, 0.0), up, "Socket");
            d.feature_mut(id).unwrap().profile = Some(nurbs_kernel::doc::Profile { points: hex, smooth: vec![false; 6], corner: vec![0.0; 6], tension: vec![], open: false, spline: false });
            id
        });
        let e = edges_between(h, (head, 0), (head, 1));
        timed(h, &mut log, "Head fillet 0.6", |d| d.add_round(0.6, false, e));
        let _ = plate;
        vec![head, sock]
    });
    make_sample(h, "Rivet", |h, _| {
        let mut log = vec![];
        let r = timed(h, &mut log, "Rivet", |d| named(d, Op::Add, Prim::Cylinder { radius: 2.5, height: 1.4 }, DVec3::new(0.0, 10.0, 0.0), up, "Rivet"));
        let e = edges_between(h, (r, 0), (r, 1));
        timed(h, &mut log, "Dome", |d| d.add_round(1.2, false, e));
        vec![r]
    });
    make_sample(h, "Vent", |h, plate| {
        let mut log = vec![];
        let v = timed(h, &mut log, "Vent", |d| {
            let id = named(d, Op::Carve, Prim::Profile { x: 14.0, y: 1.6, z: 4.0 }, DVec3::new(0.0, 8.5, 0.0), up, "Vent");
            d.feature_mut(id).unwrap().profile = Some(nurbs_kernel::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![2.0; 4], tension: vec![], open: false, spline: false });
            id
        });
        let mut e = vec![];
        for sub in 3..20 {
            e.extend(edges_between(h, (v, sub), (plate, 2)));
        }
        eprintln!("  vent rim edges: {}", e.len());
        timed(h, &mut log, "Vent rim 0.4", |d| d.add_round(0.4, false, e));
        vec![v]
    });
    make_sample(h, "Boss", |h, plate| {
        let mut log = vec![];
        let b = timed(h, &mut log, "Boss", |d| named(d, Op::Add, Prim::Cylinder { radius: 5.0, height: 4.0 }, DVec3::new(0.0, 10.0, 0.0), up, "Boss"));
        let hole = timed(h, &mut log, "Hole", |d| named(d, Op::Carve, Prim::Cylinder { radius: 2.0, height: 5.0 }, DVec3::new(0.0, 9.1, 0.0), up, "Hole"));
        let e = edges_between(h, (b, 0), (plate, 2));
        timed(h, &mut log, "Foot fillet 1", |d| d.add_round(1.0, false, e));
        let e = edges_between(h, (b, 0), (b, 1));
        timed(h, &mut log, "Top chamfer 0.5", |d| d.add_round(0.5, true, e));
        vec![b, hole]
    });
    let _ = &mut log;
    h.shot("zero-library-1-boss-sample");
    // the samples written: those of the app are these (assets/library)
    let lib = format!("{docs}/library");
    let names: Vec<String> = std::fs::read_dir(&lib).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    eprintln!("  library files: {names:?}");
    // the same four come with the app: the copies saved here put aside
    // (FORME_KEEP_LIBRARY=1 keeps them, to copy into assets/library)
    if std::env::var("FORME_KEEP_LIBRARY").is_err() {
        let _ = std::fs::remove_dir_all(&lib);
    }
    // a part to put them on: a plate with a hole, a slanted block, a pipe
    let mut d = Doc::default();
    let plate = named(&mut d, Op::Add, Prim::Box { x: 120.0, y: 8.0, z: 70.0 }, DVec3::ZERO, up, "Plate");
    let hole = named(&mut d, Op::Carve, Prim::Cylinder { radius: 3.0, height: 12.0 }, DVec3::new(-35.0, -2.0, 18.0), up, "Hole");
    let wedge = named(&mut d, Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 40.0 }, DVec3::new(30.0, 8.0, 0.0), DQuat::from_rotation_z(0.35), "Wedge");
    let pipe = named(&mut d, Op::Add, Prim::Cylinder { radius: 10.0, height: 60.0 }, DVec3::new(-55.0, 18.0, -22.0), DQuat::from_rotation_z(-std::f64::consts::FRAC_PI_2), "Pipe");
    let _ = (plate, hole, wedge, pipe);
    h.reset(d, "Library");
    h.st.library.load();
    h.st.library.open = true;
    h.st.camera.yaw = 0.6;
    h.st.camera.pitch = 0.55;
    h.st.frame_all();
    h.settle();
    let idx = |h: &Harness, n: &str| h.st.library.samples.iter().position(|s| s.name == n && !s.builtin()).or_else(|| h.st.library.samples.iter().position(|s| s.name == n)).unwrap_or(0);
    eprintln!("  samples: {:?}", h.st.library.samples.iter().map(|s| (s.name.clone(), s.builtin(), s.counts())).collect::<Vec<_>>());
    h.shot("zero-library-2-island");
    // the bolt on the hole's centre (it catches), then turned and put on the plate
    let i = idx(h, "Socket bolt");
    h.st.arm_place(i);
    let at = h.screen_of(Vec3::new(-35.0, 8.0, 18.0) + Vec3::new(1.2, 0.0, 0.8));
    h.move_to(at);
    h.steps(3);
    eprintln!("  spot on the hole: {:?}", h.st.place.as_ref().and_then(|p| p.spot).map(|s| (s.p, s.snapped)));
    h.shot("zero-library-3-preview-on-hole");
    h.click(at);
    h.settle();
    for x in [-50.0, -20.0] {
        let at = h.screen_of(Vec3::new(x, 8.0, 28.0));
        h.move_to(at);
        h.steps(2);
        h.key(egui::Key::CloseBracket);
        h.key(egui::Key::CloseBracket);
        h.click(at);
        h.settle();
    }
    // the boss on the slanted top: its normal is the boss's up
    h.st.arm_place(idx(h, "Boss"));
    let top = Vec3::new(30.0, 8.0, 0.0) + (glam::Quat::from_rotation_z(0.35) * Vec3::new(0.0, 20.0, 0.0));
    let at = h.screen_of(top + Vec3::new(0.0, 0.0, -8.0));
    h.move_to(at);
    h.steps(3);
    h.shot("zero-library-4-preview-slanted");
    h.click(at);
    h.settle();
    // vents in a row on the plate, rivets along the pipe
    h.st.arm_place(idx(h, "Vent"));
    for z in [-8.0, 2.0, 12.0] {
        let at = h.screen_of(Vec3::new(-10.0, 8.0, z + 10.0));
        h.move_to(at);
        h.steps(2);
        h.click(at);
        h.settle();
    }
    h.st.arm_place(idx(h, "Rivet"));
    for x in [-48.0, -36.0, -24.0, -12.0] {
        let at = h.screen_of(Vec3::new(x, 18.0 + 10.0 * 0.6, -22.0 + 10.0 * 0.8));
        h.move_to(at);
        h.steps(2);
        h.click(at);
        h.settle();
    }
    h.st.select_tool();
    h.settle();
    let m = h.st.model.as_ref();
    eprintln!("  placed: {} features, {} objects, errors {:?}", h.st.doc.features.len(), h.st.doc.objects.len(), m.map(|m| m.errors.clone()));
    eprintln!("  objects: {:?}", h.st.doc.objects.iter().map(|o| (o.name.clone(), o.members.len())).collect::<Vec<_>>());
    if let Some(m) = h.st.model.as_ref() {
        for f in h.st.doc.features.iter().filter(|f| f.round.is_some()) {
            for e in &f.round.as_ref().unwrap().edges {
                let (a, b) = (FaceTag::unpack(e.faces[0]), FaceTag::unpack(e.faces[1]));
                let n = m.edges.iter().filter(|me| { let t = |i: i32| m.faces.get(i as usize).and_then(|x| x.tag); let (x, y) = (t(me.faces[0]), t(me.faces[1])); (x == a && y == b) || (x == b && y == a) }).count();
                eprintln!("  {} edge {:?}/{:?} at {:?}: {n} in the model", f.name, a, b, e.at);
            }
        }
    }
    h.shot("zero-library-5-placed");
    h.st.camera.yaw = -0.4;
    h.st.camera.pitch = 0.35;
    h.st.camera.dist *= 0.6;
    h.settle();
    h.shot("zero-library-6-close");
    // a placed boss selected: its operations with it, as any shape
    let boss = h.st.doc.features.iter().rev().find(|f| f.name.starts_with("Boss")).map(|f| f.id);
    if let Some(b) = boss {
        h.st.selection = vec![b];
        h.settle();
        h.shot("zero-library-7-boss-selected");
    }
}

/// `cable` (Romain 24 nov., 8 h 21: "un premier système de Câble"): two
/// blocks, each with a connector boss; the Cable tool (H) clicks on the
/// first boss's centre, a point on the ground between them, the second
/// boss's centre, then Enter; its slack and diameter changed in its window.
fn cable(h: &mut Harness) {
    let up = DQuat::IDENTITY;
    let mut d = Doc::default();
    named(&mut d, Op::Add, Prim::Box { x: 50.0, y: 30.0, z: 40.0 }, DVec3::new(-50.0, 0.0, 0.0), up, "Box A");
    named(&mut d, Op::Add, Prim::Cylinder { radius: 5.0, height: 4.0 }, DVec3::new(-40.0, 30.0, 0.0), up, "Plug A");
    named(&mut d, Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 40.0 }, DVec3::new(60.0, 0.0, 10.0), up, "Box B");
    named(&mut d, Op::Add, Prim::Cylinder { radius: 5.0, height: 4.0 }, DVec3::new(60.0, 10.0, 30.0), DQuat::from_rotation_x(std::f64::consts::FRAC_PI_2), "Plug B");
    h.reset(d, "Cable");
    h.st.camera.yaw = 0.35;
    h.st.camera.pitch = 0.45;
    h.st.frame_all();
    h.settle();
    h.key(egui::Key::H);
    eprintln!("  tool {:?}", h.st.tool);
    let pts = [Vec3::new(-40.0, 34.0, 0.0), Vec3::new(0.0, 0.0, 35.0), Vec3::new(60.0, 10.0, 34.0)];
    for (k, w) in pts.iter().enumerate() {
        let at = h.screen_of(*w);
        h.move_to(at);
        h.steps(2);
        h.click(at);
        h.steps(30);
        eprintln!("  point {k}: {:?}", h.st.cable_draw.as_ref().and_then(|c| c.last().copied()));
    }
    let hover = h.screen_of(Vec3::new(25.0, 0.0, 45.0));
    h.move_to(hover);
    h.steps(2);
    h.shot("zero-cable-1-drawing");
    h.move_to(h.screen_of(Vec3::new(60.0, 10.0, 34.0)));
    h.steps(2);
    h.key(egui::Key::Enter);
    h.settle();
    let id = h.st.selection.first().copied();
    let m = h.st.model.as_ref();
    eprintln!("  cable {:?}: objects {:?}, errors {:?}", id, h.st.doc.objects.iter().map(|o| (o.name.clone(), o.members.clone())).collect::<Vec<_>>(), m.map(|m| m.errors.clone()));
    h.st.select_tool();
    h.st.selection = id.into_iter().collect();
    h.settle();
    h.shot("zero-cable-2-made");
    if let Some(id) = id {
        h.st.set_cable(id, None, Some(0.15), false);
        h.settle();
        h.shot("zero-cable-3-slack");
        h.st.set_cable(id, Some(3.0), None, false);
        h.settle();
        eprintln!("  thicker: errors {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
        h.st.camera.yaw = 1.2;
        h.st.camera.pitch = 0.25;
        h.settle();
        h.shot("zero-cable-4-thicker");
    }
}

/// `gear` (Romain 24 nov., 7 h 45: "faire un engrenage avec le array
/// radial"): a spur gear made as a user would, step by step: the disc, a
/// boss, one tooth (a Sketch with its tip rounded, its top edges
/// chamfered), the tooth 24 times round the axis (Array Circle), six
/// lightening holes (one hole, its rim rounded, an Array Circle), the
/// bore and its keyway, the bore's mouth chamfered. Then the count of
/// teeth changed (30), the tooth made wider: every copy follows.
fn gear(h: &mut Harness) {
    use nurbs_kernel::{Array, ArrayKind};
    let mut log: Vec<String> = vec![];
    h.reset(Doc::default(), "Gear");
    let up = DQuat::IDENTITY;
    let circle = |sources: Vec<u32>, count: u32| Array { sources, kind: ArrayKind::Circle, count, step: DVec3::ZERO, count2: 1, step2: DVec3::ZERO, axis: 1, centre: DVec3::ZERO, angle: 360.0 };
    let disc = timed(h, &mut log, "Disc (cylinder r 30)", |d| named(d, Op::Add, Prim::Cylinder { radius: 30.0, height: 8.0 }, DVec3::ZERO, up, "Disc"));
    let boss = timed(h, &mut log, "Boss (cylinder r 11)", |d| named(d, Op::Add, Prim::Cylinder { radius: 11.0, height: 14.0 }, DVec3::new(0.0, -3.0, 0.0), up, "Boss"));
    let e = edges_between(h, (boss, 0), (disc, 1));
    timed(h, &mut log, "Fillet boss foot 1.5", |d| d.add_round(1.5, false, e));
    // one tooth: a trapezoid from inside the disc's rim (r 29) out to r 37,
    // 6 mm wide at its root, 3 at its tip, the tip's corners rounded
    let tooth = timed(h, &mut log, "Tooth (sketch)", |d| {
        let id = named(d, Op::Add, Prim::Profile { x: 8.0, y: 8.0, z: 6.0 }, DVec3::new(33.0, 0.0, 0.0), up, "Tooth");
        d.feature_mut(id).unwrap().profile = Some(nurbs_kernel::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.25], [0.5, 0.25], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![0.0, 0.8, 0.8, 0.0], tension: vec![], open: false, spline: false });
        id
    });
    let mut e = vec![];
    // its top and bottom are flush with the disc's: one face with them
    for sub in 3..14 {
        for (f, s) in [(tooth, 1), (tooth, 2), (disc, 1), (disc, 2)] {
            e.extend(edges_between(h, (f, s), (tooth, sub)));
        }
    }
    eprintln!("  tooth's top and bottom edges: {}", e.len());
    timed(h, &mut log, "Chamfer tooth edges 0.5", |d| d.add_round(0.5, true, e));
    let teeth = timed(h, &mut log, "Array circle x24 (tooth)", |d| d.add_array(circle(vec![tooth], 24)));
    let hole = timed(h, &mut log, "Lightening hole (sub r 5)", |d| named(d, Op::Carve, Prim::Cylinder { radius: 5.0, height: 12.0 }, DVec3::new(19.0, -2.0, 0.0), up, "Hole"));
    let mut e = edges_between(h, (hole, 0), (disc, 1));
    e.extend(edges_between(h, (hole, 0), (disc, 2)));
    timed(h, &mut log, "Fillet hole rims 0.8", |d| d.add_round(0.8, false, e));
    timed(h, &mut log, "Array circle x6 (hole)", |d| d.add_array(circle(vec![hole], 6)));
    let bore = timed(h, &mut log, "Bore (sub r 5)", |d| named(d, Op::Carve, Prim::Cylinder { radius: 5.0, height: 20.0 }, DVec3::new(0.0, -5.0, 0.0), up, "Bore"));
    timed(h, &mut log, "Keyway (sub box)", |d| named(d, Op::Carve, Prim::Box { x: 2.0, y: 20.0, z: 3.0 }, DVec3::new(5.5, -5.0, 0.0), up, "Keyway"));
    let e = edges_between(h, (bore, 0), (boss, 1));
    timed(h, &mut log, "Chamfer bore mouth 0.6", |d| d.add_round(0.6, true, e));
    h.st.frame_all();
    h.settle();
    h.shot("zero-gear-1");
    let (yaw, pitch) = (h.st.camera.yaw, h.st.camera.pitch);
    h.st.camera.pitch = 1.35;
    h.st.frame_all();
    h.settle();
    h.shot("zero-gear-2-top");
    h.st.camera.yaw = yaw + 0.5;
    h.st.camera.pitch = 0.35;
    h.st.camera.dist *= 0.45;
    h.settle();
    h.shot("zero-gear-3-close");
    h.st.camera.yaw = yaw;
    h.st.camera.pitch = pitch;
    h.st.frame_all();
    // the Array selected: its places and grips
    h.st.select(Some(teeth), false);
    h.settle();
    h.shot("zero-gear-4-array");
    // 30 teeth, then the tooth wider: every copy follows
    timed(h, &mut log, "Teeth: 30", |d| {
        if let Some(a) = d.feature_mut(teeth).and_then(|f| f.array.as_mut()) {
            a.count = 30;
        }
    });
    timed(h, &mut log, "Tooth narrower (5 mm)", |d| {
        if let Some(f) = d.feature_mut(tooth) {
            f.prim = Prim::Profile { x: 8.0, y: 8.0, z: 5.0 };
        }
    });
    h.st.select(None, false);
    h.settle();
    h.shot("zero-gear-5-thirty");
    std::fs::write(format!("{}/gear.txt", h.out), log.join("\n") + "\n").ok();
    let save = format!("{}/gear.fnurbs", h.out);
    std::fs::write(&save, serde_json::to_string_pretty(&h.st.doc).unwrap_or_default()).ok();
}

fn stats(v: &[f64]) -> (f64, f64, f64) {
    let mut s = v.to_vec();
    if s.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    s.sort_by(|a, b| a.total_cmp(b));
    (s.iter().sum::<f64>() / s.len() as f64, s[s.len() / 2], *s.last().unwrap())
}

/// The shapes `ids` selected and dragged by the move gizmo's arrow along
/// `axis` (X, Y or Z) for 60 frames at 60 a second, as `perf`: the frames'
/// app time, the kernel's answers meanwhile, the fine model after the
/// release; then undone (`object`).
fn gizmo_drag(h: &mut Harness, log: &mut Vec<String>, label: &str, ids: &[u32], axis: Vec3, shot: &str) {
    // FORME_DRAGS=hub,dome: only the drags whose label has one of these
    if let Ok(only) = std::env::var("FORME_DRAGS") {
        let l = label.to_lowercase();
        if !only.split(',').any(|w| !w.is_empty() && l.contains(&w.to_lowercase())) {
            return;
        }
    }
    h.key(egui::Key::Escape);
    h.key(egui::Key::Num4);
    h.st.selection = ids.to_vec();
    h.key(egui::Key::W);
    h.steps(2);
    h.settle();
    let Some(pivot) = h.st.pivot() else {
        eprintln!("  {label}: no pivot");
        return;
    };
    let pivot = pivot.as_vec3();
    let v = h.st.view();
    let k = v.pixel_size_at((pivot - v.eye).length()) * 78.0;
    let (a, b) = (h.screen_of(pivot + axis * k * 0.8), h.screen_of(pivot + axis * k * 1.8));
    h.move_to(a);
    h.steps(2);
    let hot = h.st.hot_handle;
    let before = h.st.doc.clone();
    let origin0 = h.st.doc.feature(ids[0]).map(|f| f.origin);
    let abandoned0 = h.st.kernel.abandoned();
    h.press(a, true);
    let mut frames: Vec<[f64; 3]> = vec![];
    let mut answers: Vec<f64> = vec![];
    let mut seen = h.st.model_gen;
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
        if let Some(left) = Duration::from_millis(16).checked_sub(f0.elapsed()) {
            std::thread::sleep(left);
        }
    }
    let took = t0.elapsed().as_secs_f64();
    h.press(b, false);
    let t1 = Instant::now();
    h.settle();
    let fine = t1.elapsed().as_secs_f64();
    if !shot.is_empty() {
        h.shot(shot);
    }
    let moved = h.st.doc.feature(ids[0]).zip(origin0).map(|(f, o)| (f.origin - o).length()).unwrap_or(0.0);
    let app = stats(&frames.iter().map(|f| f[0] + f[1]).collect::<Vec<_>>());
    let ans = stats(&answers);
    let errs = h.st.model.as_ref().map_or(0, |m| m.errors.len());
    let gave_up = h.st.kernel.abandoned() - abandoned0;
    let line = format!("{label} ({hot:?}, {moved:.1} mm): {} kernel answers in {took:.1} s ({:.1}/s), each {:.0} ms median, {:.0} max; app {:.1} ms a frame median, {:.1} max; fine model {:.2} s after release; {errs} errors{}", answers.len(), answers.len() as f64 / took, ans.1, ans.2, app.1, app.2, fine, if gave_up > 0 { format!("; {gave_up} builds given up") } else { String::new() });
    eprintln!("  {line}");
    log.push(line);
    back_to(h, log, &before);
}

/// The document's steps undone until it is `before` again (a selection
/// step is not one); said in the log when it is not (`object`).
fn back_to(h: &mut Harness, log: &mut Vec<String>, before: &Doc) {
    for _ in 0..4 {
        if h.st.doc == *before {
            break;
        }
        h.st.undo_doc();
    }
    h.settle();
    if h.st.doc != *before {
        // (a change whose build brought an error may have been undone by
        // the app already, `check_build`, and the undo then went one further)
        let d = &h.st.doc;
        let mut what: Vec<String> = d.features.iter().filter(|f| before.feature(f.id) != Some(*f)).map(|f| format!("{} {}", f.id, f.name)).collect();
        what.extend(before.features.iter().filter(|f| d.feature(f.id).is_none()).map(|f| format!("{} {} gone", f.id, f.name)));
        if d.groups != before.groups {
            what.push("groups".into());
        }
        let line = format!("  NOT BACK: the document differs after the undo ({}), loaded again", what.join(", "));
        eprintln!("{line}");
        log.push(line);
        let name = h.st.doc_name.clone();
        h.reset(before.clone(), &name);
    }
}

/// A setting changed live as a slider does (`begin_live`, then a change a
/// frame for 30 frames, `end_live`): the kernel's answers meanwhile and
/// the fine model after; then undone (`object`).
fn live_change(h: &mut Harness, log: &mut Vec<String>, label: &str, f: impl Fn(&mut Doc, f64)) {
    let before = h.st.doc.clone();
    h.st.begin_live(label);
    let mut answers: Vec<f64> = vec![];
    let mut seen = h.st.model_gen;
    let n = 30;
    let t0 = Instant::now();
    for i in 1..=n {
        let f0 = Instant::now();
        let t = i as f64 / n as f64;
        h.st.live(|d| f(d, t));
        h.frame_timed();
        if h.st.model_gen != seen {
            seen = h.st.model_gen;
            answers.push(h.st.round_ms as f64);
        }
        if let Some(left) = Duration::from_millis(16).checked_sub(f0.elapsed()) {
            std::thread::sleep(left);
        }
    }
    let took = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    h.st.end_live();
    h.settle();
    let fine = t1.elapsed().as_secs_f64();
    let ans = stats(&answers);
    let errs = h.st.model.as_ref().map(|m| m.errors.clone()).unwrap_or_default();
    let line = format!("{label}: {} kernel answers in {took:.1} s, each {:.0} ms median, {:.0} max; fine model {:.2} s after; errors {errs:?}", answers.len(), ans.1, ans.2, fine);
    eprintln!("  {line}");
    log.push(line);
    back_to(h, log, &before);
}

/// One app action timed until the fine model (`object`).
fn timed_act(h: &mut Harness, log: &mut Vec<String>, label: &str, f: impl FnOnce(&mut Harness)) {
    let t0 = Instant::now();
    f(h);
    h.settle();
    let wall = t0.elapsed().as_secs_f64() * 1e3;
    let m = h.st.model.as_ref();
    let line = format!("{:<34} {:>6.0} ms  kernel {:>6.0} ms  {:>4} faces  {} errors", label, wall, m.map_or(0.0, |m| m.ms), m.map_or(0, |m| m.faces.len()), m.map_or(0, |m| m.errors.len()));
    eprintln!("  {line}");
    log.push(line);
}

/// `object` (Romain 21 nov., 20 h 22: "une passe de test de performance en
/// essayant de modéliser un objet"): a quadcopter modelled as a user
/// would, each step one edit timed until its fine model; then the
/// gestures on the finished part (gizmo drags on a late shape, an Array's
/// source, the first shape; a fillet's size and an Array's count changed
/// live; undo, redo, Isolate, Duplicate, Delete). The document is written
/// to `<out>/drone.fnurbs` (FORME_SAVE: elsewhere) for `drag_probe`.
fn object(h: &mut Harness) {
    let mut log: Vec<String> = vec![];
    h.reset(Doc::default(), "Drone");
    let up = DQuat::IDENTITY;
    let t_all = Instant::now();
    eprintln!("  -- modelling");
    let hub = timed(h, &mut log, "Hub (cylinder)", |d| named(d, Op::Add, Prim::Cylinder { radius: 35.0, height: 18.0 }, DVec3::ZERO, up, "Hub"));
    let dome = timed(h, &mut log, "Dome (cylinder)", |d| named(d, Op::Add, Prim::Cylinder { radius: 28.0, height: 8.0 }, DVec3::new(0.0, 18.0, 0.0), up, "Dome"));
    let e = edges_between(h, (dome, 0), (dome, 1));
    let dome_fillet = timed(h, &mut log, "Fillet dome top 4", |d| d.add_round(4.0, false, e));
    let e = edges_between(h, (hub, 0), (hub, 1));
    timed(h, &mut log, "Fillet hub top 3", |d| d.add_round(3.0, false, e));
    let e = edges_between(h, (hub, 0), (hub, 2));
    timed(h, &mut log, "Fillet hub bottom 2", |d| d.add_round(2.0, false, e));
    let arm = timed(h, &mut log, "Arm (box)", |d| named(d, Op::Add, Prim::Box { x: 70.0, y: 10.0, z: 14.0 }, DVec3::new(60.0, 4.0, 0.0), up, "Arm"));
    let motor = timed(h, &mut log, "Motor (cylinder)", |d| named(d, Op::Add, Prim::Cylinder { radius: 13.0, height: 16.0 }, DVec3::new(100.0, 0.0, 0.0), up, "Motor"));
    let recess = timed(h, &mut log, "Motor recess (carve)", |d| named(d, Op::Carve, Prim::Cylinder { radius: 9.0, height: 2.0 }, DVec3::new(100.0, 15.0, 0.0), up, "Recess"));
    let shaft = timed(h, &mut log, "Shaft (cylinder)", |d| named(d, Op::Add, Prim::Cylinder { radius: 3.0, height: 8.0 }, DVec3::new(100.0, 15.0, 0.0), up, "Shaft"));
    let slot = timed(h, &mut log, "Arm lightening cut (carve)", |d| named(d, Op::Carve, Prim::Box { x: 30.0, y: 20.0, z: 6.0 }, DVec3::new(58.0, 0.0, 0.0), up, "Cut"));
    let mut e = edges_between(h, (arm, 2), (arm, 4));
    e.extend(edges_between(h, (arm, 2), (arm, 5)));
    timed(h, &mut log, "Fillet arm top edges 2", |d| d.add_round(2.0, false, e));
    let mut e = vec![];
    for (a, b) in [(0, 4), (0, 5), (1, 4), (1, 5)] {
        e.extend(edges_between(h, (slot, a), (slot, b)));
    }
    timed(h, &mut log, "Fillet cut corners 2.5", |d| d.add_round(2.5, false, e));
    let e = edges_between(h, (motor, 0), (motor, 1));
    timed(h, &mut log, "Fillet motor top 1.5", |d| d.add_round(1.5, false, e));
    let arms = timed(h, &mut log, "Array circle x4 (arm, motor)", |d| {
        d.add_array(nurbs_kernel::Array { sources: vec![arm, motor, recess, shaft, slot], kind: nurbs_kernel::ArrayKind::Circle, count: 4, step: DVec3::ZERO, count2: 1, step2: DVec3::ZERO, axis: 1, centre: DVec3::ZERO, angle: 360.0 })
    });
    timed(h, &mut log, "Battery bay (carve)", |d| named(d, Op::Carve, Prim::Box { x: 40.0, y: 8.0, z: 24.0 }, DVec3::new(0.0, -2.0, 0.0), up, "Bay"));
    let cam = timed(h, &mut log, "Camera (box)", |d| named(d, Op::Add, Prim::Box { x: 20.0, y: 12.0, z: 14.0 }, DVec3::new(0.0, 4.0, 40.0), up, "Camera"));
    let mut e = edges_between(h, (cam, 4), (cam, 0));
    e.extend(edges_between(h, (cam, 4), (cam, 1)));
    timed(h, &mut log, "Fillet camera front corners 2", |d| d.add_round(2.0, false, e));
    let lens = timed(h, &mut log, "Lens (carve, turned)", |d| named(d, Op::Carve, Prim::Cylinder { radius: 4.0, height: 6.0 }, DVec3::new(0.0, 10.0, 44.0), DQuat::from_rotation_x(std::f64::consts::FRAC_PI_2), "Lens"));
    let e = edges_between(h, (lens, 0), (cam, 4));
    timed(h, &mut log, "Chamfer lens mouth 0.8", |d| d.add_round(0.8, true, e));
    let vent = timed(h, &mut log, "Vent (carve)", |d| named(d, Op::Carve, Prim::Box { x: 2.0, y: 4.0, z: 16.0 }, DVec3::new(-12.0, 24.0, 0.0), up, "Vent"));
    timed(h, &mut log, "Array line x5 (vent)", |d| {
        d.add_array(nurbs_kernel::Array { sources: vec![vent], kind: nurbs_kernel::ArrayKind::Line, count: 5, step: DVec3::new(6.0, 0.0, 0.0), count2: 1, step2: DVec3::ZERO, axis: 1, centre: DVec3::ZERO, angle: 360.0 })
    });
    timed(h, &mut log, "Inset camera top 2, -1", |d| d.add_push(nurbs_kernel::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: cam, sub: 2 }.pack(), at: DVec3::new(0.0, 16.0, 40.0) }, inset: 2.0, depth: -1.0, keep: false }));
    timed(h, &mut log, "Mirror X", |d| d.add_mirror(nurbs_kernel::Mirror { axis: 0, offset: 0.0, flip: false, single: false }));
    let ant = timed(h, &mut log, "Antenna (cylinder)", |d| named(d, Op::Add, Prim::Cylinder { radius: 1.5, height: 20.0 }, DVec3::new(15.0, 24.0, -12.0), up, "Antenna"));
    let e = edges_between(h, (ant, 0), (ant, 1));
    timed(h, &mut log, "Fillet antenna tip 0.75", |d| d.add_round(0.75, false, e));
    let leg1 = timed(h, &mut log, "Leg front (box)", |d| named(d, Op::Add, Prim::Box { x: 4.0, y: 22.0, z: 4.0 }, DVec3::new(22.0, -20.0, 22.0), up, "Leg"));
    let leg2 = timed(h, &mut log, "Leg back (box)", |d| named(d, Op::Add, Prim::Box { x: 4.0, y: 22.0, z: 4.0 }, DVec3::new(22.0, -20.0, -22.0), up, "Leg 2"));
    let mut e = vec![];
    for leg in [leg1, leg2] {
        for (a, b) in [(0, 4), (0, 5), (1, 4), (1, 5)] {
            e.extend(edges_between(h, (leg, a), (leg, b)));
        }
    }
    timed(h, &mut log, "Fillet legs 1", |d| d.add_round(1.0, false, e));
    let t0 = Instant::now();
    h.st.edit_quiet("Group", |d| {
        d.add_group(&[leg1, leg2]);
    });
    h.settle();
    log.push(format!("Group (legs) {:.0} ms", t0.elapsed().as_secs_f64() * 1e3));
    let total = t_all.elapsed().as_secs_f64();
    log.push(format!("modelled in {total:.1} s, {} features", h.st.doc.features.len()));
    eprintln!("  modelled in {total:.1} s");
    h.st.frame_all();
    h.settle();
    h.shot("object/object-1-drone");
    let (yaw, pitch) = (h.st.camera.yaw, h.st.camera.pitch);
    h.st.camera.pitch = 1.2;
    h.st.frame_all();
    h.settle();
    h.shot("object/object-2-top");
    h.st.camera.yaw = yaw + 2.6;
    h.st.camera.pitch = -0.35;
    h.st.frame_all();
    h.settle();
    h.shot("object/object-3-under");
    h.st.camera.yaw = yaw;
    h.st.camera.pitch = pitch;
    h.st.xray = true;
    h.st.frame_all();
    h.settle();
    h.shot("object/object-4-xray");
    h.st.xray = false;
    h.st.frame_all();
    h.settle();
    let save = std::env::var("FORME_SAVE").unwrap_or_else(|_| format!("{}/drone.fnurbs", h.out));
    std::fs::write(&save, h.st.doc.to_json()).ok();
    eprintln!("  saved {save}");

    eprintln!("  -- the whole part built again from nothing");
    let d = h.st.doc.clone();
    let t0 = Instant::now();
    h.reset(d, "Drone");
    log.push(format!("loaded and built from nothing {:.2} s", t0.elapsed().as_secs_f64()));
    eprintln!("  {}", log.last().unwrap());

    eprintln!("  -- gestures");
    gizmo_drag(h, &mut log, "Drag the antenna (late, after the Mirror) along Z", &[ant], Vec3::Z, "object/object-5-antenna-dragged");
    gizmo_drag(h, &mut log, "Drag the camera along Z", &[cam], Vec3::Z, "");
    gizmo_drag(h, &mut log, "Drag the camera along X (across the Mirror's plane)", &[cam], Vec3::X, "object/object-6-camera-across");
    gizmo_drag(h, &mut log, "Drag the vent (an Array's source) along Z", &[vent], Vec3::Z, "");
    gizmo_drag(h, &mut log, "Drag the arm (an Array's source) along X", &[arm], Vec3::X, "object/object-7-arms-longer");
    gizmo_drag(h, &mut log, "Drag the dome (second shape) along Y", &[dome], Vec3::Y, "");
    gizmo_drag(h, &mut log, "Drag the hub (first shape) along X", &[hub], Vec3::X, "");
    live_change(h, &mut log, "Dome fillet 4 to 6 live", |d, t| {
        if let Some(r) = d.feature_mut(dome_fillet).and_then(|f| f.round.as_mut()) {
            r.size = 4.0 + 2.0 * t;
        }
    });
    live_change(h, &mut log, "Arms Array 4 to 6 live", |d, t| {
        if let Some(a) = d.feature_mut(arms).and_then(|f| f.array.as_mut()) {
            a.count = 4 + (t * 2.0).round() as u32;
        }
    });
    timed_act(h, &mut log, "Undo (the legs' fillet)", |h| h.st.undo_doc());
    timed_act(h, &mut log, "Redo", |h| h.st.redo_doc());
    timed_act(h, &mut log, "Isolate the camera", |h| {
        h.st.selection = vec![cam];
        h.steps(1);
        h.st.toggle_isolate();
    });
    timed_act(h, &mut log, "Isolate off", |h| h.st.toggle_isolate());
    timed_act(h, &mut log, "Duplicate the antenna", |h| {
        h.st.selection = vec![ant];
        h.steps(1);
        h.st.duplicate_selected();
    });
    timed_act(h, &mut log, "Undo the copy", |h| h.st.undo_doc());
    timed_act(h, &mut log, "Delete the camera", |h| {
        h.st.selection = vec![cam];
        h.steps(1);
        h.st.delete_selected();
    });
    timed_act(h, &mut log, "Undo the delete", |h| h.st.undo_doc());
    let path = format!("{}/object.txt", h.out);
    std::fs::write(&path, log.join("\n") + "\n").ok();
    eprintln!("  written {path}");
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
        // only what is seen first; BACKFACES (23 nov.) at the end
        h.st.settings.select_through = false;
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
        h.st.settings.select_through = false;
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
        // BACKFACES on (Romain 23 nov., 14 h 51: "il doit prendre toutes
        // les faces y compris celle derrière", backface culling off): the
        // far side too, 6 faces, 12 edges, 8 vertices
        h.st.settings.select_through = true;
        h.key(egui::Key::F11);
        h.drag(a, b, 10, true);
        h.settle();
        let faces = h.st.face_sel.len();
        h.shot("zero-marquee-7-thin-backfaces");
        h.key(egui::Key::F10);
        h.drag(a, b, 10, true);
        h.settle();
        let edges = h.st.edge_sel.len();
        h.key(egui::Key::F9);
        h.drag(a, b, 10, true);
        h.settle();
        eprintln!("  thin wall, backfaces: {faces} faces, {edges} edges, {} vertices picked (all: 6, 12, 8)", h.st.vert_sel.len());
    }

    if want("fill-hole") {
        // Fill Hole (Romain 23 nov., 14 h 52: "Il faut un tool fillHole"):
        // two faces of a block deleted (one hole round both), the toast's
        // Fill hole; then a face deleted again, its border picked in Edge
        // mode, its window's Fill hole
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Block");
        h.st.settings.select_through = false;
        h.settle();
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(20.0, 10.0, 0.0)));
        h.hold(Modifiers::SHIFT);
        h.click(h.screen_of(Vec3::new(0.0, 20.0, 0.0)));
        h.hold(Modifiers::NONE);
        h.settle();
        h.key(egui::Key::Delete);
        h.settle();
        eprintln!("  deleted: toast {:?}", h.st.toast.as_ref().map(|t| (t.text.clone(), t.acts.iter().map(|a| a.0.clone()).collect::<Vec<_>>())));
        h.steps(3);
        h.shot("zero-fill-1-deleted");
        let act = h.st.toast.as_ref().and_then(|t| t.acts.first().map(|a| a.1));
        if let Some(a) = act {
            h.st.toast_act(a);
        }
        h.settle();
        let open = |h: &Harness| h.st.model.as_ref().map(|m| m.edges.iter().filter(|e| !e.seam && (e.faces[0] < 0 || e.faces[1] < 0)).count());
        eprintln!("  filled from the toast: faces {:?}, open edges {:?}, errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()), open(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-fill-2-filled");
        // the front deleted, its border picked, the window's button
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(-5.0, 8.0, 15.0)));
        h.key(egui::Key::Delete);
        h.settle();
        h.st.toast = None;
        h.key(egui::Key::F10);
        h.click(h.screen_of(Vec3::new(-10.0, 0.0, 15.0)));
        h.settle();
        h.steps(3);
        eprintln!("  border picked: {:?}, open edges {:?}", h.st.border_edges_picked(), open(&h));
        h.shot("zero-fill-3-border-picked");
        let at = h.st.border_edges_picked();
        h.st.fill_holes(at);
        h.settle();
        eprintln!("  filled: faces {:?}, open edges {:?}, errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()), open(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-fill-4-closed");
    }

    if want("convert") {
        // Ctrl + a mode (Maya's Convert Selection; Romain 23 nov., 16 h 11)
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Block");
        h.settle();
        let count = |h: &Harness| format!("{:?}: faces {}, edges {}, vertices {}, shapes {}", h.st.sel_mode, h.st.face_sel.len(), h.st.edge_sel.len(), h.st.vert_sel.len(), h.st.selection.len());
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(0.0, 20.0, 0.0)));
        h.settle();
        eprintln!("  picked: {}", count(&h));
        h.shot("zero-convert-1-face");
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::F10);
        h.hold(Modifiers::NONE);
        h.steps(3);
        eprintln!("  Ctrl + F10: {}", count(&h));
        h.shot("zero-convert-2-edges");
        h.st.convert_selection(SelMode::Vertex);
        h.steps(3);
        eprintln!("  to vertices: {}", count(&h));
        h.shot("zero-convert-3-vertices");
        h.st.convert_selection(SelMode::Face);
        h.steps(3);
        eprintln!("  to faces: {}", count(&h));
        h.shot("zero-convert-4-faces");
        h.st.convert_selection(SelMode::Object);
        h.steps(3);
        eprintln!("  to object: {}", count(&h));
        h.st.convert_selection(SelMode::Edge);
        h.steps(3);
        eprintln!("  object to edges: {}", count(&h));
        // one edge to its two faces
        h.key(egui::Key::Escape);
        h.key(egui::Key::F10);
        h.click(h.screen_of(Vec3::new(8.0, 20.0, 15.0)));
        h.st.convert_selection(SelMode::Face);
        h.steps(3);
        eprintln!("  one edge to faces: {}", count(&h));
        h.shot("zero-convert-5-edge-to-faces");
        h.st.undo();
        h.steps(2);
        eprintln!("  undo: {}", count(&h));
    }

    if want("settings") {
        // Romain 23 nov., 17 h 59: the Settings window (light theme, the
        // interface's size), the Documents window clickable after its dim
        // closed it once, Q after an Inset (its faces picked again), a
        // material with no face picked (the whole element)
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Block");
        h.settle();
        let gear = h.ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("settings-btn"))).map(|r| r.center());
        eprintln!("  gear at {gear:?}");
        if let Some(g) = gear {
            h.click(g);
        }
        h.steps(3);
        eprintln!("  settings open: {}", h.st.settings_open);
        h.shot("zero-settings-1-window");
        // the Interface size: its buttons clicked (Romain 24 nov., 8 h 37)
        for pct in [75u32, 125, 100] {
            if let Some(r) = h.ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("ui-scale-btn", pct)))) {
                h.click(r.center());
                h.steps(4);
                eprintln!("  clicked {pct} %: {:.0} %", h.st.settings.ui_scale * 100.0);
                h.shot(&format!("zero-settings-2-size-{pct}"));
            } else {
                eprintln!("  no {pct} % button");
            }
        }
        h.st.settings.light = true;
        h.st.settings.ui_scale = 1.25;
        h.steps(4);
        h.shot("zero-settings-2-light-125");
        h.st.settings.light = false;
        h.st.settings.ui_scale = 1.0;
        h.st.settings_open = false;
        h.steps(4);
        // the Documents window: closed by a click on its dim, opened again,
        // its name field clicked
        h.st.files_open = true;
        h.steps(3);
        h.click(Pos2::new(60.0, 860.0));
        h.steps(3);
        eprintln!("  closed by the dim: {}", !h.st.files_open);
        h.st.files_open = true;
        h.steps(3);
        h.click(Pos2::new(700.0, 385.0));
        h.steps(3);
        let focus = h.ctx.memory(|m| m.focused());
        eprintln!("  reopened: open {}, a field focused {:?}", h.st.files_open, focus.is_some());
        h.shot("zero-settings-3-documents");
        h.st.files_open = false;
        h.steps(3);
        // Inset on two faces, then Q: the two inset faces picked, the Face
        // window again
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(0.0, 20.0, 0.0)));
        h.hold(Modifiers::SHIFT);
        h.click(h.screen_of(Vec3::new(20.0, 10.0, 0.0)));
        h.hold(Modifiers::NONE);
        h.key(egui::Key::I);
        h.settle();
        eprintln!("  inset: selected {:?}, faces {}", h.st.selection, h.st.face_sel.len());
        h.key(egui::Key::Q);
        h.settle();
        h.steps(3);
        eprintln!("  Q: {:?}, selected {:?}, faces {:?}", h.st.sel_mode, h.st.selection, h.st.face_sel.iter().map(|r| r.tag).collect::<Vec<_>>());
        h.shot("zero-settings-4-q-after-inset");
        // a material with no face picked: the whole element
        h.key(egui::Key::Escape);
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(0.0, 10.0, 15.0)));
        h.steps(2);
        eprintln!("  clicked: faces {:?}, last element {:?}", h.st.face_sel.iter().map(|r| r.tag).collect::<Vec<_>>(), h.st.last_element);
        h.st.face_sel.clear();
        h.steps(2);
        h.st.apply_material(3);
        eprintln!("  toast {:?}", h.st.toast.as_ref().map(|t| t.text.clone()));
        h.steps(2);
        eprintln!("  material with no face picked: block material {:?}, painted faces {}", h.st.doc.features.first().map(|f| f.material), h.st.doc.paint.len());
    }

    if want("multi") {
        // The tools on several picked components (Romain 23 nov., 15 h 19:
        // "Les tools doivent fonctionner sur multi selection… 2 faces, fait
        // Inset, ça perform que sur 1 face"): three faces inset with I, the
        // window's sizes given to all, undone in one step; Ctrl + drag on
        // two faces; two edges pulled out by Shift + drag
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Block");
        h.settle();
        let pushes = |h: &Harness| h.st.doc.features.iter().filter_map(|f| f.push.as_ref().map(|p| (f.name.clone(), p.inset, p.depth))).collect::<Vec<_>>();
        let errs = |h: &Harness| h.st.model.as_ref().map(|m| m.errors.clone()).unwrap_or_default();
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(20.0, 10.0, 0.0)));
        h.hold(Modifiers::SHIFT);
        h.click(h.screen_of(Vec3::new(0.0, 20.0, 0.0)));
        h.click(h.screen_of(Vec3::new(0.0, 10.0, 15.0)));
        h.hold(Modifiers::NONE);
        h.settle();
        eprintln!("  picked {} faces", h.st.face_sel.len());
        h.shot("zero-multi-1-picked");
        h.key(egui::Key::I);
        h.settle();
        h.steps(3);
        eprintln!("  I: {:?}, selected {:?}, errors {:?}", pushes(&h), h.st.selection, errs(&h));
        h.shot("zero-multi-2-inset");
        h.st.set_push(4.0, 0.0, true);
        h.st.set_push(4.0, -3.0, true);
        h.st.end_live();
        h.settle();
        h.steps(3);
        eprintln!("  window 4 mm then depth -3: {:?}, errors {:?}", pushes(&h), errs(&h));
        h.shot("zero-multi-3-sized");
        // one undo takes the depth, one the inset size, one all three insets
        h.st.undo();
        h.st.undo();
        h.st.undo();
        h.settle();
        eprintln!("  three undos: {:?}", pushes(&h));
        // Ctrl + drag on two picked faces
        h.key(egui::Key::Escape);
        h.key(egui::Key::F11);
        let top = h.screen_of(Vec3::new(0.0, 20.0, 0.0));
        h.click(top);
        h.hold(Modifiers::SHIFT);
        h.click(h.screen_of(Vec3::new(20.0, 10.0, 0.0)));
        h.hold(Modifiers::CTRL);
        h.press(top, true);
        for i in 1..=10 {
            h.move_to(top + egui::vec2(4.0 * i as f32, 0.0));
        }
        h.press(top + egui::vec2(40.0, 0.0), false);
        h.hold(Modifiers::NONE);
        h.settle();
        h.steps(3);
        eprintln!("  Ctrl + drag: {:?}, errors {:?}", pushes(&h), errs(&h));
        h.shot("zero-multi-4-ctrl-drag");
        h.st.undo();
        h.settle();
        // two top edges pulled out together
        h.key(egui::Key::Escape);
        h.key(egui::Key::F10);
        let e1 = h.screen_of(Vec3::new(20.0, 20.0, 0.0));
        h.click(e1);
        h.hold(Modifiers::SHIFT);
        h.click(h.screen_of(Vec3::new(-20.0, 20.0, 0.0)));
        h.hold(Modifiers::NONE);
        eprintln!("  edges picked {}", h.st.edge_sel.len());
        let up = h.screen_of(Vec3::new(20.0, 26.0, 0.0));
        h.hold(Modifiers::SHIFT);
        h.press(e1, true);
        for i in 1..=10 {
            h.move_to(e1 + (up - e1) * (i as f32 / 10.0));
        }
        h.press(up, false);
        h.hold(Modifiers::NONE);
        h.settle();
        h.steps(3);
        let tw = h.st.doc.features.iter().filter_map(|f| f.tweak.as_ref().map(|t| (f.name.clone(), t.comps().count(), (t.delta * 100.0).round() / 100.0))).collect::<Vec<_>>();
        eprintln!("  Shift + drag on 2 edges: {:?}, errors {:?}", tw, errs(&h));
        h.shot("zero-multi-5-edges-pulled");
    }

    if want("panels") {
        // Romain 24 nov., 6 h 18: the shape list alone, Q W E R in the top
        // bar, the Transform Values under W / E / R (right click or held),
        // the view ball a sphere, Import scan in the scene list
        let mut d = Doc::default();
        let a = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let b = d.add(Op::Add, Prim::Cylinder { radius: 8.0, height: 30.0 }, DVec3::new(40.0, 0.0, 0.0), DQuat::IDENTITY);
        h.reset(d, "Block");
        h.st.settings.nav = true;
        h.settle();
        h.key(egui::Key::Escape);
        h.key(egui::Key::Num4);
        h.click(h.screen_of(Vec3::new(0.0, 20.0, 0.0)));
        h.steps(3);
        h.shot("zero-panels-1-selected");
        let btn = |h: &Harness, i: usize| h.ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("gizmo-btn", i)))).map(|r| r.center());
        if let Some(w) = btn(&h, 1) {
            h.rpress(w, true);
            h.rpress(w, false);
            h.steps(3);
        }
        eprintln!("  right click on W: values open {}", h.ctx.data(|d| d.get_temp::<(u8, egui::Rect)>(egui::Id::new("xform-pop"))).is_some());
        h.shot("zero-panels-2-move-values");
        h.key(egui::Key::Escape);
        h.steps(2);
        // held on E
        if let Some(e) = btn(&h, 2) {
            h.press(e, true);
            for _ in 0..35 {
                h.step();
            }
            h.press(e, false);
            h.steps(3);
        }
        eprintln!("  held on E: gizmo {:?}, values open {}", h.st.gizmo, h.ctx.data(|d| d.get_temp::<(u8, egui::Rect)>(egui::Id::new("xform-pop"))).is_some());
        h.shot("zero-panels-3-rotate-held");
        h.key(egui::Key::Escape);
        // a group: its own transform
        h.st.selection = vec![a, b];
        h.st.group_selection();
        h.steps(3);
        if let Some(r) = btn(&h, 3) {
            h.rpress(r, true);
            h.rpress(r, false);
            h.steps(3);
        }
        h.shot("zero-panels-4-group-scale-values");
        h.key(egui::Key::Escape);
        h.st.ungroup(h.st.doc.groups[0].id);
        h.steps(2);
        // renamed in the list by a double-click
        // past the double-click time of the clicks before
        h.steps(30);
        if let Some(p) = h.ctx.data(|d| d.get_temp::<Pos2>(egui::Id::new(("row", a)))) {
            h.click(p);
            h.click(p);
            h.steps(2);
            eprintln!("  renaming: {:?}", h.ctx.data(|d| d.get_temp::<(u8, u32)>(egui::Id::new("tree-rename"))));
            h.type_text("Base");
            eprintln!("  typed: {:?}", h.ctx.data(|d| d.get_temp::<String>(egui::Id::new("tree-rename-buf"))));
            h.key(egui::Key::Enter);
            h.steps(3);
        }
        eprintln!("  renamed: {:?}", h.st.doc.feature(a).map(|f| f.name.clone()));
        h.shot("zero-panels-5-renamed");
    }

    if want("history-off") {
        // History off (Romain 23 nov., 22 h 05: "mettre une option de
        // history en OFF"): an inset then an extrusion on a block, each
        // folded into the block once made; Ctrl + Z brings the history back
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Block");
        h.st.settings.history_off = true;
        let list = |h: &Harness| h.st.doc.features.iter().map(|f| format!("{}{}", f.name, if f.frozen.is_some() { " (one solid)" } else { "" })).collect::<Vec<_>>();
        let wait = |h: &mut Harness| {
            for _ in 0..4 {
                h.settle();
                h.steps(3);
            }
        };
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(0.0, 20.0, 0.0)));
        h.key(egui::Key::I);
        wait(&mut h);
        eprintln!("  after I: {:?}, toast {:?}", list(&h), h.st.toast.as_ref().map(|t| t.text.clone()));
        h.shot("zero-history-off-1-inset");
        h.key(egui::Key::F11);
        let top = h.screen_of(Vec3::new(0.0, 20.0, 0.0));
        h.click(top);
        let up = h.screen_of(Vec3::new(0.0, 28.0, 0.0));
        h.hold(Modifiers::SHIFT);
        h.press(top, true);
        for i in 1..=10 {
            h.move_to(top + (up - top) * (i as f32 / 10.0));
        }
        h.press(up, false);
        h.hold(Modifiers::NONE);
        wait(&mut h);
        eprintln!("  after Shift + drag: {:?}, errors {:?}", list(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-history-off-2-extruded");
        h.st.undo();
        wait(&mut h);
        eprintln!("  undo: {:?}", list(&h));
        h.shot("zero-history-off-3-undo");
        // a plain cube scaled then turned with History off (Romain's journal,
        // 24 nov., 5 h 17: each Rotate and Scale followed by "Undo")
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Box { x: 20.0, y: 20.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Cube");
        h.st.settings.history_off = true;
        h.key(egui::Key::Escape);
        h.key(egui::Key::Num4);
        h.st.selection = vec![c];
        for (key, name) in [(egui::Key::R, "scale"), (egui::Key::E, "rotate")] {
            h.key(key);
            h.steps(2);
            h.settle();
            let Some(pivot) = h.st.pivot() else { continue };
            let pivot = pivot.as_vec3();
            let v = h.st.view();
            let k = v.pixel_size_at((pivot - v.eye).length()) * 78.0;
            let (a, b) = if name == "scale" {
                (h.screen_of(pivot + Vec3::X * k * 0.6), h.screen_of(pivot + Vec3::X * k * 1.3))
            } else {
                (h.screen_of(pivot + Vec3::new(0.0, 0.0, 1.0) * k), h.screen_of(pivot + Vec3::new(0.7, 0.0, 0.7) * k))
            };
            h.move_to(a);
            h.steps(2);
            eprintln!("  {name}: handle {:?}", h.st.hot_handle);
            h.drag(a, b, 10, true);
            wait(&mut h);
            let undo: Vec<String> = h.st.undo_labels();
            eprintln!("  {name}: {:?}, undo {:?}, toast {:?}, prim {:?} rot {:?}", list(&h), undo, h.st.toast.as_ref().map(|t| t.text.clone()), h.st.doc.feature(c).map(|f| f.prim), h.st.doc.feature(c).map(|f| f.rot));
        }
        h.shot("zero-history-off-4-scaled-turned");
    }

    if want("objects") {
        // Objects (Romain 24 nov., 6 h 01: "nommer un UBER Object afin de
        // hiérarchiser… je voudrais que le Cube soustraie le gros pavé vert
        // mais pas le cylindre"): his scene's three shapes (a green block, a
        // cylinder lying on it, a cube cutting both) opened: one Object;
        // the cylinder made an Object of its own: not cut any more; a
        // click takes an Object, a double-click enters it
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 60.0, y: 30.0, z: 40.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(b).unwrap().material = 1;
        let c = d.add(Op::Add, Prim::Cylinder { radius: 9.0, height: 60.0 }, DVec3::new(-30.0, 30.0, 0.0), DQuat::from_rotation_z(-std::f64::consts::FRAC_PI_2));
        d.feature_mut(c).unwrap().material = 4;
        let k = d.add(Op::Carve, Prim::Box { x: 14.0, y: 30.0, z: 14.0 }, DVec3::new(18.0, 22.0, 0.0), DQuat::IDENTITY);
        h.reset(d, "Block");
        let wait = |h: &mut Harness| {
            for _ in 0..4 {
                h.settle();
                h.steps(3);
            }
        };
        wait(&mut h);
        let objs = |h: &Harness| h.st.doc.objects.iter().map(|o| format!("{} {:?}", o.name, o.members)).collect::<Vec<_>>();
        eprintln!("  opened: {:?}", objs(&h));
        h.key(egui::Key::Num4);
        h.click(h.screen_of(Vec3::new(-20.0, 30.0, 15.0)));
        h.steps(3);
        eprintln!("  click on the block: selection {:?}, object {:?}", h.st.selection, h.st.selected_object().map(|o| o.name.clone()));
        h.shot("zero-objects-1-one-object");
        // the cylinder out, as its row's "New Object from it"
        h.st.shapes_to_object(&[c], None);
        wait(&mut h);
        eprintln!("  cylinder alone: {:?}, faces of the cylinder {}", objs(&h), h.st.model.as_ref().map(|m| m.faces.iter().filter(|f| f.tag.is_some_and(|t| t.feature == c)).count()).unwrap_or(0));
        h.click(h.screen_of(Vec3::new(-20.0, 30.0, 15.0)));
        h.steps(3);
        h.shot("zero-objects-2-cylinder-alone");
        // a double-click on the block: into its Object, the block alone
        let at = h.screen_of(Vec3::new(-20.0, 30.0, 15.0));
        h.click(at);
        h.click(at);
        h.steps(3);
        eprintln!("  double-click: open {:?}, selection {:?}", h.st.open_object, h.st.selection);
        h.shot("zero-objects-3-entered");
        h.key(egui::Key::Escape);
        h.steps(3);
        eprintln!("  Esc: open {:?}, selection {:?}", h.st.open_object, h.st.selection);
        // the cube made Intersect from its glyph, then Sub again
        h.st.cycle_op(k);
        h.st.cycle_op(k);
        wait(&mut h);
        h.shot("zero-objects-4-intersect");
        h.st.cycle_op(k);
        // a new cube on the cylinder: in the cylinder's Object
        let mut n = 0;
        h.st.edit("Cube", |d| n = d.add(Op::Add, Prim::Box { x: 8.0, y: 8.0, z: 8.0 }, DVec3::new(-34.0, 39.0, 0.0), DQuat::IDENTITY));
        wait(&mut h);
        eprintln!("  new cube: in {:?} (cylinder's {:?})", h.st.doc.object_of(n), h.st.doc.object_of(c));
        h.st.select_object(h.st.doc.object_of(c).unwrap(), false);
        h.steps(3);
        h.shot("zero-objects-5-new-cube");
        // the new cube's row dragged onto the block's Object row (Romain
        // 7 h 32: "déplacer des sous-éléments d'un objet à un autre")
        let pos = |h: &Harness, id: egui::Id| h.ctx.data(|d| d.get_temp::<Pos2>(id));
        let ob = h.st.doc.object_of(b).unwrap();
        h.st.select(None, false);
        h.steps(3);
        if let (Some(from), Some(to)) = (pos(&h, egui::Id::new(("row", n))), pos(&h, egui::Id::new(("object-row", ob)))) {
            h.move_to(from);
            h.steps(1);
            h.press(from, true);
            for k in 1..=12 {
                h.move_to(from + (to - from) * (k as f32 / 12.0));
                h.steps(1);
            }
            h.shot("zero-objects-6-dragging");
            h.press(to, false);
            wait(&mut h);
            eprintln!("  row dropped: cube in {:?} (block's {:?}): {:?}", h.st.doc.object_of(n), ob, objs(&h));
        } else {
            eprintln!("  rows not drawn");
        }
        h.shot("zero-objects-7-dropped");
        // F2 renames the Object selected
        h.st.select_object(ob, false);
        h.steps(2);
        h.key(egui::Key::F2);
        h.steps(2);
        h.type_text("Body");
        h.steps(1);
        h.key(egui::Key::Enter);
        h.move_to(Pos2::new(900.0, 600.0));
        h.steps(3);
        eprintln!("  F2: {:?}", objs(&h));
        h.shot("zero-objects-8-renamed");
    }

    if want("object-xf") {
        // Romain 24 nov., 10 h 03: "une seule entité comme si je scale un
        // mesh". A block with a hole, a fillet and an inset (one Object), a
        // post beside it (another): the Object moved, turned, scaled, then
        // stretched by a side square; an inset made on it stretched; each
        // step's kernel time. Then 10 h 21's list: T on three faces twice,
        // the Knife's notches, an extrusion keeping its base line
        let tg = |f: u32, s: u16| FaceTag { feature: f, sub: s }.pack();
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-8.0, -5.0, 0.0), DQuat::IDENTITY);
        d.add_round(3.0, false, vec![EdgeRef { faces: [tg(b, 2), tg(b, 4)], at: DVec3::new(0.0, 20.0, 15.0) }]);
        d.add_push(nurbs_kernel::Push { face: nurbs_kernel::FaceRef { tag: tg(b, 2), at: DVec3::new(10.0, 20.0, -5.0) }, inset: 2.0, depth: -1.0, keep: false });
        d.add(Op::Add, Prim::Cylinder { radius: 6.0, height: 15.0 }, DVec3::new(60.0, 0.0, 0.0), DQuat::IDENTITY);
        h.reset(d, "Block");
        let mut log: Vec<String> = vec![];
        let state = |h: &Harness, what: &str| {
            let m = h.st.model.as_ref();
            let o = h.st.doc.objects.iter().find(|o| o.members.contains(&b));
            eprintln!(
                "  {what}: kernel {:.0} ms, {} faces, errors {:?}, frame {:?}, stretched {}",
                m.map_or(0.0, |m| m.ms),
                m.map_or(0, |m| m.faces.len()),
                m.map(|m| m.errors.clone()).unwrap_or_default(),
                o.map(|o| (o.frame.t.to_array().map(|v| (v * 100.0).round() / 100.0), (o.frame.s * 1000.0).round() / 1000.0)),
                o.is_some_and(|o| h.st.doc.is_stretched(o.id))
            );
        };
        h.key(egui::Key::Num4);
        h.click(h.screen_of(Vec3::new(5.0, 20.0, -8.0)));
        eprintln!("  picked: {:?} (Object {:?})", h.st.selection, h.st.selected_object().map(|o| o.name.clone()));
        h.shot("zero-object-xf-1-picked");
        let ids = h.st.selection.clone();
        gizmo_drag(&mut h, &mut log, "Object moved (and undone)", &ids, Vec3::X, "");
        h.st.selection = ids.clone();
        // moved along X by the arrow
        h.key(egui::Key::W);
        h.steps(2);
        let gz = |h: &Harness| {
            let pivot = h.st.pivot().unwrap().as_vec3();
            let v = h.st.view();
            (pivot, v.pixel_size_at((pivot - v.eye).length()) * 78.0)
        };
        let (pv, k) = gz(&h);
        h.drag(h.screen_of(pv + Vec3::X * k * 0.8), h.screen_of(pv + Vec3::X * (k * 0.8 + 25.0)), 12, true);
        h.settle();
        state(&h, "moved");
        h.shot("zero-object-xf-2-moved");
        // turned 30° round Y
        h.key(egui::Key::E);
        h.steps(2);
        let (pv, k) = gz(&h);
        let a = h.screen_of(pv + Vec3::X * k);
        let c = h.screen_of(pv + Vec3::new(0.866, 0.0, -0.5) * k);
        h.move_to(a);
        h.steps(2);
        h.hold(Modifiers::SHIFT);
        h.drag(a, c, 10, true);
        h.hold(Modifiers::NONE);
        h.settle();
        state(&h, "turned");
        h.shot("zero-object-xf-3-turned");
        // scaled ×1.5 by the white cube (to the right)
        h.key(egui::Key::R);
        h.steps(2);
        let (pv, _) = gz(&h);
        let s0 = h.screen_of(pv);
        h.drag(s0, s0 + vec2(70.2, 0.0), 12, true);
        h.settle();
        state(&h, "scaled");
        h.shot("zero-object-xf-4-scaled");
        // stretched by its +X side's square
        if let Some(cg) = h.st.cage() {
            let g = cg.point([1, 0, 0]);
            let to = g + cg.axis(0) * 15.0;
            h.move_to(h.screen_of(g.as_vec3()));
            h.steps(2);
            eprintln!("  side grip hovered: {:?}", h.st.hot_handle);
            h.drag(h.screen_of(g.as_vec3()), h.screen_of(to.as_vec3()), 12, true);
            h.settle();
            h.settle();
        }
        state(&h, "stretched");
        h.shot("zero-object-xf-5-stretched");
        // an inset on its top as it is seen, stretched
        let top = {
            let m = h.st.model.as_ref().unwrap();
            let i = m.faces.iter().position(|f| f.tag.is_some_and(|t| t.feature == b && t.sub == 2)).unwrap_or(0);
            let tri = m.tri_face.iter().position(|&f| f as usize == i).unwrap_or(0);
            let t = m.triangles[tri];
            (0..3).map(|j| Vec3::from(m.positions[t[j] as usize])).sum::<Vec3>() / 3.0
        };
        h.key(egui::Key::F11);
        h.click(h.screen_of(top));
        h.key(egui::Key::I);
        h.settle();
        state(&h, "inset on the stretched top");
        h.shot("zero-object-xf-6-inset");
        let n = h.st.doc.features.len();
        eprintln!("  features: {n}, undo steps: {}", h.st.undo.len());
        for l in &log {
            eprintln!("  {l}");
        }
        // T on three faces of a block, twice: each along its own normal
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Block");
        h.key(egui::Key::F11);
        h.click(h.screen_of(Vec3::new(0.0, 20.0, 0.0)));
        h.hold(Modifiers::SHIFT);
        h.click(h.screen_of(Vec3::new(20.0, 10.0, 0.0)));
        h.click(h.screen_of(Vec3::new(0.0, 10.0, 15.0)));
        h.hold(Modifiers::NONE);
        h.key(egui::Key::T);
        h.steps(2);
        for round in 0..2 {
            let (pv, k) = gz(&h);
            let n = h.st.gizmo_rot() * DVec3::Y;
            let a = h.screen_of(pv + n.as_vec3() * k * 0.8);
            h.drag(a, h.screen_of(pv + n.as_vec3() * (k * 0.8 + 4.0)), 10, true);
            h.settle();
            let tw: Vec<String> = h.st.doc.features.iter().filter_map(|f| f.tweak.as_ref().map(|t| format!("{} {:?}", f.name, t.delta.to_array().map(|v| (v * 100.0).round() / 100.0)))).collect();
            eprintln!("  T drag {}: faces still picked {}, steps {tw:?}, bounds {:?}", round + 1, h.st.face_sel.len(), h.st.model.as_ref().and_then(|m| m.bounds()));
        }
        h.shot("zero-object-xf-7-normals");
        // an extrusion keeping its base line
        let mut d = Doc::default();
        let bb = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let ins = d.add_push(nurbs_kernel::Push { face: nurbs_kernel::FaceRef { tag: tg(bb, 4), at: DVec3::new(0.0, 10.0, 15.0) }, inset: 5.0, depth: 0.0, keep: false });
        d.add_push(nurbs_kernel::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: ins, sub: 1 }.pack(), at: DVec3::new(0.0, 10.0, 15.0) }, inset: 0.0, depth: 8.0, keep: true });
        d.add_push(nurbs_kernel::Push { face: nurbs_kernel::FaceRef { tag: tg(bb, 2), at: DVec3::new(0.0, 20.0, 0.0) }, inset: 0.0, depth: 6.0, keep: true });
        h.reset(d, "Block");
        eprintln!("  extrusions kept: {} faces, errors {:?}", h.st.model.as_ref().map_or(0, |m| m.faces.len()), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-object-xf-8-keep-line");
        // the Knife's notches: Ctrl over an edge of a box
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Block");
        h.key(egui::Key::K);
        h.hold(Modifiers::CTRL);
        h.move_to(h.screen_of(Vec3::new(7.3, 20.0, 15.0)));
        h.steps(3);
        eprintln!("  knife notch: {:?}", h.st.gizmo_readout);
        h.shot("zero-object-xf-9-knife-notch");
        h.hold(Modifiers::NONE);
    }

    if want("universal") {
        // Y, the universal box (Romain 24 nov., 10 h 21: "un qui regroupe en
        // même temps le Translate, rotate et scale"): a block turned 30° with
        // a hole; its box's arrow, an arc, a side square, the ring
        let mut d = Doc::default();
        let q = DQuat::from_rotation_y(0.5236);
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, q);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, q * DVec3::new(-8.0, -5.0, 0.0), q);
        h.reset(d, "Block");
        h.key(egui::Key::Num4);
        h.click(h.screen_of(Vec3::new(8.0, 20.0, 4.0)));
        h.key(egui::Key::Y);
        h.steps(3);
        let say = |h: &Harness, what: &str| {
            let o = h.st.doc.objects.first();
            eprintln!("  {what}: frame {:?}, hot {:?}, errors {:?}", o.map(|o| (o.frame.t.to_array().map(|v| (v * 10.0).round() / 10.0), o.frame.q.to_axis_angle().1.to_degrees().round(), (o.frame.s * 100.0).round() / 100.0)), h.st.hot_handle, h.st.model.as_ref().map(|m| m.errors.clone()));
        };
        h.shot("zero-universal-1-box");
        let cb = h.st.cage().unwrap();
        let v = h.st.view();
        let px = |w: DVec3| v.pixel_size_at((w.as_vec3() - v.eye).length()) as f64;
        // the +X side's arrow, along the box's own X
        let g = cb.point([1, 0, 0]);
        let a0 = g + cb.axis(0) * px(g) * 30.0;
        h.move_to(h.screen_of(a0.as_vec3()));
        h.steps(2);
        say(&h, "arrow hovered");
        h.shot("zero-universal-2-arrow-hover");
        h.drag(h.screen_of(a0.as_vec3()), h.screen_of((a0 + cb.axis(0) * 15.0).as_vec3()), 12, true);
        h.settle();
        say(&h, "moved 15 along its X");
        // an arc round the vertical (an edge along Y)
        let cb = h.st.cage().unwrap();
        let m = cb.point([1, 0, 1]);
        let r = px(m) * 18.0;
        let a1 = m + (cb.axis(0) + cb.axis(2)).normalize() * r;
        h.move_to(h.screen_of(a1.as_vec3()));
        h.steps(2);
        say(&h, "arc hovered");
        let piv = h.st.pivot().unwrap();
        let to = piv + DQuat::from_rotation_y(-0.6) * (a1 - piv);
        h.hold(Modifiers::SHIFT);
        h.drag(h.screen_of(a1.as_vec3()), h.screen_of(to.as_vec3()), 12, true);
        h.hold(Modifiers::NONE);
        h.settle();
        say(&h, "turned by its arc");
        h.shot("zero-universal-3-turned");
        // its +Z side's square: stretched
        let cb = h.st.cage().unwrap();
        let g = cb.point([0, 0, 1]);
        h.move_to(h.screen_of(g.as_vec3()));
        h.steps(2);
        say(&h, "side square hovered");
        h.drag(h.screen_of(g.as_vec3()), h.screen_of((g + cb.axis(2) * 10.0).as_vec3()), 12, true);
        h.settle();
        h.settle();
        say(&h, "stretched along its Z");
        h.shot("zero-universal-4-stretched");

        // components (Romain 24 nov., 11 h 04): a face along its normal, an
        // edge's arrows and centre, a vertex alone sliding on its edges
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Block");
        let tweaks = |h: &Harness| h.st.doc.features.iter().filter_map(|f| f.tweak.as_ref().map(|t| (f.name.clone(), t.delta.to_array().map(|v| (v * 100.0).round() / 100.0)))).collect::<Vec<_>>();
        let arrow_of = |h: &Harness, dir: DVec3| {
            let v = h.st.view();
            let q = h.st.pivot().unwrap();
            let px = v.pixel_size_at((q.as_vec3() - v.eye).length()) as f64;
            q + dir * px * 44.0
        };
        // a face: its arrow along its normal
        h.key(egui::Key::Num3);
        h.click(h.screen_of(Vec3::new(6.0, 20.0, 4.0)));
        h.key(egui::Key::Y);
        h.steps(3);
        h.shot("zero-universal-5-face");
        let a = arrow_of(&h, DVec3::Y);
        h.move_to(h.screen_of(a.as_vec3()));
        h.steps(2);
        say(&h, "face's normal arrow hovered");
        h.drag(h.screen_of(a.as_vec3()), h.screen_of((a + DVec3::Y * 6.0).as_vec3()), 12, true);
        h.settle();
        eprintln!("  face moved along its normal: {:?}", tweaks(&h));
        h.shot("zero-universal-6-face-moved");
        // an edge: along it (its ends' squares and arrows), across (arrows
        // from the centre), the ring and the cube
        h.key(egui::Key::Num2);
        h.click(h.screen_of(Vec3::new(0.0, 20.0 + 6.0, 15.0)));
        h.steps(3);
        h.shot("zero-universal-7-edge");
        let n = h.st.cage().map(|c| c.frame * DVec3::Z).unwrap_or(DVec3::Z);
        let a = arrow_of(&h, n);
        h.move_to(h.screen_of(a.as_vec3()));
        h.steps(2);
        say(&h, "edge's cross arrow hovered");
        h.drag(h.screen_of(a.as_vec3()), h.screen_of((a + n * 4.0).as_vec3()), 12, true);
        h.settle();
        eprintln!("  edge moved across: {:?}", tweaks(&h));
        // a vertex alone: an arrow along each of its edges
        h.key(egui::Key::Num1);
        h.click(h.screen_of(Vec3::new(20.0, 26.0, -15.0)));
        h.steps(3);
        eprintln!("  picked vertices {:?}", h.st.vert_sel);
        let dirs = h.st.slide_dirs();
        eprintln!("  vertex alone: {} edges to slide on {:?}", dirs.len(), dirs.iter().map(|d| d.to_array().map(|v| (v * 100.0).round() / 100.0)).collect::<Vec<_>>());
        h.shot("zero-universal-8-vertex");
        if let Some(dir) = dirs.iter().copied().max_by(|a, b| (-a.x).partial_cmp(&-b.x).unwrap()) {
            let a = arrow_of(&h, dir);
            h.move_to(h.screen_of(a.as_vec3()));
            h.steps(2);
            say(&h, "slide arrow hovered");
            h.drag(h.screen_of(a.as_vec3()), h.screen_of((a + dir * 8.0).as_vec3()), 12, true);
            h.settle();
            eprintln!("  vertex slid along its edge: {:?}", tweaks(&h));
            h.shot("zero-universal-9-vertex-slid");
        }
    }

    if want("side-scale") {
        // Romain 24 nov., 7 h 16: the scale gizmo's side squares (one
        // direction only, the side across staying), Shift's 15° steps on
        // the rotate rings, Import scan in the Documents window
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Block");
        h.key(egui::Key::Num4);
        h.click(h.screen_of(Vec3::new(0.0, 20.0, 0.0)));
        h.key(egui::Key::R);
        h.steps(3);
        let grip = h.screen_of(Vec3::new(20.0, 10.0, 0.0));
        h.move_to(grip);
        h.steps(2);
        eprintln!("  hover +X side: {:?}", h.st.hot_handle);
        h.shot("zero-side-scale-1-grips");
        let to = h.screen_of(Vec3::new(32.0, 10.0, 0.0));
        h.drag(grip, to, 12, true);
        for _ in 0..3 {
            h.settle();
            h.steps(3);
        }
        let f = h.st.doc.feature(b).unwrap();
        let (lo, hi) = f.bounds();
        eprintln!("  after +X side drag: prim {:?}, x {:.2}..{:.2}", f.prim, lo.x, hi.x);
        h.shot("zero-side-scale-2-dragged");
        // rotate with Shift: 15° steps
        h.key(egui::Key::E);
        h.steps(3);
        let pivot = h.st.pivot().unwrap().as_vec3();
        let v = h.st.view();
        let k = v.pixel_size_at((pivot - v.eye).length()) * 78.0;
        let a = h.screen_of(pivot + Vec3::new(1.0, 0.0, 0.0) * k);
        let bq = h.screen_of(pivot + Vec3::new(0.8, 0.0, -0.6) * k);
        h.move_to(a);
        h.steps(2);
        eprintln!("  ring: {:?}", h.st.hot_handle);
        h.hold(Modifiers::SHIFT);
        h.drag(a, bq, 10, true);
        h.hold(Modifiers::NONE);
        h.settle();
        let r = h.st.doc.feature(b).unwrap().rot;
        let (ax, ang) = r.to_axis_angle();
        eprintln!("  rotated with Shift: {:.3}° about {:?}", ang.to_degrees(), ax);
        h.shot("zero-side-scale-3-rotated");
        h.st.files_open = true;
        h.steps(3);
        h.shot("zero-side-scale-4-documents");
    }

    if want("xfreeze") {
        // Romain 24 nov., 12 h 24: "ça ne fonctionne plus sur un simple Cube
        // qui a été scale. On dirait il manque un FreezeTransform". A cube
        // alone stretched by its +X side square: its sizes change, no
        // stretch kept; then a fillet and a 2 mm shell on it, each timed.
        // Then a block with a fillet stretched (kept as a stretch), Freeze
        // Transform. Then Align onto the world and the grid; the Sub
        // Outliner newest first; a cube drawn in the Front view.
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 20.0, y: 20.0, z: 20.0 }, DVec3::new(13.0, 0.0, 7.0), DQuat::IDENTITY);
        h.reset(d, "Cube");
        h.key(egui::Key::Num4);
        h.click(h.screen_of(Vec3::new(13.0, 20.0, 7.0)));
        h.key(egui::Key::R);
        h.steps(3);
        let grip = h.screen_of(Vec3::new(23.0, 10.0, 7.0));
        h.move_to(grip);
        h.steps(2);
        eprintln!("  hover +X side: {:?}", h.st.hot_handle);
        let to = h.screen_of(Vec3::new(43.0, 10.0, 7.0));
        h.drag(grip, to, 12, true);
        for _ in 0..3 {
            h.settle();
            h.steps(3);
        }
        let ob = h.st.doc.object_of(b).and_then(|o| h.st.doc.object(o)).cloned();
        eprintln!("  cube scaled: prim {:?}, stretch kept {:?}", h.st.doc.feature(b).unwrap().prim, ob.map(|o| h.st.doc.is_stretched(o.id)));
        let t0 = std::time::Instant::now();
        let mut d = h.st.doc.clone();
        let (lo, hi) = d.feature(b).unwrap().bounds();
        d.add_round(2.0, false, vec![EdgeRef { faces: [FaceTag { feature: b, sub: 2 }.pack(), FaceTag { feature: b, sub: 4 }.pack()], at: DVec3::new((lo.x + hi.x) * 0.5, hi.y, hi.z) }]);
        h.st.edit("Fillet", |x| *x = d);
        h.settle();
        eprintln!("  fillet 2 mm: {:.0} ms, errors {:?}", t0.elapsed().as_secs_f64() * 1e3, h.st.model.as_ref().map(|m| m.errors.clone()));
        let t0 = std::time::Instant::now();
        let mut d = h.st.doc.clone();
        d.add_shell(nurbs_kernel::Shell { faces: vec![], thickness: 2.0, outward: false }, DVec3::new((lo.x + hi.x) * 0.5, hi.y, 7.0));
        h.st.edit("Shell", |x| *x = d);
        h.settle();
        eprintln!("  shell 2 mm: {:.0} ms, errors {:?}", t0.elapsed().as_secs_f64() * 1e3, h.st.model.as_ref().map(|m| m.errors.clone()));
        h.st.xray = true;
        h.steps(3);
        h.shot("zero-xfreeze-1-cube-scaled-shell");
        h.st.xray = false;
        // a block with a fillet stretched: kept as a stretch, then frozen
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 20.0, y: 20.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add_round(3.0, false, vec![EdgeRef { faces: [FaceTag { feature: b, sub: 2 }.pack(), FaceTag { feature: b, sub: 4 }.pack()], at: DVec3::new(0.0, 20.0, 10.0) }]);
        h.reset(d, "Block");
        h.key(egui::Key::Num4);
        h.click(h.screen_of(Vec3::new(0.0, 20.0, 0.0)));
        h.key(egui::Key::R);
        h.steps(3);
        let grip = h.screen_of(Vec3::new(10.0, 10.0, 0.0));
        h.drag(grip, h.screen_of(Vec3::new(30.0, 10.0, 0.0)), 12, true);
        for _ in 0..3 {
            h.settle();
            h.steps(3);
        }
        let ob = h.st.doc.object_of(b).and_then(|o| h.st.doc.object(o)).cloned();
        eprintln!("  block with a fillet scaled: stretch kept {:?}", ob.as_ref().map(|o| h.st.doc.is_stretched(o.id)));
        h.shot("zero-xfreeze-2-stretched");
        let t0 = std::time::Instant::now();
        h.hold(Modifiers::ALT | Modifiers::SHIFT);
        h.key(egui::Key::F);
        h.hold(Modifiers::NONE);
        h.settle();
        let ob = h.st.doc.object_of(b).and_then(|o| h.st.doc.object(o)).cloned();
        eprintln!("  frozen ({:.0} ms): prim {:?}, stretch {:?}, toast {:?}, errors {:?}", t0.elapsed().as_secs_f64() * 1e3, h.st.doc.feature(b).unwrap().prim, ob.map(|o| h.st.doc.is_stretched(o.id)), h.st.toast.as_ref().map(|t| t.text.clone()), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-xfreeze-3-frozen");
        // Align onto the world, then the grid
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 20.0, y: 10.0, z: 20.0 }, DVec3::new(13.3, 4.2, -6.7), DQuat::IDENTITY);
        h.reset(d, "Block");
        h.key(egui::Key::Num4);
        h.click(h.screen_of(Vec3::new(13.3, 14.2, -6.7)));
        h.st.begin_align();
        h.steps(3);
        h.shot("zero-xfreeze-4-align-open");
        h.st.align_world(nurbs_app::align::WorldTarget::Origin);
        h.steps(3);
        eprintln!("  align World (centre): origin {:?}", h.st.doc.feature(b).unwrap().origin);
        h.shot("zero-xfreeze-5-align-world");
        h.st.align_cancel();
        h.st.begin_align();
        h.st.align_world(nurbs_app::align::WorldTarget::Grid);
        h.st.align_ok();
        h.steps(3);
        eprintln!("  align Grid (centre): origin {:?}", h.st.doc.feature(b).unwrap().origin);
        // the Front view: a cube drawn where there is no face
        let d = Doc::default();
        h.reset(d, "Empty");
        h.st.camera.preset(nurbs_app::camera::ViewPreset::Front);
        h.steps(3);
        h.key(egui::Key::B);
        let a = h.screen_of(Vec3::new(-10.0, 10.0, 0.0));
        let c = h.screen_of(Vec3::new(10.0, 30.0, 0.0));
        h.drag(a, c, 8, true);
        h.steps(2);
        h.move_to(c + egui::vec2(0.0, -40.0));
        h.steps(2);
        h.click(c + egui::vec2(0.0, -40.0));
        h.settle();
        let f = h.st.doc.features.last().cloned();
        eprintln!("  front view cube: {:?}", f.map(|f| (f.prim, f.origin, f.rot * DVec3::Y)));
        h.shot("zero-xfreeze-6-front-cube");
        // a saved scene (FORME_DOC): every stretched Object frozen, timed
        if let Ok(path) = std::env::var("FORME_DOC") {
            let d: Doc = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            h.reset(d, "Scene");
            for _ in 0..4 {
                h.settle();
                h.steps(3);
            }
            eprintln!("  scene: errors {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
            h.shot("zero-xfreeze-7-scene");
            let objs: Vec<u32> = h.st.doc.objects.iter().filter(|o| h.st.doc.is_stretched(o.id)).map(|o| o.id).collect();
            for o in objs {
                h.key(egui::Key::Num4);
                h.st.select_object(o, false);
                let t0 = std::time::Instant::now();
                h.st.freeze_transform();
                h.settle();
                eprintln!("  freeze {o}: {:.0} ms, toast {:?}, errors {:?}", t0.elapsed().as_secs_f64() * 1e3, h.st.toast.as_ref().map(|t| t.text.clone()), h.st.model.as_ref().map(|m| m.errors.clone()));
            }
            h.shot("zero-xfreeze-8-scene-frozen");
        }
    }

    if want("scaled") {
        // Romain 24 nov., 12 h 24: "Tu as l'aprem pour faire des tests de
        // performance, là ça ne fonctionne plus sur un simple Cube qui a
        // été scale". A cube stretched by its side squares (real drags),
        // then modelled: fillets, inset, extrusion, a hole, a move of a
        // face, a shell; its drags and live changes; then the whole Object
        // scaled again (with operations: kept as a stretch), a fillet made
        // on it, Freeze Transform, a fillet again. scaled.txt
        let mut log: Vec<String> = vec![];
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 20.0, y: 20.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        h.reset(d, "Scaled");
        let side = |h: &mut Harness, from: Vec3, to: Vec3| {
            h.key(egui::Key::Escape);
            h.key(egui::Key::Num4);
            let c = h.st.doc.feature(b).unwrap().centre().as_vec3();
            h.click(h.screen_of(c));
            h.key(egui::Key::R);
            h.steps(3);
            let g = h.screen_of(from);
            h.move_to(g);
            h.steps(2);
            let hot = h.st.hot_handle;
            h.drag(g, h.screen_of(to), 12, true);
            h.settle();
            hot
        };
        let hot = side(&mut h, Vec3::new(10.0, 10.0, 0.0), Vec3::new(30.0, 10.0, 0.0));
        let hot2 = side(&mut h, Vec3::new(10.0, 20.0, 0.0), Vec3::new(10.0, 8.0, 0.0));
        let f = h.st.doc.feature(b).unwrap().clone();
        let ob = h.st.doc.object_of(b).and_then(|o| h.st.doc.object(o)).cloned().unwrap();
        let line = format!("cube scaled by {hot:?} then {hot2:?}: {:?} at {:?}, stretch kept {}", f.prim, f.origin, h.st.doc.is_stretched(ob.id));
        eprintln!("  {line}");
        log.push(line);
        let (lo, hi) = f.bounds();
        let c = (lo + hi) * 0.5;
        let top = FaceTag { feature: b, sub: 2 }.pack();
        let e = |a: i64, bb: i64, at: DVec3| EdgeRef { faces: [a, bb], at };
        let s = |k: u16| FaceTag { feature: b, sub: k }.pack();
        timed(&mut h, &mut log, "Fillet 2 on the 4 top edges", |d| {
            d.add_round(2.0, false, vec![e(top, s(0), DVec3::new(hi.x, hi.y, c.z)), e(top, s(1), DVec3::new(lo.x, hi.y, c.z)), e(top, s(4), DVec3::new(c.x, hi.y, hi.z)), e(top, s(5), DVec3::new(c.x, hi.y, lo.z))])
        });
        let inset = timed(&mut h, &mut log, "Inset 4 on the top", |d| d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::doc::FaceRef { tag: top, at: DVec3::new(c.x, hi.y, c.z) }, inset: 4.0, depth: 0.0, keep: false }));
        timed(&mut h, &mut log, "Extrude its inside 3 down", |d| d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::doc::FaceRef { tag: FaceTag { feature: inset, sub: 1 }.pack(), at: DVec3::new(c.x, hi.y, c.z) }, inset: 0.0, depth: -3.0, keep: true }));
        let hole = timed(&mut h, &mut log, "A hole through it", |d| d.add(Op::Carve, Prim::Cylinder { radius: 2.5, height: 20.0 }, DVec3::new(lo.x + 5.0, -2.0, c.z), DQuat::IDENTITY));
        timed(&mut h, &mut log, "Move its +X face 5 out", |d| {
            let mut t = nurbs_kernel::Tweak::new(vec![nurbs_kernel::Comp::Face(nurbs_kernel::doc::FaceRef { tag: s(0), at: DVec3::new(hi.x, (lo.y + hi.y) * 0.5, c.z) })]).unwrap();
            t.delta = DVec3::new(5.0, 0.0, 0.0);
            d.add_tweak(t)
        });
        h.shot("zero-scaled-1-modelled");
        gizmo_drag(&mut h, &mut log, "drag the Object along X", &[b, hole], Vec3::X, "zero-scaled-2-dragged");
        live_change(&mut h, &mut log, "fillet 2 -> 4 live", |d, t| {
            if let Some(r) = d.features.iter_mut().find_map(|f| f.round.as_mut()) {
                r.size = 2.0 + 2.0 * t;
            }
        });
        // which step a closed shell cannot be made after (each left out)
        if std::env::var("FORME_SHELL_TRY").is_ok() {
            let base = h.st.doc.clone();
            let ids: Vec<u32> = base.features.iter().map(|f| f.id).collect();
            for skip in std::iter::once(None).chain(ids.iter().map(|i| Some(*i))) {
                let mut d = base.clone();
                if let Some(sk) = skip {
                    d.features.retain(|f| f.id != sk);
                }
                d.add_shell(nurbs_kernel::Shell { faces: vec![], thickness: 1.5, outward: false }, DVec3::new(c.x, hi.y, c.z));
                h.st.edit("try", |x| *x = d);
                h.settle();
                eprintln!("  shell without {skip:?}: {:?}", h.st.model.as_ref().map(|m| m.errors.clone()));
                h.st.undo();
                h.settle();
            }
        }
        timed(&mut h, &mut log, "Shell 1.5 open at the bottom", |d| d.add_shell(nurbs_kernel::Shell { faces: vec![nurbs_kernel::doc::FaceRef { tag: s(3), at: DVec3::new(c.x + 8.0, lo.y, c.z - 5.0) }], thickness: 1.5, outward: false }, DVec3::new(c.x, hi.y, c.z)));
        // the whole Object scaled again, with its operations: a stretch
        let t0 = Instant::now();
        let (lo2, hi2) = h.st.doc.feature(b).unwrap().bounds();
        let _ = (lo2, hi2);
        let hot3 = side(&mut h, Vec3::new(12.5, 4.0, 10.0), Vec3::new(12.5, 4.0, 25.0));
        let ob = h.st.doc.object_of(b).and_then(|o| h.st.doc.object(o)).cloned().unwrap();
        let line = format!("Object with operations scaled by {hot3:?} along Z: {:.0} ms, stretch kept {}, errors {:?}", t0.elapsed().as_secs_f64() * 1e3, h.st.doc.is_stretched(ob.id), h.st.model.as_ref().map(|m| m.errors.clone()));
        eprintln!("  {line}");
        log.push(line);
        timed(&mut h, &mut log, "Fillet 1 on the stretched Object", |d| d.add_round(1.0, false, vec![e(FaceTag { feature: b, sub: 3 }.pack(), s(0), DVec3::new(hi.x + 5.0, lo.y, c.z))]));
        let toast = h.st.toast.as_ref().map(|t| (t.text.clone(), t.acts.len()));
        eprintln!("  toast {toast:?}");
        log.push(format!("  toast {toast:?}"));
        h.shot("zero-scaled-3-stretched");
        timed_act(&mut h, &mut log, "Freeze Transform", |h| {
            h.st.select_object(ob.id, false);
            h.st.freeze_transform();
        });
        let toast = h.st.toast.as_ref().map(|t| t.text.clone());
        log.push(format!("  toast {toast:?}"));
        eprintln!("  toast {toast:?}");
        h.shot("zero-scaled-4-frozen");
        gizmo_drag(&mut h, &mut log, "drag the frozen Object along Z", &[b, hole], Vec3::Z, "zero-scaled-5-dragged");
        std::fs::write(format!("{}/scaled.txt", h.out), log.join("\n")).ok();
    }

    if want("rescale") {
        // Romain 24 nov., 13 h 09: the Scale made again as a step of its
        // Object. A block with a fillet scaled by its +X side square (a
        // "Scale" step, its faces still flat), Ctrl + Z and Ctrl + Y (the
        // part follows), a Sub cylinder drawn on its scaled top (round),
        // a box drawn on it; Legacy (the shapes take the scale, the fillet
        // keeps its size); a Sub hovered and selected (its volume filled
        // lightly). FORME_DOC: his scene, migrated, a cylinder drawn on it.
        let tg = |f: u32, s: u16| FaceTag { feature: f, sub: s }.pack();
        let bounds = |h: &Harness| -> String {
            let Some(m) = h.st.model.as_ref() else { return "no model".into() };
            let lo = m.positions.iter().fold([f32::MAX; 3], |a, p| [a[0].min(p[0]), a[1].min(p[1]), a[2].min(p[2])]);
            let hi = m.positions.iter().fold([f32::MIN; 3], |a, p| [a[0].max(p[0]), a[1].max(p[1]), a[2].max(p[2])]);
            let r = |v: f32| (v * 10.0).round() / 10.0;
            format!("{:?} to {:?}, {} faces ({} flat), errors {:?}", lo.map(r), hi.map(r), m.faces.len(), m.faces.iter().filter(|f| f.kind == nurbs_kernel::model::SurfKind::Plane).count(), m.errors)
        };
        let steps = |h: &Harness| -> Vec<String> { h.st.doc.features.iter().map(|f| format!("{}{}", f.name, if f.stretch.is_some() { " (scale)" } else { "" })).collect() };
        let round_of = |h: &Harness, id: u32| -> Vec<(nurbs_kernel::model::SurfKind, f64)> {
            h.st.model.as_ref().map(|m| m.faces.iter().filter(|f| f.tag.is_some_and(|t| t.feature == id) && f.kind != nurbs_kernel::model::SurfKind::Plane).map(|f| (f.kind, (f.radius * 1000.0).round() / 1000.0)).collect()).unwrap_or_default()
        };
        let scale_side = |h: &mut Harness, pick: Vec3, from: Vec3, to: Vec3| {
            h.key(egui::Key::Escape);
            h.st.tool = Tool::Select;
            h.key(egui::Key::Num4);
            h.click(h.screen_of(pick));
            h.key(egui::Key::R);
            h.steps(3);
            let g = h.screen_of(from);
            h.move_to(g);
            h.steps(2);
            let hot = h.st.hot_handle;
            h.drag(g, h.screen_of(to), 12, true);
            for _ in 0..3 {
                h.settle();
                h.steps(3);
            }
            hot
        };
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add_round(3.0, false, vec![EdgeRef { faces: [tg(b, 2), tg(b, 4)], at: DVec3::new(0.0, 20.0, 15.0) }]);
        h.reset(d, "Block");
        eprintln!("  before: {}", bounds(&h));
        let hot = scale_side(&mut h, Vec3::new(0.0, 20.0, 0.0), Vec3::new(20.0, 10.0, 0.0), Vec3::new(40.0, 10.0, 0.0));
        eprintln!("  scaled by {hot:?}: steps {:?}, {}", steps(&h), bounds(&h));
        h.shot("zero-rescale-1-scaled");
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::Z);
        h.hold(Modifiers::NONE);
        for _ in 0..3 {
            h.settle();
            h.steps(3);
        }
        eprintln!("  Ctrl + Z: steps {:?}, {}", steps(&h), bounds(&h));
        h.shot("zero-rescale-2-undone");
        h.hold(Modifiers::CTRL);
        h.key(egui::Key::Y);
        h.hold(Modifiers::NONE);
        for _ in 0..3 {
            h.settle();
            h.steps(3);
        }
        eprintln!("  Ctrl + Y: steps {:?}, {}", steps(&h), bounds(&h));
        // a Sub cylinder drawn on the scaled top: round
        h.key(egui::Key::Escape);
        h.st.selection.clear();
        h.st.mode = Op::Carve;
        h.st.tool = Tool::Create(PrimKind::Cylinder);
        let c = h.screen_of(Vec3::new(-15.0, 20.0, -4.0));
        let r = h.screen_of(Vec3::new(-9.0, 20.0, -4.0));
        h.move_to(c);
        h.steps(2);
        h.drag(c, r, 8, true);
        h.steps(2);
        let down = h.screen_of(Vec3::new(-15.0, 8.0, -4.0));
        h.move_to(down);
        h.settle();
        h.shot("zero-rescale-3-cylinder-drawn");
        h.click(down);
        for _ in 0..3 {
            h.settle();
            h.steps(3);
        }
        let cyl = h.st.doc.features.last().unwrap().clone();
        eprintln!("  cylinder drawn: {:?} at {:?}, steps {:?}, its faces {:?}, {}", cyl.prim, cyl.origin, steps(&h), round_of(&h, cyl.id), bounds(&h));
        // a box drawn on the scaled top, Add
        h.st.mode = Op::Add;
        h.st.tool = Tool::Create(PrimKind::Box);
        let a = h.screen_of(Vec3::new(5.0, 20.0, -8.0));
        let e = h.screen_of(Vec3::new(20.0, 20.0, 2.0));
        h.move_to(a);
        h.steps(2);
        h.drag(a, e, 10, true);
        h.steps(2);
        let up = h.screen_of(Vec3::new(12.0, 28.0, -3.0));
        h.move_to(up);
        h.settle();
        h.click(up);
        for _ in 0..3 {
            h.settle();
            h.steps(3);
        }
        let bx = h.st.doc.features.last().unwrap().clone();
        eprintln!("  box drawn: {:?} at {:?}, steps {:?}, {}", bx.prim, bx.origin, steps(&h), bounds(&h));
        h.st.tool = Tool::Select;
        h.st.selection.clear();
        h.steps(3);
        h.shot("zero-rescale-4-drawn-on-it");
        // the Sub hovered then selected: its volume filled lightly
        h.key(egui::Key::Num4);
        // the hole's far wall, seen through its mouth
        let eye = h.st.view().eye.as_dvec3();
        let c0 = cyl.origin;
        let away = DVec3::new(c0.x - eye.x, 0.0, c0.z - eye.z).normalize() * 5.0;
        let wall = c0 + away + DVec3::new(0.0, -3.0, 0.0);
        h.move_to(h.screen_of(wall.as_vec3() + Vec3::new(0.0, 0.0, 0.0)));
        h.steps(2);
        h.move_to(h.screen_of(wall.as_vec3()));
        h.steps(3);
        eprintln!("  hover at {wall:?}: {:?} also {:?} (the Sub {})", h.st.hover_feature, h.st.hover_also, cyl.id);
        h.shot("zero-rescale-5-sub-hovered");
        h.st.selection = vec![cyl.id];
        h.steps(3);
        h.shot("zero-rescale-6-sub-selected");
        // Legacy: the shapes take the scale, the fillet keeps its size
        h.st.settings.scale_legacy = true;
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add_round(3.0, false, vec![EdgeRef { faces: [tg(b, 2), tg(b, 4)], at: DVec3::new(0.0, 20.0, 15.0) }]);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-8.0, -5.0, 0.0), DQuat::IDENTITY);
        d.add_push(nurbs_kernel::Push { face: nurbs_kernel::FaceRef { tag: tg(b, 2), at: DVec3::new(10.0, 20.0, -5.0) }, inset: 2.0, depth: -1.0, keep: false });
        h.reset(d, "Legacy");
        h.st.settings.scale_legacy = true;
        let hot = scale_side(&mut h, Vec3::new(0.0, 20.0, 0.0), Vec3::new(20.0, 10.0, 0.0), Vec3::new(40.0, 10.0, 0.0));
        for f in &h.st.doc.features {
            eprintln!("  legacy {hot:?}: {} {:?} at {:?} round {:?} push {:?}", f.name, f.prim, f.origin, f.round.as_ref().map(|r| r.size), f.push.as_ref().map(|p| (p.inset, p.depth)));
        }
        eprintln!("  legacy: {}", bounds(&h));
        h.shot("zero-rescale-7-legacy");
        h.st.settings.scale_legacy = false;
        // his scene (FORME_DOC), migrated: a Sub cylinder drawn on its Cube
        if let Ok(path) = std::env::var("FORME_DOC") {
            let d: Doc = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            h.reset(d, "Scene");
            for _ in 0..3 {
                h.settle();
                h.steps(3);
            }
            eprintln!("  scene: steps {:?}, {}", steps(&h), bounds(&h));
            h.shot("zero-rescale-8-scene");
            // the Cube's largest flat face seen from the eye
            let eye = h.st.view().eye.as_dvec3();
            let face = h.st.model.as_ref().and_then(|m| {
                m.faces
                    .iter()
                    .filter(|f| f.kind == nurbs_kernel::model::SurfKind::Plane && f.tag.is_some_and(|t| t.feature == 1))
                    .map(|f| (DVec3::from(f.point), DVec3::from(f.normal)))
                    .filter(|(p, n)| n.dot(eye - *p) > 0.0)
                    .max_by(|x, y| x.1.dot((eye - x.0).normalize()).total_cmp(&y.1.dot((eye - y.0).normalize())))
            });
            if let Some((p, n)) = face {
                let t = n.any_orthonormal_vector();
                h.st.mode = Op::Carve;
                h.st.tool = Tool::Create(PrimKind::Cylinder);
                let c = h.screen_of(p.as_vec3());
                let r = h.screen_of((p + t * 12.0).as_vec3());
                h.move_to(c);
                h.steps(2);
                h.drag(c, r, 8, true);
                h.steps(2);
                let down = h.screen_of((p - n * 20.0).as_vec3());
                h.move_to(down);
                h.settle();
                h.click(down);
                for _ in 0..3 {
                    h.settle();
                    h.steps(3);
                }
                let cyl = h.st.doc.features.last().unwrap().clone();
                eprintln!("  scene: cylinder drawn on {p:?} {n:?}: {:?} at {:?}, its faces {:?}, steps {:?}, {}", cyl.prim, cyl.origin, round_of(&h, cyl.id), steps(&h), bounds(&h));
                h.st.tool = Tool::Select;
                h.st.selection = vec![cyl.id];
                h.steps(3);
                h.shot("zero-rescale-9-scene-cylinder");
            }
        }
    }

    if want("project") {
        // Romain 24 nov., 19 h 03: the keys 1 2 3 on his AZERTY keyboard
        // (& é " ') give Vertex, Edge, Face (4 Object); a curve's line in
        // the fillets' yellow and its points three times bigger; Project
        // curve (Maya's Project Curve on Mesh): an open curve drawn on the
        // block's top, drawn on its faces, the piece it parts off extruded;
        // a closed one cutting through, then keeping its inside; the open
        // one cutting a side away. FORME_DOC: his scene, Path 4 projected.
        let mode = |h: &Harness| format!("{:?}", h.st.sel_mode);
        let info = |h: &Harness| -> String {
            let Some(m) = h.st.model.as_ref() else { return "no model".into() };
            format!("{} faces, {} edges, errors {:?}", m.faces.len(), m.edges.len(), m.errors)
        };
        h.reset(test_part(), "Project");
        h.st.camera.frame(Vec3::new(-25.0, 0.0, -20.0), Vec3::new(25.0, 30.0, 20.0));
        h.st.camera.target = Vec3::new(-5.0, 14.0, 4.0);
        h.st.camera.dist *= 0.62;
        h.steps(2);
        for (key, phys, text) in [
            (egui::Key::Num1, egui::Key::Num1, "&"),
            (egui::Key::Num2, egui::Key::Num2, "é"),
            (egui::Key::Num3, egui::Key::Num3, "\""),
            (egui::Key::Quote, egui::Key::Num4, "'"),
            (egui::Key::Num1, egui::Key::Num1, "1"),
        ] {
            h.key_phys(key, phys, text);
            eprintln!("  key {text:?} (physical {phys:?}): {}", mode(&h));
        }
        h.key_phys(egui::Key::Quote, egui::Key::Num4, "'");
        // an open curve on the top: from the −X edge to the slot's edge,
        // in front of the hole
        let t = |h: &Harness, x: f32, z: f32| h.screen_of(Vec3::new(x, 20.0, z));
        h.key(egui::Key::P);
        for (x, z) in [(-20.0, 7.0), (-14.0, 11.0), (-8.0, 9.0)] {
            let q = t(&h, x, z);
            h.move_to(q);
            h.steps(1);
            h.click(q);
        }
        let q = t(&h, -4.0, 11.0);
        h.move_to(q);
        h.steps(2);
        h.shot("zero-project-1-drawing");
        h.click(q);
        h.key(egui::Key::Enter);
        h.settle();
        let open = h.st.doc.features.last().unwrap().clone();
        eprintln!("  open curve {} {:?} points {:?}", open.name, open.prim, open.profile.as_ref().map(|p| p.points.len()));
        // Vertex mode: its points
        h.key_phys(egui::Key::Num1, egui::Key::Num1, "&");
        h.st.selection.clear();
        h.move_to(t(&h, -14.0, 11.0));
        h.steps(3);
        eprintln!("  {}: curve points {}", mode(&h), h.st.curve_points().len());
        h.shot("zero-project-2-points");
        // Object mode, the curve selected: its window's Project section
        h.key_phys(egui::Key::Quote, egui::Key::Num4, "'");
        h.click(t(&h, -14.0, 11.0));
        h.steps(3);
        eprintln!("  selected {:?}", h.st.selection);
        h.shot("zero-project-3-curve-window");
        let pid = h.st.add_project(open.id, false);
        h.settle();
        h.steps(2);
        eprintln!("  projected (draw) {pid:?}: {} ; toast {:?}", info(&h), h.st.toast.as_ref().map(|t| t.text.clone()));
        h.shot("zero-project-4-drawn-on-faces");
        // Face mode: the piece in front of the curve picked, extruded by
        // Shift + drag
        h.key_phys(egui::Key::Num3, egui::Key::Num3, "\"");
        h.st.selection.clear();
        let piece = t(&h, -12.0, 13.5);
        h.click(piece);
        h.steps(2);
        eprintln!("  {}: picked {:?}", mode(&h), h.st.face_sel);
        h.shot("zero-project-5-piece-picked");
        h.hold(Modifiers::SHIFT);
        h.drag(piece, piece - egui::vec2(0.0, 60.0), 12, true);
        h.hold(Modifiers::NONE);
        h.settle();
        h.steps(2);
        let push = h.st.doc.features.last().unwrap().clone();
        eprintln!("  extruded: {} {:?}: {}", push.name, push.push.as_ref().map(|p| (p.inset, p.depth)), info(&h));
        h.key_phys(egui::Key::Quote, egui::Key::Num4, "'");
        h.st.selection.clear();
        h.move_to(pos2(560.0, 820.0));
        h.steps(2);
        eprintln!("  state: xray {} hover {:?} also {:?} sel {:?} tool {:?} push {} drag {}", h.st.xray, h.st.hover_feature, h.st.hover_also, h.st.selection, h.st.tool, h.st.push.is_some(), h.st.drag.is_some());
        h.shot("zero-project-6-piece-extruded");
        // a closed curve on the top in front of the boss, cutting through
        h.key(egui::Key::P);
        let first = t(&h, 6.0, 9.0);
        for (x, z) in [(6.0, 9.0), (17.0, 9.0), (17.0, 13.5), (6.0, 13.5)] {
            let q = t(&h, x, z);
            h.move_to(q);
            h.steps(1);
            h.click(q);
        }
        h.move_to(first);
        h.steps(1);
        h.click(first);
        h.settle();
        let closed = h.st.doc.features.last().unwrap().clone();
        eprintln!("  closed curve {} open {:?}", closed.name, closed.profile.as_ref().map(|p| p.open));
        let cid = h.st.add_project(closed.id, true);
        h.settle();
        h.steps(2);
        eprintln!("  cut through {cid:?}: {}", info(&h));
        h.shot("zero-project-7-closed-cut");
        if let Some(cid) = cid {
            let p = h.st.doc.feature(cid).and_then(|f| f.project.clone()).unwrap();
            h.st.set_project(cid, nurbs_kernel::Project { flip: true, ..p });
            h.settle();
            h.steps(2);
            eprintln!("  inside kept: {}", info(&h));
            h.shot("zero-project-8-inside-kept");
            h.hold(Modifiers::CTRL);
            h.key(egui::Key::Z);
            h.hold(Modifiers::NONE);
            h.settle();
        }
        // the open curve's projection made a cut: a side taken away
        if let Some(pid) = pid {
            let p = h.st.doc.feature(pid).and_then(|f| f.project.clone()).unwrap();
            h.st.set_project(pid, nurbs_kernel::Project { cut: true, ..p.clone() });
            h.st.selection = vec![pid];
            h.settle();
            h.steps(2);
            eprintln!("  open curve cuts: {} names {:?}", info(&h), h.st.doc.feature(pid).map(|f| f.name.clone()));
            h.shot("zero-project-9-open-cut");
            h.st.set_project(pid, nurbs_kernel::Project { cut: true, flip: true, ..p });
            h.settle();
            h.steps(2);
            eprintln!("  the other side: {}", info(&h));
            h.shot("zero-project-10-open-cut-flipped");
        }
        // his scene: Path 4 drawn on the Cube, then cutting it
        if let Ok(path) = std::env::var("FORME_DOC") {
            let d: Doc = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            h.reset(d, "Scene");
            for _ in 0..3 {
                h.settle();
                h.steps(3);
            }
            let name = std::env::var("FORME_CURVE").unwrap_or_else(|_| "Path 4".into());
            let curve = h.st.doc.features.iter().find(|f| f.name == name).map(|f| f.id);
            eprintln!("  scene: {} ; {name} = {curve:?}", info(&h));
            if let Some(c) = curve {
                h.st.selection = vec![c];
                h.steps(3);
                h.shot("zero-project-11-scene-curve");
                let pid = h.st.add_project(c, false);
                h.settle();
                h.steps(2);
                eprintln!("  scene drawn {pid:?}: {}", info(&h));
                h.st.selection.clear();
                h.steps(2);
                h.shot("zero-project-12-scene-drawn");
                if let Some(pid) = pid {
                    h.st.selection = vec![pid];
                    let p = h.st.doc.feature(pid).and_then(|f| f.project.clone()).unwrap();
                    h.st.set_project(pid, nurbs_kernel::Project { cut: true, ..p });
                    h.settle();
                    h.steps(2);
                    eprintln!("  scene cut: {}", info(&h));
                    h.shot("zero-project-13-scene-cut");
                }
            }
        }
    }

    if want("sym-project") {
        // Romain 25 nov., 6 h 17: "Sur la scène ouverte j'ai cut avec la
        // spline mais ça ne prend pas le miroir / En miroir un point au
        // milieu de la coupe doit fusionner, actuellement ça casse la
        // symétrie / … la touche petit 2 … : faut bien object mode / Quand
        // on direct fillet un point cacher le Gizmo". A block with its own
        // symmetry across its Z, an open curve in front of it drawn on its
        // faces: the drawing on its back too; its symmetry across X: the
        // curve and its image crossing on the plane; a corner pulled to the
        // middle of its edge with its image: one vertex; a point of the
        // drawing pulled out, both sides. FORME_DOC: his scene (its
        // Project 2 and 3, the same drawing again, left out).
        let mode = |h: &Harness| format!("{:?}", h.st.sel_mode);
        let info = |h: &Harness| -> String {
            let Some(m) = h.st.model.as_ref() else { return "no model".into() };
            format!("{} faces, {} edges, errors {:?}", m.faces.len(), m.edges.len(), m.errors)
        };
        let doc: Doc = match std::env::var("FORME_DOC") {
            Ok(path) => serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap(),
            Err(_) => serde_json::from_str(r#"{"version":1,"next_id":22,"features":[
                {"id":12,"name":"Cube","op":"Add","prim":{"kind":"Box","x":119.6554,"y":203.6433,"z":32.6572},"origin":[-6.0825,0.0,16.90387],"rot":[0,0,0,1],"visible":true,"material":0,"sym":{"axis":2}},
                {"id":16,"name":"Path","op":"Add","prim":{"kind":"Path","x":137.9733,"z":116.5667},"origin":[11.9721,41.6156,40.2026],"rot":[0.7071067811865475,0,0,0.7071067811865475],"visible":true,"material":0,"profile":{"points":[[0.5,-0.5],[0.0559,-0.3447],[-0.3487,0.2154],[-0.5,0.5]],"smooth":[false,false,false,false],"corner":[0.0,57.1,56.0,0.0],"open":true}},
                {"id":21,"name":"Project","op":"Add","prim":{"kind":"Box","x":0,"y":0,"z":0},"origin":[8.47,45.84,40.2],"rot":[0,0,0,1],"visible":true,"material":0,"project":{"curve":16,"cut":false,"flip":false,"shapes":[12]}}]}"#).unwrap(),
        };
        let mut doc = doc;
        doc.features.retain(|f| f.id != 22 && f.id != 23);
        h.reset(doc, "Scene");
        for _ in 0..3 {
            h.settle();
            h.steps(3);
        }
        eprintln!("  sym Z: {}", info(&h));
        let look = |h: &mut Harness, yaw: f32, pitch: f32, dist: f32| {
            h.st.camera.yaw = yaw;
            h.st.camera.pitch = pitch;
            h.st.camera.target = Vec3::new(-6.0, 100.0, 17.0);
            h.st.camera.dist = dist;
            h.steps(3);
        };
        look(&mut h, 0.55, 0.25, 520.0);
        h.shot("zero-symproj-1-front");
        look(&mut h, 0.55 + std::f32::consts::PI, 0.25, 520.0);
        h.shot("zero-symproj-2-back-mirrored");
        // its symmetry across its X instead: the curve's image crosses it
        // on the plane, one point there
        let cube = h.st.doc.features.iter().find(|f| f.sym.is_some()).map(|f| f.id);
        if let Some(c) = cube {
            h.st.edit("Symmetry", |d| d.feature_mut(c).unwrap().sym.as_mut().unwrap().axis = 0);
            h.settle();
            h.steps(3);
            eprintln!("  sym X: {}", info(&h));
            look(&mut h, 0.0, 0.0, 480.0);
            h.shot("zero-symproj-3-sym-x-crossing");
            h.st.edit("Symmetry", |d| d.feature_mut(c).unwrap().sym.as_mut().unwrap().axis = 2);
            h.settle();
            h.steps(3);
        }
        // the corner pulled down to the middle of its edge (the plane): it
        // and its image one vertex
        let at = h.st.doc.feature(cube.unwrap_or(12)).map(|f| {
            let (lo, hi) = f.bounds();
            (DVec3::new(hi.x, lo.y, hi.z), 0.5 * (lo.z + hi.z) - hi.z)
        });
        if let Some((corner, dz)) = at {
            look(&mut h, 1.15, 0.12, 380.0);
            h.st.camera.target = Vec3::new(corner.x as f32, 40.0, 17.0);
            h.steps(3);
            h.shot("zero-symproj-4-corner-before");
            let mut t = nurbs_kernel::Tweak::new(vec![nurbs_kernel::Comp::Vertex(corner)]).unwrap();
            t.delta = DVec3::new(0.0, 0.0, dz);
            h.st.edit("Move vertex", |d| {
                d.add_tweak(t);
            });
            h.settle();
            h.steps(3);
            let near = h.st.model.as_ref().map(|m| {
                let mut v: Vec<[f32; 3]> = m.edges.iter().flat_map(|e| [e.pts.first().copied(), e.pts.last().copied()]).flatten().filter(|p| (p[0] - corner.x as f32).abs() < 0.05 && p[1].abs() < 0.05).collect();
                v.sort_by(|a, b| a[2].total_cmp(&b[2]));
                v.dedup_by(|a, b| (a[2] - b[2]).abs() < 1e-3);
                v
            });
            eprintln!("  corner moved {dz:.3}: {} ; vertices left on that edge {near:?}", info(&h));
            h.key_phys(egui::Key::Num1, egui::Key::Num1, "&");
            h.steps(2);
            h.shot("zero-symproj-5-corner-merged");
            h.key_phys(egui::Key::Quote, egui::Key::Num4, "'");
            // a point of the drawing (where it meets the block's side)
            // pulled out of the face: both sides (the corner put back first)
            h.st.undo_doc();
            h.settle();
            h.steps(2);
            let side = h.st.model.as_ref().and_then(|m| {
                let same = |e: &nurbs_kernel::model::Edge| e.faces[0] >= 0 && e.faces[1] >= 0 && m.faces[e.faces[0] as usize].tag == m.faces[e.faces[1] as usize].tag;
                m.edges.iter().filter(|e| same(e)).flat_map(|e| [e.pts.first().copied(), e.pts.last().copied()]).flatten().filter(|p| (p[0] - corner.x as f32).abs() < 0.05 && p[1] > 1.0).max_by(|a, b| a[2].total_cmp(&b[2]))
            });
            if let Some(p) = side {
                let mut t = nurbs_kernel::Tweak::new(vec![nurbs_kernel::Comp::Vertex(DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64))]).unwrap();
                t.delta = DVec3::new(0.0, 0.0, 14.7);
                h.st.edit("Move vertex", |d| {
                    d.add_tweak(t);
                });
                h.settle();
                h.steps(3);
                eprintln!("  point {p:?} pulled 14.7 out: {}", info(&h));
                look(&mut h, 1.0, 0.3, 560.0);
                h.shot("zero-symproj-6-point-pulled");
            }
        }
        // the key left of 1 (AZERTY's ²) gives Object mode
        h.key_phys(egui::Key::Num2, egui::Key::Num2, "é");
        eprintln!("  é: {}", mode(&h));
        h.key_phys(egui::Key::Backtick, egui::Key::Backtick, "²");
        eprintln!("  ²: {}", mode(&h));
        // a direct fillet on a curve's corner: no gizmo while it is dragged
        // (another point of the curve picked, its gizmo shown, then the
        // corner dragged)
        let corner = h.st.doc.features.iter().filter(|f| f.is_path() && f.profile.as_ref().is_some_and(|p| p.open && p.points.len() >= 4 && p.corner.iter().any(|c| *c > 0.0))).min_by(|a, b| a.rot.angle_between(DQuat::IDENTITY).total_cmp(&b.rot.angle_between(DQuat::IDENTITY))).cloned();
        if let Some(c) = corner {
            let pr = c.profile.clone().unwrap();
            let i = (1..pr.points.len() - 1).max_by(|a, b| pr.corner[*a].abs().total_cmp(&pr.corner[*b].abs())).unwrap_or(1);
            let w = nurbs_app::surfaces::path_point_pos(&c, i).unwrap_or(c.origin);
            let other = nurbs_app::surfaces::path_point_pos(&c, 0).unwrap_or(c.origin);
            look(&mut h, 0.2, 1.0, 420.0);
            h.st.camera.target = w.as_vec3();
            h.steps(3);
            h.key_phys(egui::Key::Num1, egui::Key::Num1, "&");
            h.key(egui::Key::W);
            h.steps(2);
            let q = h.screen_of(w.as_vec3());
            h.click(h.screen_of(other.as_vec3()));
            h.steps(2);
            eprintln!("  {} picked {:?} (gizmo {:?})", c.name, h.st.vert_sel, h.st.gizmo);
            h.shot("zero-symproj-7-corner-picked");
            h.drag(q, q + egui::vec2(40.0, 30.0), 10, false);
            h.steps(2);
            eprintln!("  fillet dragged: round drag {}", h.st.round_drag.is_some());
            h.shot("zero-symproj-8-corner-fillet-no-gizmo");
            h.press(q + egui::vec2(40.0, 30.0), false);
            h.settle();
            h.steps(2);
            eprintln!("  after: {}", info(&h));
        }
    }

    if want("intersect") {
        // Boolean Intersection and CARVE named SUB (Romain 24 nov., 5 h 45:
        // "Il y avait pas un mode Boolean Intersection? Appelle SUB à la
        // place de CARVE"): a block, a cylinder across it made Intersect;
        // the top bar's three modes; then History off turned on with a block
        // already inset: "Fold them" folds it
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let cy = d.add(Op::Add, Prim::Cylinder { radius: 12.0, height: 60.0 }, DVec3::new(-30.0, 10.0, 0.0), DQuat::from_rotation_z(-std::f64::consts::FRAC_PI_2));
        h.reset(d, "Block");
        h.shot("zero-intersect-1-added");
        h.key(egui::Key::Num4);
        h.st.selection = vec![cy];
        h.steps(2);
        // the top bar's INTERSECT segment, as a click would
        h.st.set_ops(&[cy], Op::Intersect);
        for _ in 0..4 {
            h.settle();
            h.steps(3);
        }
        let m = h.st.model.as_ref();
        eprintln!("  intersect: faces {:?}, errors {:?}", m.map(|m| m.faces.len()), m.map(|m| m.errors.clone()));
        h.shot("zero-intersect-2-intersect");
        h.st.set_ops(&[cy], Op::Carve);
        for _ in 0..4 {
            h.settle();
            h.steps(3);
        }
        h.shot("zero-intersect-3-sub");
        // History off turned on with an inset made before
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::doc::FaceRef { tag: FaceTag { feature: b, sub: 2 }.pack(), at: DVec3::new(0.0, 20.0, 0.0) }, inset: 3.0, depth: -4.0, keep: false });
        h.reset(d, "Block");
        for _ in 0..3 {
            h.settle();
            h.steps(3);
        }
        h.st.settings.history_off = true;
        eprintln!("  foldable: {:?}", h.st.foldable_elements());
        h.st.fold_all_history();
        for _ in 0..6 {
            h.settle();
            h.steps(3);
        }
        eprintln!("  folded: {:?}, toast {:?}", h.st.doc.features.iter().map(|f| format!("{}{}", f.name, if f.frozen.is_some() { " (one solid)" } else { "" })).collect::<Vec<_>>(), h.st.toast.as_ref().map(|t| t.text.clone()));
        h.shot("zero-intersect-4-folded");
    }

    if want("normal") {
        // The move along the picked components' normals (Maya's Axis
        // Orientation Normal; Romain 23 nov., 22 h 05: "un transformTool
        // comme dans Maya qui déplace une sélection par rapport à ces
        // normal"; 24 nov., 8 h 34: as Maya's Mesh > Transform, T): a face of a block turned 30°, its gizmo along
        // its normal, dragged; then three faces dragged along N, each along
        // its own
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::from_rotation_y(0.5) * DQuat::from_rotation_z(0.3));
        h.reset(d, "Block");
        let tweaks = |h: &Harness| h.st.doc.features.iter().filter_map(|f| f.tweak.as_ref().map(|t| (f.name.clone(), [t.delta.x, t.delta.y, t.delta.z].map(|v| (v * 100.0).round() / 100.0)))).collect::<Vec<_>>();
        let face_at = |h: &Harness, local: DVec3| -> Pos2 {
            let f = &h.st.doc.features[0];
            h.screen_of((f.origin + f.rot * local).as_vec3())
        };
        h.key(egui::Key::F11);
        h.click(face_at(&h, DVec3::new(-8.0, 20.0, 0.0)));
        h.key(egui::Key::T);
        h.steps(3);
        h.shot("zero-normal-1-gizmo");
        let drag_n = |h: &mut Harness, mm: f32, shot: &str| {
            let Some(pivot) = h.st.pivot() else { return };
            let pivot = pivot.as_vec3();
            let axis = h.st.gizmo_rot().as_quat() * Vec3::Y;
            let v = h.st.view();
            let k = v.pixel_size_at((pivot - v.eye).length()) * 78.0;
            let a = h.screen_of(pivot + axis * k * 0.8);
            let b = h.screen_of(pivot + axis * (k * 0.8 + mm));
            h.move_to(a);
            h.steps(2);
            eprintln!("  hot {:?}", h.st.hot_handle);
            h.drag(a, b, 10, false);
            h.steps(2);
            h.shot(&format!("{shot}-drag"));
            h.press(b, false);
            h.settle();
            h.steps(3);
        };
        drag_n(&mut h, 6.0, "zero-normal-2");
        eprintln!("  one face along its normal: {:?}, errors {:?}", tweaks(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-normal-3-moved");
        h.st.undo();
        h.settle();
        h.key(egui::Key::Escape);
        h.key(egui::Key::F11);
        h.click(face_at(&h, DVec3::new(20.0, 10.0, 0.0)));
        h.hold(Modifiers::SHIFT);
        h.click(face_at(&h, DVec3::new(0.0, 20.0, 0.0)));
        h.click(face_at(&h, DVec3::new(0.0, 10.0, 15.0)));
        h.hold(Modifiers::NONE);
        h.key(egui::Key::T);
        h.steps(3);
        eprintln!("  picked {}", h.st.face_sel.len());
        h.shot("zero-normal-4-three");
        drag_n(&mut h, 5.0, "zero-normal-5");
        eprintln!("  three faces along N: {:?}, errors {:?}", tweaks(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-normal-6-moved");
    }

    if want("open-cached") {
        // A long scene opened again (Romain 23 nov., 22 h 05: "Quand
        // j'ouvre la scene très longue à tout rebuild"): the kernel's model
        // of the last time shown at once (`show_saved_model`), the kernel
        // building meanwhile. FORME_DOC = a scene (else the test part);
        // FORME_DOCS sets where the cache goes.
        let d = match std::env::var("FORME_DOC") {
            Ok(p) => Doc::from_json(&std::fs::read_to_string(p).unwrap()).unwrap(),
            Err(_) => test_part(),
        };
        h.reset(d.clone(), "Scene");
        h.st.keep_model();
        std::thread::sleep(Duration::from_millis(1500));
        // the app started again on it
        let t0 = Instant::now();
        h.st = AppState::new(d, "Scene", Kernel::start(kernel_exe()));
        h.st.show_saved_model();
        h.st.frame_all();
        h.steps(2);
        eprintln!("  shown after {:.0} ms: {} faces", t0.elapsed().as_secs_f64() * 1e3, h.st.model.as_ref().map_or(0, |m| m.faces.len()));
        h.shot("zero-open-1-at-once");
        h.settle();
        eprintln!("  built after {:.1} s: {} faces", t0.elapsed().as_secs_f64(), h.st.model.as_ref().map_or(0, |m| m.faces.len()));
        h.shot("zero-open-2-built");
    }

    if want("extrude-live") {
        // An Extrude of several faces dragged by its arrow (Romain 23 nov.,
        // 22 h 05: "je fait une extrude sur 4 face ici et ça lag"): the
        // ends drawn moved at once and the walls stretched while the kernel
        // works (`push_preview`), the kernel making them in one boolean
        // (`push_batch`). FORME_DOC = a scene, FORME_PUSHES = its ids (else
        // the extrusions at its end); else a block with four extruded faces.
        let d = match std::env::var("FORME_DOC") {
            Ok(p) => Doc::from_json(&std::fs::read_to_string(p).unwrap()).unwrap(),
            Err(_) => {
                let mut d = Doc::default();
                let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
                for (sub, at) in [(0u16, DVec3::new(20.0, 10.0, 0.0)), (1, DVec3::new(-20.0, 10.0, 0.0)), (2, DVec3::new(0.0, 20.0, 0.0)), (4, DVec3::new(0.0, 10.0, 15.0))] {
                    let face = nurbs_kernel::FaceRef { tag: FaceTag { feature: b, sub }.pack(), at };
                    d.add_push(nurbs_kernel::Push { face, inset: 3.0, depth: 4.0, keep: false });
                }
                d
            }
        };
        let t0 = Instant::now();
        h.reset(d, "Extrudes");
        h.settle();
        eprintln!("  built in {:.2} s", t0.elapsed().as_secs_f64());
        let ids: Vec<u32> = std::env::var("FORME_PUSHES").ok().map(|v| v.split(',').filter_map(|w| w.parse().ok()).collect()).unwrap_or_else(|| {
            let fs = &h.st.doc.features;
            let k = fs.iter().rposition(|f| f.push.as_ref().is_none_or(|p| p.depth.abs() < 1e-6)).map_or(0, |k| k + 1);
            fs[k..].iter().map(|f| f.id).collect()
        });
        h.key(egui::Key::F11);
        h.st.selection = ids.clone();
        h.key(egui::Key::F);
        h.settle();
        h.st.camera.dist *= 1.8;
        h.st.camera.yaw += 0.5;
        h.steps(3);
        if let Some(hs) = h.st.push_handles() {
        h.shot("zero-extrude-live-1-selected");
        let grab = hs.base + (hs.tip - hs.base) * 0.6;
        let (a, b) = (h.screen_of(grab.as_vec3()), h.screen_of((grab + hs.n * 6.0).as_vec3()));
        h.move_to(a);
        h.steps(2);
        h.press(a, true);
        let mut seen = h.st.model_gen;
        let (mut answers, mut previewed, mut worst) = (0, 0, 0.0f64);
        let n = 40;
        for i in 1..=n {
            h.pending.push(Event::PointerMoved(a + (b - a) * (i as f32 / n as f32)));
            let t = h.frame_timed();
            worst = worst.max(t.iter().sum());
            if h.st.push_preview().is_some() {
                previewed += 1;
            }
            if h.st.model_gen != seen {
                seen = h.st.model_gen;
                answers += 1;
            }
            if i == 20 {
                eprintln!("  frame 20: preview {}", h.st.push_preview().is_some());
                h.shot("zero-extrude-live-2-drag");
            }
            std::thread::sleep(Duration::from_millis(16));
        }
        // held still at the end: drawn from the preview at once, then the
        // kernel's model of the same depth
        h.steps(1);
        eprintln!("  held: preview {}", h.st.push_preview().is_some());
        h.shot("zero-extrude-live-3-held-preview");
        for _ in 0..400 {
            if h.st.push_preview().is_none() {
                break;
            }
            h.steps(1);
            std::thread::sleep(Duration::from_millis(10));
        }
        h.steps(2);
        h.shot("zero-extrude-live-4-held-built");
        h.press(b, false);
        eprintln!("  {n} frames: {previewed} drawn from the preview, {answers} kernel answers, worst frame {worst:.1} ms");
        h.settle();
        h.steps(3);
        let depths: Vec<(String, f64)> = ids.iter().filter_map(|id| h.st.doc.feature(*id).and_then(|f| Some((f.name.clone(), (f.push.as_ref()?.depth * 100.0).round() / 100.0)))).collect();
        eprintln!("  depths {depths:?}, errors {:?}", h.st.model.as_ref().map(|m| m.errors.len()));
        h.shot("zero-extrude-live-5-done");
        } else {
            eprintln!("  no handles for {ids:?}");
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
        h.steps(40);
        // off the gizmo sitting where the face was clicked (Ctrl on it moves
        // it without snapping)
        let front = h.screen_of(Vec3::new(-8.0, 5.0, 15.0));
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
        // (on the gizmo's X arrow, which lies along the edge: Shift still
        // pulls the edge; -14 is under the Edge window now)
        let e = h.screen_of(Vec3::new(8.0, 20.0, 15.0));
        h.click(e);
        // past the double-click time (a press there picked the edge's loop)
        h.steps(40);
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
        let p1 = d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 1, sub: 2 }.pack(), at: DVec3::new(-15.0, 20.0, 10.0) }, inset: 2.0, depth: -2.0, keep: false });
        let p2 = d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 1, sub: 2 }.pack(), at: DVec3::new(15.0, 20.0, 10.0) }, inset: 2.0, depth: 1.5, keep: false });
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
        d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 1, sub: 4 }.pack(), at: DVec3::new(-12.0, 8.0, 15.0) }, inset: 2.0, depth: -1.0, keep: false });
        d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 3, sub: 1 }.pack(), at: DVec3::new(10.0, 30.0, 0.0) }, inset: 2.0, depth: 2.0, keep: false });
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

    if want("freeze") {
        // Delete History (Alt + Shift + D) on an element (FORME_DOC: a saved
        // scene, FORME_FREEZE: its id; else the test part's Block with a
        // fillet and an inset): frozen into one solid, moved, undone
        let (d, id) = match std::env::var("FORME_DOC") {
            Ok(path) => (nurbs_kernel::Doc::from_json(&std::fs::read_to_string(&path).expect("FORME_DOC")).expect("a document"), std::env::var("FORME_FREEZE").ok().and_then(|v| v.parse().ok()).unwrap_or(1)),
            Err(_) => {
                let mut d = test_part();
                d.add_round(2.0, false, vec![EdgeRef { faces: [FaceTag { feature: 1, sub: 2 }.pack(), FaceTag { feature: 1, sub: 4 }.pack()], at: DVec3::new(12.0, 20.0, 15.0) }]);
                d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 1, sub: 4 }.pack(), at: DVec3::new(-12.0, 8.0, 15.0) }, inset: 2.0, depth: -1.0, keep: false });
                (d, 1)
            }
        };
        h.reset(d, "Freeze");
        h.settle();
        let errors = |h: &Harness| h.st.model.as_ref().map(|m| m.errors.len()).unwrap_or(0);
        let faces = |h: &Harness| h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0);
        h.key(egui::Key::Escape);
        h.key(egui::Key::Num4);
        h.st.selection = vec![id];
        h.st.frame_selection();
        h.settle();
        h.move_to(pos2(1500.0, 120.0));
        h.steps(3);
        h.shot("zero-freeze-1-before");
        eprintln!("  before: {} features, {} operations of it, {} faces, {} errors", h.st.doc.features.len(), h.st.element_ops(id).len(), faces(&h), errors(&h));
        h.hold(Modifiers { alt: true, shift: true, ..Default::default() });
        h.key(egui::Key::D);
        h.hold(Modifiers::default());
        for _ in 0..4 {
            h.settle();
        }
        h.steps(3);
        h.shot("zero-freeze-2-frozen");
        let fz = h.st.doc.feature(id).map(|f| f.frozen.is_some()).unwrap_or(false);
        eprintln!("  frozen {fz}: {} features, {} operations of it, {} faces, {} errors, toast {:?}", h.st.doc.features.len(), h.st.element_ops(id).len(), faces(&h), errors(&h), h.st.toast.as_ref().map(|t| t.text.clone()));
        let mut log = vec![];
        let before = h.st.doc.clone();
        gizmo_drag(&mut h, &mut log, "frozen element", &[id], Vec3::X, "zero-freeze-3-moved");
        let _ = before;
        h.st.undo_doc();
        h.settle();
        h.settle();
        eprintln!("  undone: {} features, {} operations of it, {} faces, {} errors, frozen {}", h.st.doc.features.len(), h.st.element_ops(id).len(), faces(&h), errors(&h), h.st.doc.feature(id).map(|f| f.frozen.is_some()).unwrap_or(false));
        h.shot("zero-freeze-4-undone");
    }

    if want("uni-doc") {
        // FORME_DOC a saved scene, FORME_PAINT a shape: its whole Object
        // picked, Y then R: the box round the part as built
        if let Ok(path) = std::env::var("FORME_DOC") {
            let d = nurbs_kernel::Doc::from_json(&std::fs::read_to_string(&path).expect("FORME_DOC")).expect("a document");
            let id: u32 = std::env::var("FORME_PAINT").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
            h.reset(d, "Scene");
            h.settle();
            h.key(egui::Key::Escape);
            let ids = h.st.doc.objects.iter().find(|o| o.members.contains(&id)).map(|o| o.members.clone()).unwrap_or(vec![id]);
            h.st.selection = ids;
            h.st.frame_selection();
            h.settle();
            h.key(egui::Key::Y);
            h.steps(3);
            eprintln!("  cage {:?}", h.st.cage().map(|c| (c.lo, c.hi)));
            h.shot("zero-uni-doc-1-y");
            h.key(egui::Key::R);
            h.steps(3);
            h.shot("zero-uni-doc-2-r");
        }
    }

    if want("sel-doc") {
        // FORME_DOC a saved scene, FORME_SEL a step selected (FORME_MODE 1
        // to 4): its handles printed and shot
        if let Ok(path) = std::env::var("FORME_DOC") {
            let d = nurbs_kernel::Doc::from_json(&std::fs::read_to_string(&path).expect("FORME_DOC")).expect("a document");
            let id: u32 = std::env::var("FORME_SEL").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
            h.reset(d, "Scene");
            h.settle();
            let key = match std::env::var("FORME_MODE").as_deref() {
                Ok("1") => egui::Key::Num1,
                Ok("2") => egui::Key::Num2,
                Ok("3") => egui::Key::Num3,
                _ => egui::Key::Num4,
            };
            h.key(key);
            h.st.selection = vec![id];
            h.st.frame_selection();
            h.settle();
            h.steps(3);
            let ph = h.st.push_handles();
            eprintln!("  push handles {:?}", ph.map(|p| (p.base, p.tip, p.n)));
            if let Some(m) = h.st.model.as_ref() {
                for (i, f) in m.faces.iter().enumerate().filter(|(_, f)| f.tag.map(|t| t.feature == id).unwrap_or(false)) {
                    eprintln!("  face {i} {:?} point {:?} n {:?}", f.tag, f.point, f.normal);
                }
            }
            eprintln!("  pivot {:?}", h.st.pivot());
            h.shot("zero-sel-doc");
        }
    }

    if want("paint") {
        // An element's material on all its faces (Romain 23 nov., 21 h 03:
        // the pocket's walls and the Shell's stayed grey): FORME_DOC a saved
        // scene, FORME_PAINT the element; each feature whose faces lie in the
        // element's box, with the material they show
        let (d, id) = match std::env::var("FORME_DOC") {
            Ok(path) => (nurbs_kernel::Doc::from_json(&std::fs::read_to_string(&path).expect("FORME_DOC")).expect("a document"), std::env::var("FORME_PAINT").ok().and_then(|v| v.parse().ok()).unwrap_or(1)),
            Err(_) => {
                let mut d = test_part();
                d.features[0].material = 2;
                (d, 1)
            }
        };
        h.reset(d, "Paint");
        h.settle();
        h.settle();
        h.key(egui::Key::Escape);
        h.st.selection = vec![id];
        h.st.frame_selection();
        h.settle();
        let want_m = h.st.doc.feature(id).map(|f| f.material).unwrap_or(0);
        if let Some(m) = h.st.model.clone() {
            // the element's own faces' box
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for (t, tri) in m.triangles.iter().enumerate() {
                let f = m.tri_face[t] as usize;
                if m.faces[f].tag.map(|x| h.st.paint_owner(h.st.element_of(x.feature)) == id).unwrap_or(false) {
                    for &v in tri {
                        let p = Vec3::from(m.positions[v as usize]);
                        lo = lo.min(p);
                        hi = hi.max(p);
                    }
                }
            }
            let mut rows: std::collections::BTreeMap<(u32, u32, u32), usize> = Default::default();
            for (t, tri) in m.triangles.iter().enumerate() {
                let f = m.tri_face[t] as usize;
                let Some(tag) = m.faces[f].tag else { continue };
                let c = tri.iter().map(|&v| Vec3::from(m.positions[v as usize])).sum::<Vec3>() / 3.0;
                if c.cmpge(lo - 0.05).all() && c.cmple(hi + 0.05).all() {
                    *rows.entry((tag.feature, h.st.paint_owner(h.st.element_of(tag.feature)), h.st.face_material(f))).or_default() += 1;
                }
            }
            eprintln!("  element {id} material {want_m}, its box {lo:?} {hi:?}");
            for ((fid, el, mat), n) in rows {
                let name = h.st.doc.feature(fid).map(|f| f.name.clone()).unwrap_or_default();
                eprintln!("  {}{fid} {name}: element {el}, material {mat}, {n} triangles", if mat != want_m { "!! " } else { "   " });
            }
        }
        h.move_to(pos2(1500.0, 120.0));
        h.steps(3);
        h.shot("zero-paint-1");
    }

    if want("delete-edges") {
        // Delete edges between flat faces nearly on one plane (23 nov.,
        // 19 h 25, his leg's end in three bands 0.19 mm apart; FORME_DOC: a
        // saved scene, FORME_EDGES: "tag,tag,x,y,z;…"; else a block's front
        // cut by the Knife, its right edge moved 0.3 mm out): one flat face
        let (d, edges) = match std::env::var("FORME_DOC") {
            Ok(path) => {
                let d = nurbs_kernel::Doc::from_json(&std::fs::read_to_string(&path).expect("FORME_DOC")).expect("a document");
                let edges: Vec<EdgeRef> = std::env::var("FORME_EDGES")
                    .expect("FORME_EDGES")
                    .split(';')
                    .map(|e| {
                        let v: Vec<f64> = e.split(',').map(|x| x.trim().parse().unwrap()).collect();
                        EdgeRef { faces: [v[0] as i64, v[1] as i64], at: DVec3::new(v[2], v[3], v[4]) }
                    })
                    .collect();
                (d, edges)
            }
            Err(_) => {
                let mut d = nurbs_kernel::Doc::default();
                let b = d.add(nurbs_kernel::Op::Add, nurbs_kernel::Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, glam::DQuat::IDENTITY);
                let t = |s: u16| FaceTag { feature: b, sub: s }.pack();
                let kp = |faces: [i64; 2], at: DVec3| nurbs_kernel::doc::KnifePoint { edge: EdgeRef { faces, at }, f: 0.5, at };
                d.add_knife(nurbs_kernel::doc::Knife { pts: vec![kp([t(4), t(2)], DVec3::new(0.0, 20.0, 15.0)), kp([t(4), t(3)], DVec3::new(0.0, 0.0, 15.0))], segs: vec![[0, 1]], remove_faces: vec![], remove_edges: vec![], fill: vec![] }, "Knife");
                d.add_tweak(nurbs_kernel::doc::Tweak { comp: nurbs_kernel::doc::Comp::Edge(EdgeRef { faces: [t(4), t(0)], at: DVec3::new(20.0, 10.0, 15.0) }), delta: DVec3::new(0.0, 0.0, 0.3), more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true, bend: true });
                (d, vec![EdgeRef { faces: [t(4), t(4)], at: DVec3::new(0.0, 10.0, 15.0) }])
            }
        };
        h.reset(d, "Delete edges");
        h.settle();
        let tags: Vec<i64> = edges.iter().flat_map(|e| e.faces).collect();
        let count = |h: &Harness| h.st.model.as_ref().map(|m| m.faces.iter().filter(|f| f.tag.map(|t| tags.contains(&t.pack())).unwrap_or(false)).count()).unwrap_or(0);
        h.key(egui::Key::Escape);
        h.key(egui::Key::Num2);
        h.st.edge_sel = edges.clone();
        h.st.frame_current();
        h.settle();
        h.move_to(pos2(1500.0, 120.0));
        h.steps(3);
        h.shot("zero-delete-edges-1-before");
        eprintln!("  before: {} faces of those tags", count(&h));
        h.key(egui::Key::Delete);
        for _ in 0..3 {
            h.settle();
        }
        h.steps(3);
        h.shot("zero-delete-edges-2-after");
        eprintln!("  after: {} faces of those tags, errors {:?}, toast {:?}", count(&h), h.st.model.as_ref().map(|m| m.errors.iter().filter(|e| e.starts_with("Delete edges")).cloned().collect::<Vec<_>>()), h.st.toast.as_ref().map(|t| t.text.clone()));
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
                d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 3, sub: 1 }.pack(), at: DVec3::new(10.0, 30.0, 0.0) }, inset: 2.0, depth: 2.0, keep: false });
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
        d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 1, sub: 4 }.pack(), at: DVec3::new(-12.0, 8.0, 15.0) }, inset: 2.0, depth: -1.0, keep: false });
        d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 3, sub: 1 }.pack(), at: DVec3::new(10.0, 30.0, 0.0) }, inset: 2.0, depth: 2.0, keep: false });
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
            d.add_push(nurbs_kernel::doc::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: 1, sub: 0 }.pack(), at: DVec3::new(20.0, 10.0, 0.0) }, inset: 3.0, depth: -1.0, keep: false });
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
        h.st.set_mirror(nurbs_kernel::Mirror { axis: 2, offset: 0.0, flip: true, single: false }, true);
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

    if want("inset-curved") {
        // Inset on curved faces (Romain 23 nov., 22 h 05: "Si possible que
        // le Inset fonctionne sur face courbé"): the Boss's side clicked
        // with the Inset tool (a band 2 mm in from its rims), raised by
        // its arrow, then sunk; a fillet's face inset and sunk; the
        // Hole's wall inset and pushed into the block
        // (the Boss 2 mm along +X, off the Slot: a side notched by it is
        // refused for now)
        let mut doc = test_part();
        if let Some(b) = doc.features.iter_mut().find(|f| f.name == "Boss") {
            b.origin.x = 12.0;
        }
        h.reset(doc, "Test part");
        h.key(egui::Key::Num3);
        h.st.camera.frame(Vec3::new(-20.0, 0.0, -15.0), Vec3::new(20.0, 30.0, 15.0));
        h.steps(2);
        h.st.arm_push(PushKind::Inset, None);
        let side = Vec3::new(12.0, 25.0, 7.0);
        let a = h.screen_of(side);
        h.move_to(a);
        h.steps(2);
        h.click(a);
        h.settle();
        let ex = |h: &Harness| h.st.doc.features.iter().filter_map(|f| f.push.as_ref().map(|p| (f.name.clone(), p.inset, p.depth))).collect::<Vec<_>>();
        eprintln!("  inset {:?} errors {:?}", ex(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-inset-curved-1-band");
        if std::env::var_os("FORME_SAVE").is_some() {
            let path = format!("{}/inset-curved.fnurbs", h.out);
            eprintln!("  saved {path}: {:?}", std::fs::write(&path, h.st.doc.to_json()));
        }
        // its arrow dragged out 1.5 mm
        if let Some(hs) = h.st.push_handles() {
            let a = h.screen_of(hs.tip.as_vec3() - hs.n.as_vec3() * 0.3);
            let b = h.screen_of((hs.tip + hs.n * 1.5).as_vec3() - hs.n.as_vec3() * 0.3);
            h.move_to(a);
            h.steps(2);
            h.drag(a, b, 8, true);
            h.settle();
        }
        eprintln!("  raised {:?} errors {:?}", ex(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.move_to(h.screen_of(Vec3::new(0.0, -10.0, 40.0)));
        h.steps(2);
        h.shot("zero-inset-curved-2-raised");
        h.st.set_push(2.0, -1.5, true);
        h.st.end_live();
        h.settle();
        eprintln!("  sunk {:?} errors {:?}", ex(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-inset-curved-3-sunk");
        // a fillet on the block's front top edge, its face inset 1 mm and sunk 1 mm
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
        h.st.edit("Inset", |d| p = d.add_push(nurbs_kernel::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: r, sub: 0 }.pack(), at }, inset: 1.0, depth: -1.0, keep: false }));
        h.st.selection = vec![p];
        h.settle();
        eprintln!("  fillet face {:?} errors {:?}", ex(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-inset-curved-4-fillet-face");
        // the Hole's wall: a groove 1 mm into the block, 3 mm in from its rims
        let hole = h.st.doc.features.iter().find(|f| f.name == "Hole").map(|f| f.id).unwrap();
        let mut q = 0;
        h.st.edit("Inset", |d| q = d.add_push(nurbs_kernel::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: hole, sub: 0 }.pack(), at: DVec3::new(-10.0, 10.0, 5.0) }, inset: 3.0, depth: -1.0, keep: false }));
        h.st.selection = vec![q];
        h.settle();
        eprintln!("  hole wall {:?} errors {:?}", ex(&h), h.st.model.as_ref().map(|m| m.errors.clone()));
        // seen from above, into the hole
        h.st.camera.frame(Vec3::new(-16.0, 0.0, -6.0), Vec3::new(-4.0, 20.0, 6.0));
        h.st.camera.pitch = 1.15;
        h.st.camera.yaw = 0.3;
        h.move_to(egui::pos2(500.0, 860.0));
        h.steps(2);
        h.settle();
        h.shot("zero-inset-curved-5-hole-wall");
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
        h.st.edit("Extrude", |d| q = d.add_push(nurbs_kernel::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: hole, sub: 0 }.pack(), at: DVec3::new(-10.0, 10.0, 5.0) }, inset: 0.0, depth: -2.0, keep: false }));
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
        h.st.edit("Extrude", |d| p = d.add_push(nurbs_kernel::Push { face: nurbs_kernel::FaceRef { tag: FaceTag { feature: r, sub: 0 }.pack(), at }, inset: 0.0, depth: 1.5, keep: false }));
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
                fill: vec![],
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
                    // FORME_SHOTS: pictures during the drag
                    if std::env::var_os("FORME_SHOTS").is_some() && (i == 12 || i == 40) {
                        h.shot(&format!("zero-perf-{id}-drag-{i}"));
                    }
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
                if std::env::var_os("FORME_SHOTS").is_some() {
                    eprintln!("    after the release: {:?} · toast {:?}", h.st.model.as_ref().map(|m| m.errors.clone()), h.st.toast.as_ref().map(|t| t.text.clone()));
                    h.shot(&format!("zero-perf-{id}-released"));
                }
                h.st.undo();
                h.settle();
            }
        }
    }

    if want("object") {
        object(&mut h);
    }

    if want("gear") {
        gear(&mut h);
    }

    if want("library") {
        library(&mut h, &out);
    }

    if want("cable") {
        cable(&mut h);
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
    if want("nov23") {
        // Romain's list of 23 nov., 7 h 04 to 7 h 12
        use nurbs_kernel::doc::{CurveKind, CurveOp};
        use nurbs_kernel::{Comp, FaceRef, Push, Tweak};
        let tag = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
        let errors = |h: &Harness| h.st.model.as_ref().map(|m| m.errors.clone()).unwrap_or_default();
        // 1. a 2D boolean's corners rounded where its curves cross ("Il
        // faut créer des points à fillet aux intersections")
        h.reset(Doc::default(), "Corners");
        h.st.camera.frame(Vec3::new(-30.0, 0.0, -30.0), Vec3::new(30.0, 5.0, 25.0));
        h.st.camera.pitch = 1.2;
        h.steps(2);
        let w = |x: f32, z: f32| Vec3::new(x, 0.0, z);
        for (k, a, b) in [(CurveKind::Rectangle, w(-25.0, -5.0), w(25.0, 22.0)), (CurveKind::Circle, w(0.0, -10.0), w(16.0, -10.0))] {
            h.st.arm_curve(k);
            h.steps(1);
            let (pa, pb) = (h.screen_of(a), h.screen_of(b));
            h.drag(pa, pb, 8, false);
            h.steps(1);
            h.press(pb, false);
            h.steps(2);
        }
        let ids: Vec<_> = h.st.doc.features.iter().map(|f| f.id).collect();
        h.st.selection = ids.clone();
        h.st.make_curve_bool(CurveOp::Union);
        h.settle();
        let b = *h.st.selection.first().unwrap();
        h.st.select(None, false);
        h.key(egui::Key::F9);
        h.steps(2);
        let corners = h.st.bool_corners();
        eprintln!("  boolean corners: {:?}", corners.iter().map(|c| (c.0, (c.1 * 100.0).round() / 100.0, c.2)).collect::<Vec<_>>());
        h.shot("zero-nov23-1-corners");
        // the two where the circle meets the rectangle's side, picked by clicks
        let cross: Vec<DVec3> = corners.iter().filter(|c| c.1.z < -4.0).map(|c| c.1).collect();
        for (k, q) in cross.iter().enumerate() {
            if k > 0 {
                h.hold(Modifiers::SHIFT);
            }
            h.click(h.screen_of(q.as_vec3()));
            h.hold(Modifiers::NONE);
        }
        eprintln!("  picked {:?} -> {} corners of the boolean", h.st.vert_sel.iter().map(|v| (*v * 100.0).round() / 100.0).collect::<Vec<_>>(), h.st.picked_bool_corners().len());
        h.st.set_bool_corners(4.0, false, true);
        h.st.end_live();
        h.settle();
        eprintln!("  rounded: {:?}, errors {:?}", h.st.doc.feature(b).and_then(|f| f.shape_bool.clone()).map(|s| s.corners), errors(&h));
        h.shot("zero-nov23-2-rounded");
        // a chamfer on one, then the boolean pulled into a solid
        h.st.vert_sel.truncate(1);
        h.st.set_bool_corners(3.0, true, true);
        h.st.end_live();
        h.key(egui::Key::F8);
        h.st.selection = vec![b];
        h.steps(2);
        h.st.arm_push(PushKind::Extrude, None);
        h.steps(1);
        let c = h.screen_of(w(0.0, 10.0));
        h.move_to(c);
        h.steps(1);
        h.move_to(c + vec2(0.0, -60.0));
        h.steps(1);
        h.press(c + vec2(0.0, -60.0), false);
        h.settle();
        eprintln!("  extruded: {:?}, errors {:?}", h.st.doc.feature(b).map(|f| f.prim), errors(&h));
        h.key(egui::Key::Escape);
        h.st.select(None, false);
        h.st.camera.pitch = 0.6;
        h.settle();
        h.shot("zero-nov23-3-extruded");

        // 2. after a Mirror, an Extrude made on this side only
        let mut d = Doc::default();
        let blk = d.add(Op::Add, Prim::Box { x: 20.0, y: 30.0, z: 20.0 }, DVec3::new(20.0, 0.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(blk).unwrap().name = "Leg".into();
        h.reset(d, "Mirror side");
        h.st.camera.dist *= 1.6;
        h.st.camera.yaw += 0.5;
        h.settle();
        let mir = h.st.add_mirror(0).unwrap_or(0);
        h.settle();
        h.st.select(None, false);
        let mut push = 0;
        h.st.edit("Extrude", |d| push = d.add_push(Push { face: FaceRef { tag: tag(blk, 1), at: DVec3::new(10.0, 15.0, 0.0) }, inset: 4.0, depth: 6.0, keep: false }));
        h.settle();
        eprintln!("  toast {:?}, after mirror {}, errors {:?}", h.st.toast.as_ref().map(|t| t.text.clone()), h.st.after_mirror(push), errors(&h));
        h.st.selection = vec![push];
        h.steps(3);
        h.shot("zero-nov23-4-both-sides");
        h.st.set_one_side(push, true);
        h.settle();
        let vol = |h: &Harness| h.st.model.as_ref().and_then(|m| m.bounds()).map(|(lo, hi)| (lo[0], hi[0]));
        eprintln!("  this side only: x {:?}, errors {:?}", vol(&h), errors(&h));
        h.steps(3);
        h.shot("zero-nov23-5-one-side");
        // the toast's "Always this side": the Mirror's later steps all on
        // their own side (its window says so), a second Extrude too
        h.st.set_one_side(push, false);
        h.settle();
        h.st.toast_act(nurbs_app::state::ToastAct::MirrorSingle(push));
        h.settle();
        eprintln!("  mirror single: {:?}, x {:?}, errors {:?}", h.st.doc.feature(mir).and_then(|f| f.mirror).map(|m| m.single), vol(&h), errors(&h));
        let mut p2 = 0;
        h.st.edit("Extrude", |d| p2 = d.add_push(Push { face: FaceRef { tag: tag(blk, 4), at: DVec3::new(20.0, 15.0, 10.0) }, inset: 3.0, depth: 4.0, keep: false }));
        h.settle();
        eprintln!("  second extrude: toast {:?}, z {:?}", h.st.toast.as_ref().map(|t| t.text.clone()), h.st.model.as_ref().and_then(|m| m.bounds()).map(|(lo, hi)| (lo[2], hi[2])));
        let _ = p2;
        h.st.selection = vec![mir];
        h.steps(3);
        h.shot("zero-nov23-5b-mirror-single");

        // 3. the face of an inset left as a line moved out along its
        // normal: walls made, no refusal
        let mut d = Doc::default();
        let blk = d.add(Op::Add, Prim::Box { x: 30.0, y: 30.0, z: 30.0 }, DVec3::new(0.0, 0.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(blk).unwrap().name = "Block".into();
        d.add_round(4.0, false, vec![EdgeRef { faces: [tag(blk, 0), tag(blk, 3)], at: DVec3::new(15.0, 0.0, 0.0) }]);
        let ins = d.add_push(Push { face: FaceRef { tag: tag(blk, 0), at: DVec3::new(15.0, 15.0, 0.0) }, inset: 3.0, depth: 0.0, keep: false });
        h.reset(d, "Move inset face");
        h.st.camera.yaw += 0.8;
        h.settle();
        eprintln!("  inset made, errors {:?}", errors(&h));
        let mut t = Tweak::new(vec![Comp::Face(FaceRef { tag: tag(ins, 1), at: DVec3::new(15.0, 15.0, 0.0) })]).unwrap();
        t.delta = DVec3::new(5.0, 0.0, 0.0);
        let mut mv = 0;
        h.st.edit("Move face", |d| mv = d.add_tweak(t.clone()));
        h.settle();
        eprintln!("  moved out 5: errors {:?}, x to {:?}", errors(&h), vol(&h).map(|v| v.1));
        h.steps(3);
        h.shot("zero-nov23-6-move-inset-face");
        h.st.edit("Move face", |d| d.feature_mut(mv).unwrap().tweak.as_mut().unwrap().delta = DVec3::new(-4.0, 0.0, 0.0));
        h.settle();
        eprintln!("  moved in 4: errors {:?}, x to {:?}", errors(&h), vol(&h).map(|v| v.1));
        h.steps(3);
        h.shot("zero-nov23-7-move-inset-face-in");

        // 4. Shift + drag on the move gizmo's arrow with a face picked: an
        // extrusion, not a move
        h.st.undo_doc();
        h.st.undo_doc();
        h.settle();
        h.key(egui::Key::F11);
        let top = h.screen_of(Vec3::new(0.0, 30.0, 0.0));
        h.click(top);
        h.key(egui::Key::W);
        h.steps(2);
        let n0 = h.st.doc.features.len();
        let c = h.st.pivot().expect("a gizmo").as_vec3();
        let s0 = h.screen_of(c);
        let dir = (h.screen_of(c + Vec3::Y) - s0).normalized();
        let grab = s0 + dir * 55.0;
        h.hold(Modifiers::SHIFT);
        h.drag(grab, grab + dir * 70.0, 10, false);
        h.steps(1);
        h.press(grab + dir * 70.0, false);
        h.hold(Modifiers::NONE);
        h.settle();
        let made: Vec<String> = h.st.doc.features[n0.min(h.st.doc.features.len())..].iter().map(|f| format!("{} {:?}", f.name, f.push.as_ref().map(|p| p.depth))).collect();
        eprintln!("  shift + drag on the arrow made {made:?}, errors {:?}", errors(&h));
        h.steps(3);
        h.shot("zero-nov23-8-shift-gizmo");

        // 6. Bridge: two legs under a block, their inner faces picked and
        // joined ("Il faut un Bridge tool"), then the same with a Mirror
        let mut d = Doc::default();
        let top = d.add(Op::Add, Prim::Box { x: 60.0, y: 12.0, z: 20.0 }, DVec3::new(0.0, 30.0, 0.0), DQuat::IDENTITY);
        let l1 = d.add(Op::Add, Prim::Box { x: 8.0, y: 30.0, z: 16.0 }, DVec3::new(-14.0, 0.0, 0.0), DQuat::IDENTITY);
        let l2 = d.add(Op::Add, Prim::Box { x: 8.0, y: 30.0, z: 16.0 }, DVec3::new(14.0, 0.0, 0.0), DQuat::IDENTITY);
        d.add_round(5.0, false, vec![EdgeRef { faces: [tag(l1, 3), tag(l1, 1)], at: DVec3::new(-18.0, 0.0, 0.0) }]);
        d.add_round(5.0, false, vec![EdgeRef { faces: [tag(l2, 3), tag(l2, 0)], at: DVec3::new(18.0, 0.0, 0.0) }]);
        let _ = top;
        h.reset(d, "Bridge");
        h.st.camera.yaw += 0.35;
        h.settle();
        h.key(egui::Key::F11);
        // the two inner faces (the far one is turned away from the eye:
        // picked as a click on it would)
        let inner = |h: &Harness, t: i64| h.st.model.as_ref().and_then(|m| m.faces.iter().position(|f| f.tag.map(|x| x.pack()) == Some(t)));
        for (t, at) in [(tag(l1, 0), DVec3::new(-10.0, 15.0, 0.0)), (tag(l2, 1), DVec3::new(10.0, 15.0, 0.0))] {
            let i = inner(&h, t).expect("an inner face");
            let r = h.st.face_ref(i, at).expect("its ref");
            h.st.face_sel.push(r);
        }
        h.steps(3);
        eprintln!("  faces picked {:?}", h.st.face_sel.iter().map(|r| FaceTag::unpack(r.tag)).collect::<Vec<_>>());
        h.shot("zero-nov23-10-bridge-picked");
        let br = h.st.add_bridge();
        h.settle();
        eprintln!("  bridge {br:?}: faces {:?}, errors {:?}", h.st.model.as_ref().map(|m| m.faces.len()), errors(&h));
        h.st.select(None, false);
        h.steps(3);
        h.shot("zero-nov23-11-bridged");

        // 5. the rectangle in Object mode takes an element it only touches
        let mut d = Doc::default();
        let a = d.add(Op::Add, Prim::Box { x: 20.0, y: 20.0, z: 20.0 }, DVec3::new(-20.0, 0.0, 0.0), DQuat::IDENTITY);
        let b2 = d.add(Op::Add, Prim::Box { x: 20.0, y: 20.0, z: 20.0 }, DVec3::new(20.0, 0.0, 0.0), DQuat::IDENTITY);
        h.reset(d, "Rectangle");
        h.settle();
        h.key(egui::Key::Q);
        let p0 = h.screen_of(Vec3::new(-14.0, 25.0, 0.0));
        let p1 = h.screen_of(Vec3::new(-6.0, 12.0, 0.0));
        h.drag(p0, p1, 8, true);
        h.steps(2);
        eprintln!("  rectangle over a corner of {a}: selection {:?} (other {b2})", h.st.selection);
        h.shot("zero-nov23-9-marquee");
    }
    if want("sym") {
        // a shape's own symmetry (Romain 23 nov., 21 h 03: "Chaque objet
        // doit avoir sa symétrie en local"): a block 60 mm off the origin,
        // turned 30°, Alt + M on it: its hole and a fillet on its +X end made
        // on both sides of its own plane, which turns with it
        use nurbs_kernel::{EdgeRef, FaceRef, Push};
        let tag = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
        let mut d = Doc::default();
        let q = DQuat::from_rotation_y(30f64.to_radians());
        let o = DVec3::new(60.0, 0.0, 0.0);
        let w = |l: DVec3| o + q * l;
        let b = d.add(Op::Add, Prim::Box { x: 50.0, y: 16.0, z: 24.0 }, o, q);
        d.feature_mut(b).unwrap().name = "Body".into();
        let h0 = d.add(Op::Carve, Prim::Cylinder { radius: 4.0, height: 24.0 }, w(DVec3::new(14.0, -4.0, 0.0)), q);
        d.feature_mut(h0).unwrap().name = "Hole".into();
        d.add_round(3.0, false, vec![EdgeRef { faces: [tag(b, 0), tag(b, 2)], at: w(DVec3::new(25.0, 16.0, 0.0)) }]);
        d.add_push(Push { face: FaceRef { tag: tag(b, 4), at: w(DVec3::new(15.0, 8.0, 12.0)) }, inset: 3.0, depth: -2.0, keep: false });
        h.reset(d, "Symmetry");
        h.st.settings.mirror_bar = true;
        h.st.settings.mirror_plane = true;
        h.settle();
        h.key(egui::Key::Escape);
        h.st.selection = vec![b];
        h.st.frame_selection();
        h.settle();
        h.move_to(pos2(1500.0, 120.0));
        h.steps(3);
        h.shot("zero-sym-1-before");
        let faces = |h: &Harness| h.st.model.as_ref().map(|m| m.faces.len()).unwrap_or(0);
        let errors = |h: &Harness| h.st.model.as_ref().map(|m| m.errors.clone()).unwrap_or_default();
        eprintln!("  before: {} faces {:?}", faces(&h), errors(&h));
        h.hold(Modifiers { alt: true, ..Default::default() });
        h.key(egui::Key::M);
        h.hold(Modifiers::default());
        h.settle();
        h.settle();
        h.steps(3);
        eprintln!("  Alt + M: sym {:?}, {} faces {:?}, toast {:?}", h.st.doc.feature(b).and_then(|f| f.sym), faces(&h), errors(&h), h.st.toast.as_ref().map(|t| t.text.clone()));
        h.shot("zero-sym-2-on");
        // across its own Z instead
        h.st.set_local_sym(Some(nurbs_kernel::Sym { axis: 2, flip: false }));
        h.settle();
        h.settle();
        h.steps(3);
        eprintln!("  its Z: {} faces {:?}", faces(&h), errors(&h));
        h.shot("zero-sym-3-z");
        h.st.set_local_sym(Some(nurbs_kernel::Sym { axis: 0, flip: false }));
        h.settle();
        // nothing selected: the bar is the scene's Mirror again
        h.key(egui::Key::Escape);
        h.st.selection.clear();
        h.settle();
        h.steps(3);
        h.shot("zero-sym-4-deselected");
    }

    if want("mirror-bar") {
        // the Mirror bar (Romain 23 nov., 14 h 15): its icon in the top
        // bar, On / X Y Z / Offset / Keep / Steps after it / Plane, and the
        // selection's state
        use nurbs_kernel::{FaceRef, Push};
        let tag = |f: u32, sub: u16| FaceTag { feature: f, sub }.pack();
        let mut d = Doc::default();
        let leg = d.add(Op::Add, Prim::Box { x: 20.0, y: 30.0, z: 20.0 }, DVec3::new(20.0, 0.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(leg).unwrap().name = "Leg".into();
        h.reset(d, "Mirror bar");
        h.st.camera.dist *= 1.6;
        h.st.camera.yaw += 0.5;
        h.st.settings.mirror_bar = true;
        h.st.settings.mirror_plane = true;
        h.settle();
        h.shot("zero-mirror-bar-1-none");
        h.st.set_scene_mirror_on(true, 0);
        h.settle();
        h.st.selection = vec![leg];
        h.steps(3);
        eprintln!("  on: {} mirrors, state {:?}", h.st.scene_mirror_ids().len(), h.st.mirror_state());
        h.shot("zero-mirror-bar-2-on-leg");
        let mut push = 0;
        h.st.edit("Extrude", |d| push = d.add_push(Push { face: FaceRef { tag: tag(leg, 4), at: DVec3::new(20.0, 15.0, 10.0) }, inset: 4.0, depth: 5.0, keep: false }));
        h.settle();
        h.st.selection = vec![push];
        h.steps(3);
        eprintln!("  extrude: state {:?}", h.st.mirror_state());
        h.shot("zero-mirror-bar-3-step-both");
        let m = h.st.scene_mirror().and_then(|f| f.mirror).unwrap();
        h.st.set_scene_mirror(nurbs_kernel::Mirror { single: true, ..m }, true);
        h.st.end_live();
        h.settle();
        h.steps(3);
        eprintln!("  steps after it on one side: state {:?}, errors {:?}", h.st.mirror_state(), h.st.model.as_ref().map(|m| m.errors.clone()));
        h.shot("zero-mirror-bar-4-this-side");
        h.st.settings.mirror_plane = false;
        h.st.set_scene_mirror_on(false, 0);
        h.settle();
        h.steps(3);
        eprintln!("  off: on {}, x {:?}", h.st.scene_mirror_on(), h.st.model.as_ref().and_then(|m| m.bounds()).map(|(lo, hi)| (lo[0], hi[0])));
        h.shot("zero-mirror-bar-5-off");
    }

    if want("nav") {
        // the navigation with a pen's tip alone (Romain 22 nov., 16 h 49:
        // "La navigation doit pouvoir se faire full stylet"): the island at
        // the view's top right, its ball dragged and its axes tapped, Zoom,
        // Pan, Frame; then Alt + Shift / Alt + Ctrl with the tip, the pen's
        // button as Alt
        use nurbs_app::camera::ViewPreset;
        use nurbs_app::nav;
        h.reset(test_part(), "Test part");
        // off by default on the PC (Settings › Display › Navigation buttons)
        h.st.settings.nav = true;
        h.steps(2);
        let cam = |h: &Harness| format!("yaw {:.3} pitch {:.3} dist {:.1} target [{:.1}, {:.1}, {:.1}] {:?}", h.st.camera.yaw, h.st.camera.pitch, h.st.camera.dist, h.st.camera.target.x, h.st.camera.target.y, h.st.camera.target.z, h.st.camera.preset_of());
        let r = nav::rects(&h.ctx).expect("the navigation island");
        eprintln!("  island: ball {:?} zoom {:?} pan {:?} frame {:?}", r.ball, r.zoom, r.pan, r.frame);
        eprintln!("  start: {}", cam(&h));
        h.move_to(r.ball.center() + vec2(-14.0, 20.0));
        h.steps(2);
        h.shot("zero-nav-1-island");
        // the ball dragged: the view turns
        let c = r.ball.center();
        h.drag(c + vec2(-14.0, 20.0), c + vec2(70.0, 34.0), 14, false);
        eprintln!("  ball dragged: {}", cam(&h));
        h.shot("zero-nav-2-orbit");
        h.press(c + vec2(70.0, 34.0), false);
        h.steps(2);
        // an axis tapped: the view along it, again: the other side
        let tap = |h: &mut Harness, axis: usize, sign: f32| {
            let c = nav::rects(&h.ctx).unwrap().ball.center();
            let dots = nav::axis_dots(&h.st, c);
            // the dot nearest the eye of those on that axis and side
            let d = dots.iter().rev().find(|d| d.axis == axis && d.sign == sign).copied().unwrap();
            h.click(d.at);
            h.settle();
        };
        tap(&mut h, 0, 1.0);
        eprintln!("  +X tapped: {}", cam(&h));
        h.move_to(r.ball.center() + vec2(0.0, 200.0));
        h.steps(2);
        h.shot("zero-nav-3-right");
        tap(&mut h, 0, 1.0);
        eprintln!("  +X again: {}", cam(&h));
        h.shot("zero-nav-4-left");
        tap(&mut h, 1, 1.0);
        eprintln!("  +Y tapped: {}", cam(&h));
        h.shot("zero-nav-5-top");
        // back to 3D, then Zoom and Pan dragged with the tip
        h.st.camera.preset(ViewPreset::Perspective);
        h.st.frame_all();
        h.settle();
        let d0 = h.st.camera.dist;
        let z = r.zoom.center();
        h.drag(z, z + vec2(0.0, -90.0), 12, false);
        eprintln!("  zoom dragged up 90: dist {:.1} → {:.1}", d0, h.st.camera.dist);
        h.shot("zero-nav-6-zoom");
        h.press(z + vec2(0.0, -90.0), false);
        h.steps(2);
        let t0 = h.st.camera.target;
        let p = r.pan.center();
        h.drag(p, p + vec2(-160.0, 60.0), 12, false);
        eprintln!("  pan dragged: target {:?} → {:?}", t0, h.st.camera.target);
        h.shot("zero-nav-7-pan");
        h.press(p + vec2(-160.0, 60.0), false);
        h.steps(2);
        h.click(r.frame.center());
        h.settle();
        eprintln!("  frame: {}", cam(&h));
        h.shot("zero-nav-8-frame");
        // with the keyboard: Alt + Shift + tip pans, Alt + Ctrl + tip zooms
        // (in the view, on empty space: nothing picked)
        let e = pos2(560.0, 240.0);
        let (t0, d0) = (h.st.camera.target, h.st.camera.dist);
        h.hold(Modifiers { alt: true, shift: true, ..Default::default() });
        h.drag(e, e + vec2(120.0, 0.0), 10, true);
        h.hold(Modifiers { alt: true, ctrl: true, ..Default::default() });
        let target_moved = (h.st.camera.target - t0).length();
        h.drag(e, e + vec2(0.0, 90.0), 10, true);
        h.hold(Modifiers::NONE);
        eprintln!("  Alt + Shift: target moved {:.1} mm · Alt + Ctrl: dist {:.1} → {:.1} · selection {:?}", target_moved, d0, h.st.camera.dist, h.st.selection);
        // the pen's button held (Windows Ink): the tip's drag turns the view
        // and picks nothing, even when it starts on the part
        h.st.frame_all();
        h.settle();
        let y0 = h.st.camera.yaw;
        nurbs_app::pen::set_barrel(true);
        let on_part = h.screen_of(Vec3::new(14.0, 6.0, 15.0));
        eprintln!("  pen button held: {} (on the part at {:?})", nurbs_app::pen::barrel(), on_part);
        h.drag(on_part, on_part + vec2(90.0, 0.0), 10, true);
        nurbs_app::pen::set_barrel(false);
        eprintln!("  pen button + drag on the part: yaw {:.3} → {:.3} · selection {:?} (nothing picked)", y0, h.st.camera.yaw, h.st.selection);
        // a shape's window placed: not over the island
        h.click(on_part);
        h.settle();
        let pop = h.ctx.memory(|m| m.area_rect(egui::Id::new("op-pop")));
        let nav_r = h.ctx.memory(|m| m.area_rect(egui::Id::new("nav")));
        eprintln!("  window {:?} island {:?} overlap {}", pop, nav_r, pop.zip(nav_r).map(|(a, b)| a.intersects(b)).unwrap_or(false));
        h.shot("zero-nav-9-window");
        // the setting hides it
        h.st.settings.nav = false;
        h.steps(2);
        eprintln!("  hidden: {}", nav::rects(&h.ctx).is_none());
        h.shot("zero-nav-10-hidden");
        h.st.settings.nav = true;
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
        // framed wider on the right: the brush's window sits left of the
        // navigation island there
        h.st.camera.frame(Vec3::new(-24.0, 0.0, -20.0), Vec3::new(96.0, 45.0, 20.0));
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
        d.add_knife(Knife { pts: vec![kx([t(b, 4), t(r, 0)], glam::DVec3::new(-30.0, 33.4, 13.7)), kx([t(b, 4), t(b, 3)], glam::DVec3::new(5.0, 0.0, 13.7))], segs: vec![[0, 1]], remove_faces: vec![], remove_edges: vec![], fill: vec![] }, "Knife");
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
    if want("bend") {
        // the move made as Maya does (23 nov., 20 h 03: "revoir l'algo de
        // déplacement de composant de zéro"): every face round what moves
        // follows, bent when it has to. A block's top corner with a hole in
        // the top raised 8 (refused until then: "cannot bend"), then a
        // second corner, then a box's side face with a hole pushed out
        // past its neighbours' corners
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
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-5.0, -5.0, 0.0), DQuat::IDENTITY);
        h.reset(d, "Block with a hole");
        h.st.camera.frame(Vec3::new(-22.0, 0.0, -16.0), Vec3::new(22.0, 30.0, 16.0));
        h.steps(2);
        h.settle();
        h.key(egui::Key::Num1);
        let corner = h.screen_of(Vec3::new(20.0, 20.0, 15.0));
        h.click(corner);
        h.steps(2);
        h.shot("zero-bend-1-picked");
        pull(&mut h, 1, 8.0);
        say(&h);
        h.key(egui::Key::Escape);
        h.st.select(None, false);
        h.steps(2);
        h.shot("zero-bend-2-corner-up");
        h.key(egui::Key::Num1);
        let corner = h.screen_of(Vec3::new(-20.0, 20.0, -15.0));
        h.click(corner);
        h.steps(2);
        pull(&mut h, 1, -6.0);
        say(&h);
        h.key(egui::Key::Escape);
        h.st.select(None, false);
        h.steps(2);
        h.shot("zero-bend-3-two-corners");
        // the hole's wall at the moved top: still a cylinder, cut by the
        // bent top
        h.st.undo_doc();
        h.st.undo_doc();
        h.steps(2);
        h.settle();
        h.key(egui::Key::Num2);
        let edge = h.screen_of(Vec3::new(20.0, 20.0, 0.0));
        h.click(edge);
        h.steps(2);
        pull(&mut h, 0, 8.0);
        say(&h);
        h.key(egui::Key::Escape);
        h.st.select(None, false);
        h.steps(2);
        h.shot("zero-bend-4-edge-out");
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
