#![allow(deprecated)]
//! forme-rectify-shot: runs the real app headless (egui + wgpu, a software
//! GPU is fine) on a mesh, drives its tools with real mouse events and saves
//! PNGs.
//!
//! cargo run --release -p rectify-app --bin forme-rectify-shot -- OUT_DIR MESH.obj [scenario prefix…]
//!
//! RECTIFY_LOOK="fx,fy,fz,dist,yaw,pitch": the camera of the `look`
//! scenario (a point of the mesh's box in fractions); RECTIFY_DRAG="x,y;x,y":
//! a Line drag there (screen points), shot before, during and after;
//! RECTIFY_STRENGTH="0.3,1": the Line placed shown at these strengths;
//! RECTIFY_CIRCLE="x,y;x,y": a Circle drag there instead (rim to rim), the
//! rings shown, applied (RECTIFY_LEVELS, RECTIFY_HEIGHT to set them first).

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect};
use glam::Vec3;
use rectify_app::camera::ViewPreset;
use rectify_app::draw::Drawer;
use rectify_app::state::{AppState, Tool};
use rectify_app::theme;
use rectify_core::Mesh;

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
    pending_tex: egui::TexturesDelta,
}

impl Harness {
    fn new(out: &str) -> Harness {
        let gpu = Gpu::new();
        let drawer = Drawer::new(&gpu.device, FMT);
        let ctx = egui::Context::default();
        theme::install(&ctx);
        Harness { gpu, drawer, ctx, st: AppState::new(), t: 0.0, size: [1600.0, 900.0], ppp: 2.0, pending: vec![], out: out.to_string(), pending_tex: Default::default() }
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
        let out = self.ctx.run_pass(raw, |ctx| rectify_app::run(ctx, st));
        self.pending_tex.append(out.textures_delta);
    }

    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    fn wait(&mut self) {
        while self.st.job.is_some() {
            self.step();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        self.steps(2);
    }

    fn click(&mut self, p: Pos2) {
        for down in [true, false] {
            self.pending.push(Event::PointerMoved(p));
            self.pending.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: down, modifiers: Modifiers::NONE });
            self.step();
        }
        self.steps(2);
    }

    /// A left-button drag through world points (projected on screen); with
    /// `hold`, the button stays down at the end (a stroke being painted).
    fn drag_screen(&mut self, pts: &[[f32; 2]], hold: bool) {
        let scr: Vec<Pos2> = pts.iter().map(|a| Pos2::new(a[0], a[1])).collect();
        let Some(&first) = scr.first() else { return };
        self.pending.push(Event::PointerMoved(first));
        self.step();
        self.pending.push(Event::PointerButton { pos: first, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        self.step();
        for w in scr.windows(2) {
            for k in 1..=3 {
                let t = k as f32 / 3.0;
                self.pending.push(Event::PointerMoved(w[0] + (w[1] - w[0]) * t));
                self.step();
            }
        }
        if !hold {
            let last = *scr.last().unwrap();
            self.pending.push(Event::PointerButton { pos: last, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
            self.step();
            self.steps(2);
        }
    }

    fn release(&mut self) {
        let p = self.ctx.input(|i| i.pointer.latest_pos()).unwrap_or(Pos2::ZERO);
        self.pending.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        self.step();
        self.steps(2);
    }

    fn shot(&mut self, name: &str) {
        self.steps(2);
        self.t += 1.0 / 60.0;
        let raw = self.raw();
        self.st.tick(self.t);
        let st = &mut self.st;
        let mut out = self.ctx.run_pass(raw, |ctx| rectify_app::run(ctx, st));
        let paint = self.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        let mut tex = std::mem::take(&mut self.pending_tex);
        tex.append(out.textures_delta);
        let (w, h) = ((self.size[0] * self.ppp) as u32, (self.size[1] * self.ppp) as u32);
        let target = self.gpu.target(w, h);
        let view = target.create_view(&Default::default());
        self.drawer.render(&self.gpu.device, &self.gpu.queue, &view, [w, h], self.ppp, &mut self.st, &paint, &mut tex);
        let px = self.gpu.read_rgba(&target);
        let path = format!("{}/{}.png", self.out, name);
        if let Some(dir) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(dir).ok();
        }
        image::save_buffer(&path, &px, w, h, image::ExtendedColorType::Rgba8).expect("png");
        eprintln!("{path}");
    }
}

/// Camera on a point of the mesh (fractions of its box), at a distance.
fn look(h: &mut Harness, frac: [f32; 3], dist: f32, yaw: f32, pitch: f32) {
    let (lo, hi) = h.st.doc.as_ref().unwrap().original.bounds();
    let (lo, hi) = (lo.as_vec3(), hi.as_vec3());
    let c = lo + (hi - lo) * Vec3::from(frac);
    h.st.camera.target = c;
    h.st.camera.dist = dist;
    h.st.camera.yaw = yaw;
    h.st.camera.pitch = pitch;
    h.st.camera.ortho = false;
}

/// Numbers from an env var ("a,b,c").
fn nums(s: &str) -> Vec<f32> {
    s.split(',').filter_map(|t| t.trim().parse().ok()).collect()
}

/// A Line drag through screen points: before (bare), during (the
/// interface, the edge it would fix), after (bare), and zebra before / after.
fn line_drag(h: &mut Harness, name: &str, pts: &[[f32; 2]]) {
    h.st.set_tool(Tool::Navigate);
    h.st.bare = true;
    h.shot(&format!("{name}-0-before"));
    // the points are given in the bare view: the same mesh points once the
    // interface is shown (the view's centre moves)
    let world: Vec<glam::Vec3> = pts.iter().filter_map(|p| h.st.pick_hit(*p).map(|(_, x)| x.as_vec3())).collect();
    h.st.settings.zebra = true;
    h.shot(&format!("{name}-0-zebra"));
    h.st.settings.zebra = false;
    h.st.bare = false;
    h.st.set_tool(Tool::Line);
    h.steps(2);
    let view = h.st.view();
    let scr: Vec<[f32; 2]> = world.iter().filter_map(|x| view.project(*x)).collect();
    h.drag_screen(&scr, true);
    if let Some(ld) = &h.st.line_drag {
        match &ld.fit {
            Some(Ok(f)) => eprintln!("LINE {name}: {:.1}° rms {:.3} mm, {} moved, {} on the edge", f.angle, f.rms, f.moves.idx.len(), f.edge.len()),
            Some(Err(e)) => eprintln!("LINE {name}: {e}"),
            None => eprintln!("LINE {name}: no fit"),
        }
    }
    h.shot(&format!("{name}-1-drag"));
    h.release();
    // placed: its strength set live (RECTIFY_STRENGTH="0.5,1"), then applied
    let strengths = std::env::var("RECTIFY_STRENGTH").map(|s| nums(&s)).unwrap_or_default();
    for s in &strengths {
        h.st.line_set_strength(*s as f64);
        let pc = (s * 100.0).round();
        h.shot(&format!("{name}-2-placed-{pc}"));
        h.st.bare = true;
        h.shot(&format!("{name}-2-s{pc}"));
        h.st.settings.zebra = true;
        h.shot(&format!("{name}-2-s{pc}-zebra"));
        h.st.settings.zebra = false;
        h.st.bare = false;
    }
    h.shot(&format!("{name}-2-placed"));
    h.st.line_apply();
    if let Some(t) = &h.st.toast {
        eprintln!("{}", t.text);
    }
    h.shot(&format!("{name}-2-done"));
    h.st.bare = true;
    h.shot(&format!("{name}-3-after"));
    h.st.settings.zebra = true;
    h.shot(&format!("{name}-3-zebra"));
    h.st.settings.zebra = false;
    h.st.bare = false;
}

/// How many triangles the last edit turned (> 20°, > 60°), and where the
/// largest moves are.
fn turned(st: &AppState) {
    let (Some(d), Some(step)) = (&st.doc, st.history.last()) else { return };
    let mut tris: Vec<u32> = step.moves.idx.iter().flat_map(|&v| d.topo.tris_of(v as usize).iter().copied()).collect();
    tris.sort_unstable();
    tris.dedup();
    let (mut t20, mut t60) = (0, 0);
    for &t in &tris {
        let a = d.original.tri_cross(t as usize).normalize_or_zero();
        let b = d.mesh.tri_cross(t as usize).normalize_or_zero();
        let ang = a.dot(b).clamp(-1.0, 1.0).acos().to_degrees();
        if ang > 20.0 {
            t20 += 1;
        }
        if ang > 60.0 {
            t60 += 1;
        }
    }
    eprintln!("TURNED: {} triangles near the moves, {t20} turned > 20°, {t60} > 60°", tris.len());
}

/// A Circle drag from one side of a rim to the other (screen points of the
/// bare view): before, the rings with the interface, after (bare and zebra).
fn circle_drag(h: &mut Harness, name: &str, pts: &[[f32; 2]]) {
    h.st.set_tool(Tool::Navigate);
    h.st.bare = true;
    h.shot(&format!("{name}-0-before"));
    let world: Vec<glam::Vec3> = pts.iter().filter_map(|p| h.st.pick_hit(*p).map(|(_, x)| x.as_vec3())).collect();
    h.st.settings.zebra = true;
    h.shot(&format!("{name}-0-zebra"));
    h.st.settings.zebra = false;
    h.st.bare = false;
    h.st.set_tool(Tool::Circle);
    h.steps(2);
    let view = h.st.view();
    let scr: Vec<[f32; 2]> = world.iter().filter_map(|x| view.project(*x)).collect();
    h.drag_screen(&scr, true);
    h.shot(&format!("{name}-1-drag"));
    h.release();
    let levels = std::env::var("RECTIFY_LEVELS").ok().and_then(|s| s.parse().ok());
    let height = std::env::var("RECTIFY_HEIGHT").ok().and_then(|s| s.parse().ok());
    if let Some(ce) = &mut h.st.circle {
        if let Some(l) = levels {
            ce.levels = l;
        }
        if let Some(hh) = height {
            ce.height = hh;
        }
    }
    h.st.circle_refit();
    for s in std::env::var("RECTIFY_STRENGTH").map(|s| nums(&s)).unwrap_or_default() {
        h.st.circle_set_strength(s as f64);
        let pc = (s * 100.0).round();
        h.st.bare = true;
        h.shot(&format!("{name}-2-s{pc}"));
        h.st.settings.zebra = true;
        h.shot(&format!("{name}-2-s{pc}-zebra"));
        h.st.settings.zebra = false;
        h.st.bare = false;
    }
    match &h.st.circle {
        Some(ce) => {
            let c = &ce.circle;
            let rings: Vec<String> = ce.rings.iter().map(|(hh, r)| format!("{hh:.2}:{:.3}", r * 2.0)).collect();
            eprintln!("CIRCLE {name}: c {:.2},{:.2},{:.2} axis {:.3},{:.3},{:.3} Ø {:.3} height {:.2} region {} rings {} · off {:.3} → {:.3}", c.c.x, c.c.y, c.c.z, c.a.x, c.a.y, c.a.z, c.r * 2.0, ce.height, c.region.len(), rings.join(" "), ce.off, ce.after);
        }
        None => eprintln!("CIRCLE {name}: {}", h.st.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default()),
    }
    h.shot(&format!("{name}-2-rings"));
    // another camera to look at it (RECTIFY_LOOK2): the rings, then after /
    // before (undone, redone)
    let look2 = std::env::var("RECTIFY_LOOK2").map(|s| nums(&s)).ok().filter(|v| v.len() == 6);
    let cam = (h.st.camera.target, h.st.camera.dist, h.st.camera.yaw, h.st.camera.pitch);
    if let Some(v) = &look2 {
        look(h, [v[0], v[1], v[2]], v[3], v[4], v[5]);
        h.shot(&format!("{name}-2-rings-2"));
        h.st.camera.target = cam.0;
        h.st.camera.dist = cam.1;
        h.st.camera.yaw = cam.2;
        h.st.camera.pitch = cam.3;
    }
    let dump = h.st.circle.as_ref().map(|ce| ce.circle.clone());
    if h.st.circle.is_some() {
        h.st.circle_apply();
        if let (Ok(path), Some(c), Some(d)) = (std::env::var("RECTIFY_CIRCLE_DUMP"), &dump, &h.st.doc) {
            let (u, v) = c.basis();
            let mut out = String::from("h0,r0,h1,r1,t\n");
            for &vx in &c.region {
                let (x0, x1) = (d.original.pos[vx as usize], d.mesh.pos[vx as usize]);
                let (h0, r0, _) = c.local(x0);
                let (h1, r1, _) = c.local(x1);
                let q = x0 - c.c;
                let t = q.dot(v).atan2(q.dot(u));
                out += &format!("{h0:.4},{r0:.4},{h1:.4},{r1:.4},{t:.4}\n");
            }
            std::fs::write(path, out).ok();
        }
        if let Some(t) = &h.st.toast {
            eprintln!("{}", t.text);
        }
        turned(&h.st);
        h.shot(&format!("{name}-3-done"));
    }
    h.st.bare = true;
    h.shot(&format!("{name}-4-after"));
    h.st.settings.zebra = true;
    h.shot(&format!("{name}-4-zebra"));
    h.st.settings.zebra = false;
    if let Some(v) = &look2 {
        look(h, [v[0], v[1], v[2]], v[3], v[4], v[5]);
        h.shot(&format!("{name}-5-after-2"));
        h.st.settings.zebra = true;
        h.shot(&format!("{name}-5-after-2-zebra"));
        h.st.undo();
        h.shot(&format!("{name}-5-before-2-zebra"));
        h.st.settings.zebra = false;
        h.shot(&format!("{name}-5-before-2"));
        h.st.redo();
    }
    h.st.bare = false;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).cloned().unwrap_or_else(|| "shots".into());
    let mesh = args.get(2).expect("MESH.obj");
    let filters: Vec<String> = args.iter().skip(3).cloned().collect();
    let want = |name: &str| filters.is_empty() || filters.iter().any(|f| name.starts_with(f.as_str()));
    let mut h = Harness::new(&out);
    let t = std::time::Instant::now();
    let m = Mesh::load_obj(mesh.as_ref()).expect("mesh");
    let name = std::path::Path::new(mesh).file_stem().unwrap().to_string_lossy().to_string();
    h.st.set_mesh(m, &name);
    eprintln!("loaded in {:.1}s", t.elapsed().as_secs_f64());
    h.steps(3);
    if want("ui") {
        h.st.camera.preset(ViewPreset::Perspective);
        h.st.frame_all();
        h.shot("ui-0-loaded");
        h.st.set_tool(Tool::Line);
        h.shot("ui-1-line");
        h.st.set_tool(Tool::Navigate);
    }
    if let Some(w) = std::env::var("RECTIFY_WIDTH").ok().and_then(|s| s.parse().ok()) {
        h.st.line.width = w;
    }
    if want("mirror") {
        h.st.camera.preset(ViewPreset::Perspective);
        h.st.frame_all();
        h.st.set_tool(Tool::Mirror);
        if let Some(m) = std::env::var("RECTIFY_MODE").ok() {
            use rectify_core::sym::SymMode;
            h.st.sym.mode = match m.as_str() {
                "plus" => SymMode::PlusToMinus,
                "minus" => SymMode::MinusToPlus,
                _ => SymMode::Average,
            };
        }
        h.shot("mirror-0-plane");
        let v = std::env::var("RECTIFY_LOOK").map(|s| nums(&s)).unwrap_or_default();
        let v = if v.len() == 6 { v } else { vec![0.3, 0.72, 0.5, 60.0, 0.35, 0.1] };
        look(&mut h, [v[0], v[1], v[2]], v[3], v[4], v[5]);
        h.st.bare = true;
        h.shot("mirror-1-before");
        h.st.settings.zebra = true;
        h.shot("mirror-1-zebra");
        h.st.settings.zebra = false;
        h.st.camera.yaw += std::f32::consts::PI;
        h.shot("mirror-3-other-before");
        h.st.settings.zebra = true;
        h.shot("mirror-3-other-zebra-before");
        h.st.settings.zebra = false;
        h.st.camera.yaw -= std::f32::consts::PI;
        let t = std::time::Instant::now();
        h.st.apply_mirror();
        h.wait();
        eprintln!("mirror applied in {:.1}s", t.elapsed().as_secs_f64());
        if let Some(t) = &h.st.toast {
            eprintln!("{}", t.text);
        }
        h.shot("mirror-2-after");
        h.st.settings.zebra = true;
        h.shot("mirror-2-zebra");
        h.st.settings.zebra = false;
        // the other side
        h.st.camera.yaw += std::f32::consts::PI;
        h.shot("mirror-3-other-after");
        h.st.settings.zebra = true;
        h.shot("mirror-3-other-zebra-after");
        h.st.settings.zebra = false;
        h.st.bare = false;
        h.st.camera.preset(ViewPreset::Perspective);
        h.st.frame_all();
        h.shot("mirror-4-ui");
        h.st.set_tool(Tool::Navigate);
    }
    if want("edges") {
        h.st.camera.preset(ViewPreset::Perspective);
        h.st.frame_all();
        let t = std::time::Instant::now();
        h.st.set_tool(Tool::Edges);
        eprintln!("edges found and fixed in {:.1}s", t.elapsed().as_secs_f64());
        if let Some(ee) = &h.st.edges {
            eprintln!("EDGES: {} found, {} fixed, {} vertices, {:.2} mm at most", ee.found.len(), ee.fits.iter().filter(|f| f.is_some()).count(), ee.moves.idx.len(), ee.moves.max_move());
        }
        h.shot("edges-0-ui");
        look(&mut h, [0.45, 0.72, 0.5], 75.0, 0.3, 0.15);
        h.shot("edges-0-ui-side");
        look(&mut h, [0.85, 0.75, 0.5], 60.0, 0.9, 0.3);
        h.shot("edges-0-ui-front");
        // a click on an edge leaves it out
        let first = h.st.edges.as_ref().and_then(|ee| ee.fits.iter().position(|f| f.is_some()).map(|i| ee.found[i].clone()));
        if let Some(f) = first {
            if let Some(p) = h.st.view().project(((f.a + f.b) * 0.5).as_vec3()) {
                h.click(Pos2::new(p[0], p[1]));
                h.shot("edges-0-ui-one-off");
                h.click(Pos2::new(p[0], p[1]));
            }
        }
        h.st.edges_apply();
        if let Some(t) = &h.st.toast {
            eprintln!("{}", t.text);
        }
        turned(&h.st);
        let looks: Vec<(String, Vec<f32>)> = match std::env::var("RECTIFY_LOOK").map(|s| nums(&s)) {
            Ok(v) if v.len() == 6 => vec![("look".into(), v)],
            _ => vec![
                ("side".into(), vec![0.3, 0.72, 0.5, 60.0, 0.35, 0.1]),
                ("front".into(), vec![0.85, 0.75, 0.5, 60.0, 0.9, 0.3]),
                ("back".into(), vec![0.1, 0.6, 0.5, 70.0, -0.9, 0.35]),
            ],
        };
        h.st.set_tool(Tool::Navigate);
        h.st.bare = true;
        for (name, v) in &looks {
            look(&mut h, [v[0], v[1], v[2]], v[3], v[4], v[5]);
            h.shot(&format!("edges-{name}-after"));
            h.st.settings.zebra = true;
            h.shot(&format!("edges-{name}-after-zebra"));
            h.st.undo();
            h.shot(&format!("edges-{name}-before-zebra"));
            h.st.settings.zebra = false;
            h.shot(&format!("edges-{name}-before"));
            h.st.redo();
        }
        h.st.bare = false;
    }
    if want("look") {
        let v = std::env::var("RECTIFY_LOOK").map(|s| nums(&s)).unwrap_or_default();
        let v = if v.len() == 6 { v } else { vec![0.3, 0.72, 0.5, 60.0, 0.35, 0.1] };
        look(&mut h, [v[0], v[1], v[2]], v[3], v[4], v[5]);
        let pts_of = |s: String| -> Vec<[f32; 2]> { s.split(';').map(nums).filter(|v| v.len() == 2).map(|v| [v[0], v[1]]).collect() };
        if let Ok(s) = std::env::var("RECTIFY_CIRCLE") {
            let tag = std::env::var("RECTIFY_TAG").unwrap_or_else(|_| "circle".into());
            circle_drag(&mut h, &tag, &pts_of(s));
            return;
        }
        match std::env::var("RECTIFY_DRAG") {
            Ok(s) => {
                let pts: Vec<[f32; 2]> = s.split(';').map(nums).filter(|v| v.len() == 2).map(|v| [v[0], v[1]]).collect();
                let tag = std::env::var("RECTIFY_TAG").unwrap_or_else(|_| "look".into());
                line_drag(&mut h, &tag, &pts);
            }
            Err(_) => {
                h.st.bare = true;
                h.shot("look");
                h.st.bare = false;
            }
        }
    }
}
