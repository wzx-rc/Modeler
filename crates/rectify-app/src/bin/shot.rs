#![allow(deprecated)]
//! forme-rectify-shot: runs the real app headless (egui + wgpu, a software
//! GPU is fine) on a mesh, runs the steps and saves PNGs.
//!
//! cargo run --release -p rectify-app --bin forme-rectify-shot -- OUT_DIR MESH.obj [scenario prefix…]

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect};
use glam::Vec3;
use rectify_app::camera::ViewPreset;
use rectify_app::draw::Drawer;
use rectify_app::state::{AppState, ViewMode};
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
        self.drawer.render(&self.gpu.device, &self.gpu.queue, &view, [w, h], self.ppp, &self.st, &paint, &mut tex);
        let px = self.gpu.read_rgba(&target);
        let path = format!("{}/{}.png", self.out, name);
        if let Some(dir) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(dir).ok();
        }
        image::save_buffer(&path, &px, w, h, image::ExtendedColorType::Rgba8).expect("png");
        eprintln!("{path}");
    }

    /// The same view in the three modes (`bare`: no interface).
    fn trio(&mut self, name: &str, bare: bool) {
        self.st.bare = bare;
        for m in [ViewMode::Original, ViewMode::Regions, ViewMode::Result] {
            self.st.set_mode(m);
            // the result without the lines: the surface itself
            self.st.settings.edges = m != ViewMode::Result;
            self.st.gen += 1;
            self.shot(&format!("{name}-{}", m.label().to_lowercase()));
        }
        self.st.settings.edges = true;
        self.st.gen += 1;
        self.st.bare = false;
    }
}

/// Camera on a point of the mesh (fractions of its box), at a distance.
fn look(h: &mut Harness, frac: [f32; 3], dist: f32, yaw: f32, pitch: f32) {
    let (lo, hi) = h.st.pipe.as_ref().unwrap().original.bounds();
    let (lo, hi) = (lo.as_vec3(), hi.as_vec3());
    let c = lo + (hi - lo) * Vec3::from(frac);
    h.st.camera.target = c;
    h.st.camera.dist = dist;
    h.st.camera.yaw = yaw;
    h.st.camera.pitch = pitch;
    h.st.camera.ortho = false;
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
    }
    // RECTIFY_PARAMS="fade=2,flatten=0.3,tol=0.3,…": settings to try
    if let Ok(s) = std::env::var("RECTIFY_PARAMS") {
        for k in h.st.params.apply(&s) {
            eprintln!("unknown setting {k}");
        }
    }
    h.st.run_all();
    h.wait();
    eprintln!("steps done in {:.1}s", t.elapsed().as_secs_f64());
    if want("ui") {
        h.st.frame_all();
        h.st.camera.preset(ViewPreset::Perspective);
        h.st.step = rectify_core::Step::Segment;
        h.st.set_mode(ViewMode::Regions);
        h.shot("ui-1-regions");
        h.st.step = rectify_core::Step::Snap;
        h.st.set_mode(ViewMode::Result);
        h.shot("ui-2-result");
        // a region picked
        let c = h.st.principal();
        h.st.set_mode(ViewMode::Regions);
        h.click(egui::pos2(c[0] - 40.0, c[1] - 30.0));
        h.shot("ui-3-picked");
        h.st.selected = None;
    }
    if want("side") {
        h.st.frame_all();
        h.st.camera.preset(ViewPreset::Front);
        h.st.camera.dist *= 0.62;
        h.trio("side", true);
    }
    if want("persp") {
        h.st.frame_all();
        h.st.camera.dist *= 0.62;
        h.st.camera.yaw = 0.55;
        h.st.camera.pitch = 0.22;
        h.st.camera.ortho = false;
        h.trio("persp", true);
    }
    if want("back") {
        h.st.frame_all();
        h.st.camera.dist *= 0.62;
        h.st.camera.yaw = 0.55 + std::f32::consts::PI;
        h.st.camera.pitch = 0.22;
        h.st.camera.ortho = false;
        h.trio("back", true);
    }
    if want("zebra") {
        h.st.settings.zebra = true;
        h.st.frame_all();
        h.st.camera.dist *= 0.62;
        h.st.camera.yaw = 0.55;
        h.st.camera.pitch = 0.22;
        h.st.camera.ortho = false;
        h.trio("zebra-persp", true);
        look(&mut h, [0.3, 0.72, 0.5], 60.0, 0.35, 0.1);
        h.trio("zebra-vents", true);
        h.st.settings.zebra = false;
    }
    if want("tools") {
        use rectify_app::state::{BrushMode, Tool};
        use rectify_core::Kind;
        look(&mut h, [0.3, 0.72, 0.5], 60.0, 0.35, 0.1);
        h.st.bare = false;
        h.st.set_mode(ViewMode::Regions);
        h.st.settings.edges = true;
        h.st.gen += 1;
        h.steps(3);
        h.shot("tools-0-regions");
        // strokes in screen points (this camera, the interface shown)
        let line = |a: [f32; 2], b: [f32; 2], n: usize| -> Vec<[f32; 2]> { (0..=n).map(|i| { let t = i as f32 / n as f32; [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t] }).collect() };
        // the big panel cut across, then joined again
        h.st.set_tool(Tool::Brush(BrushMode::Split));
        h.st.brush = 8.0;
        h.drag_screen(&line([500.0, 160.0], [956.0, 160.0], 8), true);
        h.shot("tools-1-split-stroke");
        h.release();
        h.shot("tools-2-split");
        h.st.set_tool(Tool::Brush(BrushMode::Merge));
        h.st.brush = 12.0;
        h.drag_screen(&line([800.0, 100.0], [800.0, 230.0], 4), true);
        h.shot("tools-3-merge-stroke");
        h.release();
        h.shot("tools-4-merged");
        // the pieces of the round button's ring joined
        h.drag_screen(&[[608.0, 344.0], [680.0, 248.0], [776.0, 256.0], [848.0, 360.0], [824.0, 480.0]], false);
        h.shot("tools-5-ring-merged");
        // a cylinder painted along the pipe under the panel
        h.st.set_tool(Tool::Shape(Kind::Cylinder));
        h.st.brush = 14.0;
        h.drag_screen(&line([520.0, 722.0], [990.0, 722.0], 6), true);
        h.shot("tools-6-shape-stroke");
        h.release();
        h.shot("tools-7-shape");
        // the result follows when it is shown
        h.st.set_tool(Tool::Select);
        h.st.set_mode(ViewMode::Result);
        h.steps(2);
        h.wait();
        h.st.bare = true;
        h.st.settings.edges = false;
        h.st.gen += 1;
        h.shot("tools-8-result");
        h.st.settings.edges = true;
        h.st.bare = false;
    }
    let close: [(&str, [f32; 3], f32, f32, f32); 3] = [("close-muzzle", [0.1, 0.72, 0.5], 70.0, -0.9, 0.15), ("close-vents", [0.3, 0.72, 0.5], 60.0, 0.35, 0.1), ("close-grip", [0.65, 0.35, 0.5], 75.0, 0.4, 0.05)];
    for (n, f, d, y, p) in close {
        if want(n) {
            look(&mut h, f, d, y, p);
            // RECTIFY_PICK="x,y;x,y": the world points under these screen points
            if let Ok(s) = std::env::var("RECTIFY_PICK") {
                h.st.bare = true;
                h.steps(2);
                for xy in s.split(';') {
                    let v: Vec<f32> = xy.split(',').filter_map(|t| t.trim().parse().ok()).collect();
                    if v.len() == 2 {
                        if let Some(t) = h.st.pick([v[0], v[1]]) {
                            let m = h.st.pipe.as_ref().unwrap().original.tri_centre(t);
                            eprintln!("PICK {n} {xy}: tri {t} at {:.3} {:.3} {:.3}", m.x, m.y, m.z);
                        }
                    }
                }
            }
            h.trio(n, true);
        }
    }
}
