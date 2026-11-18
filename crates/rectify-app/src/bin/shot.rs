#![allow(deprecated)]
//! forme-rectify-shot: runs the real app headless (egui + wgpu, a software
//! GPU is fine) on a mesh, drives its tools with real mouse and key events
//! and saves PNGs.
//!
//! cargo run --release -p rectify-app --bin forme-rectify-shot -- OUT_DIR MESH.obj [scenario prefix…]
//!
//! RECTIFY_DO="cmd args | cmd args | …": a script, screen points of the
//! view with the interface (a capture's pixels / 2):
//! `look fx,fy,fz,dist,yaw,pitch` (the camera: a point of the mesh's box in
//! fractions; `cam x,y,z,dist,yaw,pitch`: a point in mm), `tool q|p|c|s|b`, `mode 1|2|3|4`, `click x,y …` (`sclick`
//! with Shift, `cclick` with Ctrl), `drag x,y x,y …` (`sdrag`, `cdrag`;
//! `hold`, `shold`: not released, `release`),
//! `hover x,y` (`shover`), `key enter|esc|back|del`, `corner r`, `fillet
//! r`, `thick t`, `shot NAME` (with the interface), `bare NAME` (the view
//! alone), `alone NAME` (the shapes alone), `print` (the shapes, what is
//! picked).
//! RECTIFY_LOOK="fx,fy,fz,dist,yaw,pitch": the camera of the `look`
//! scenario. RECTIFY_SLOT="x,y;x,y": a Slot drag (end to end, screen points
//! of the bare view), with RECTIFY_DEPTH, RECTIFY_LEVELS,
//! RECTIFY_PROFILE=straight|tapered|curve; RECTIFY_BOX the same with the
//! Box tool. Several drags separated by `|` are each kept and shown
//! together, RECTIFY_TOOLS="b,s,…" the tool of each.

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect};
use glam::Vec3;
use rectify_app::camera::ViewPreset;
use rectify_app::draw::Drawer;
use rectify_app::state::{AppState, SelMode, Tool};
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
    /// The keys held (Shift, Ctrl).
    mods: Modifiers,
}

impl Harness {
    fn new(out: &str) -> Harness {
        let gpu = Gpu::new();
        let drawer = Drawer::new(&gpu.device, FMT);
        let ctx = egui::Context::default();
        theme::install(&ctx);
        Harness { gpu, drawer, ctx, st: AppState::new(), t: 0.0, size: [1600.0, 900.0], ppp: 2.0, pending: vec![], out: out.to_string(), pending_tex: Default::default(), mods: Modifiers::NONE }
    }

    fn raw(&mut self) -> RawInput {
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(self.size[0], self.size[1]))),
            time: Some(self.t),
            predicted_dt: 1.0 / 60.0,
            events: std::iter::once(Event::ModifiersChanged(self.mods)).chain(std::mem::take(&mut self.pending)).collect(),
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

    fn hover(&mut self, p: Pos2) {
        self.pending.push(Event::PointerMoved(p));
        self.steps(2);
    }

    fn click(&mut self, p: Pos2) {
        self.pending.push(Event::PointerMoved(p));
        self.step();
        for down in [true, false] {
            self.pending.push(Event::PointerMoved(p));
            self.pending.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: down, modifiers: self.mods });
            self.step();
        }
        self.steps(2);
    }

    /// A left-button drag through screen points; with `hold`, the button
    /// stays down at the end.
    fn drag_screen(&mut self, pts: &[[f32; 2]], hold: bool) {
        let scr: Vec<Pos2> = pts.iter().map(|a| Pos2::new(a[0], a[1])).collect();
        let Some(&first) = scr.first() else { return };
        self.pending.push(Event::PointerMoved(first));
        self.step();
        self.pending.push(Event::PointerButton { pos: first, button: PointerButton::Primary, pressed: true, modifiers: self.mods });
        self.step();
        for w in scr.windows(2) {
            for k in 1..=4 {
                let t = k as f32 / 4.0;
                self.pending.push(Event::PointerMoved(w[0] + (w[1] - w[0]) * t));
                self.step();
            }
        }
        if !hold {
            let last = *scr.last().unwrap();
            self.pending.push(Event::PointerButton { pos: last, button: PointerButton::Primary, pressed: false, modifiers: self.mods });
            self.step();
            self.steps(2);
        }
    }

    fn release(&mut self) {
        let p = self.ctx.input(|i| i.pointer.latest_pos()).unwrap_or(Pos2::ZERO);
        self.pending.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: self.mods });
        self.step();
        self.steps(2);
    }

    fn key(&mut self, key: egui::Key) {
        for pressed in [true, false] {
            self.pending.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: self.mods });
            self.step();
        }
        self.steps(1);
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
    let (lo, hi) = h.st.doc.as_ref().unwrap().mesh.bounds();
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

/// A Slot drag along a slot, a Box drag across a block, a Circle drag
/// across a round part (screen points of the bare view): the surface
/// placed, alone, from a second camera, kept.
fn slot_drag(h: &mut Harness, name: &str, pts: &[[f32; 2]], tool: Tool) {
    use rectify_core::circle::ProfileKind;
    h.st.set_tool(Tool::Select);
    h.st.bare = true;
    h.shot(&format!("{name}-0-before"));
    let world: Vec<glam::Vec3> = pts.iter().filter_map(|p| h.st.pick_hit(*p).map(|(_, x)| x.as_vec3())).collect();
    h.st.bare = false;
    h.st.set_tool(tool);
    let profile = match std::env::var("RECTIFY_PROFILE").as_deref() {
        Ok("tapered" | "cone") => Some(ProfileKind::Cone),
        Ok("curve") => Some(ProfileKind::Curve),
        Ok("straight" | "cylinder") => Some(ProfileKind::Cylinder),
        _ => None,
    };
    if let Some(k) = profile {
        h.st.slot_profile = k;
    }
    h.steps(2);
    let view = h.st.view();
    let scr: Vec<[f32; 2]> = world.iter().filter_map(|x| view.project(*x)).collect();
    h.drag_screen(&scr, true);
    h.shot(&format!("{name}-1-drag"));
    let t = std::time::Instant::now();
    h.release();
    eprintln!("slot found and laid in {:.2}s", t.elapsed().as_secs_f64());
    let env = |k: &str| std::env::var(k).ok().and_then(|s| s.parse::<f64>().ok());
    if let Some(se) = &mut h.st.slot {
        if let Some(l) = env("RECTIFY_LEVELS") {
            se.params.levels = l as usize;
        }
        if let Some(d) = env("RECTIFY_DEPTH").or_else(|| env("RECTIFY_HEIGHT")) {
            se.params.depth = d;
        }
    }
    h.st.slot_refit();
    match &h.st.slot {
        Some(se) => {
            let sl = &se.slot;
            let s = &se.laid.surface;
            eprintln!(
                "SLOT {name}: A {:.2},{:.2},{:.2} B {:.2},{:.2},{:.2} axis {:.3},{:.3},{:.3} · {:.3} × {:.3} r {:.3} · depth {:.2} (found {:.2}) {:?} levels {} · degree 2x{} {}x{} · {} points · off {:.3} · inward {}",
                sl.a.x, sl.a.y, sl.a.z, sl.b.x, sl.b.y, sl.b.z, sl.axis.x, sl.axis.y, sl.axis.z, sl.length(), sl.width(), sl.r, se.params.depth, sl.depth, se.params.profile, se.params.levels, s.dv, s.nu, s.nv, se.laid.points, se.laid.off, sl.inward
            );
        }
        None => eprintln!("SLOT {name}: {}", h.st.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default()),
    }
    h.shot(&format!("{name}-2-placed"));
    let views = |h: &mut Harness, tag: &str| {
        h.st.bare = true;
        h.shot(&format!("{name}-{tag}"));
        h.st.hide_scan = true;
        h.shot(&format!("{name}-{tag}-alone"));
        h.st.hide_scan = false;
        h.st.bare = false;
    };
    views(h, "3-surface");
    let look2 = std::env::var("RECTIFY_LOOK2").map(|s| nums(&s)).ok().filter(|v| v.len() == 6);
    if let Some(v) = &look2 {
        look(h, [v[0], v[1], v[2]], v[3], v[4], v[5]);
        h.shot(&format!("{name}-2-placed-2"));
        views(h, "3-surface-2");
    }
    if h.st.slot.is_some() {
        h.st.slot_apply();
        if let Some(t) = &h.st.toast {
            eprintln!("{}", t.text);
        }
        h.shot(&format!("{name}-4-kept"));
    }
}

/// Several Slot drags ("x,y;x,y|x,y;x,y…"), each kept: all of them shown
/// at the end (and from RECTIFY_LOOK2).
fn slot_many(h: &mut Harness, name: &str, s: &str, tools: &[Tool]) {
    h.st.set_tool(Tool::Select);
    h.st.bare = true;
    h.shot(&format!("{name}-0-before"));
    let drags: Vec<Vec<glam::Vec3>> = s.split('|').map(|d| d.split(';').map(nums).filter(|v| v.len() == 2).filter_map(|v| h.st.pick_hit([v[0], v[1]]).map(|(_, x)| x.as_vec3())).collect()).collect();
    h.st.bare = false;
    for (k, w) in drags.iter().enumerate() {
        h.st.set_tool(tools[k.min(tools.len() - 1)]);
        h.steps(1);
        let view = h.st.view();
        let scr: Vec<[f32; 2]> = w.iter().filter_map(|x| view.project(*x)).collect();
        h.drag_screen(&scr, true);
        h.release();
        match &h.st.slot {
            Some(se) => eprintln!("{:?} {name} #{k}: {:.3} × {:.3} r {:.3} · depth {:.2} inward {} {:?} levels {} · {} points · off {:.3}", se.slot.kind, se.slot.length(), se.slot.width(), se.slot.r, se.params.depth, se.slot.inward, se.params.profile, se.params.levels, se.laid.points, se.laid.off),
            None => eprintln!("{:?} {name} #{k}: {}", h.st.tool, h.st.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default()),
        }
        h.st.slot_apply();
        h.st.set_tool(Tool::Select);
    }
    h.shot(&format!("{name}-1-kept"));
    let views = |h: &mut Harness, tag: &str| {
        h.st.bare = true;
        h.shot(&format!("{name}-{tag}"));
        h.st.hide_scan = true;
        h.shot(&format!("{name}-{tag}-alone"));
        h.st.hide_scan = false;
        h.st.bare = false;
    };
    views(h, "2-view");
    if let Some(v) = std::env::var("RECTIFY_LOOK2").map(|s| nums(&s)).ok().filter(|v| v.len() == 6) {
        look(h, [v[0], v[1], v[2]], v[3], v[4], v[5]);
        h.shot(&format!("{name}-1-kept-2"));
        views(h, "2-view-2");
    }
}

/// The shapes and what is picked, printed.
fn print_state(h: &Harness, what: &str) {
    eprintln!("== {what}: tool {:?}, mode {:?}, {} shapes, picked {:?}", h.st.tool, h.st.mode, h.st.objects.len(), h.st.sel);
    for (i, o) in h.st.objects.iter().enumerate() {
        match o.body() {
            Some(b) => eprintln!(
                "  {} {:?}: {} · {} faces, {} edges, {} vertices, size {:.2}{}{}",
                i + 1,
                o.tool,
                o.what,
                b.built.topo.faces.len(),
                b.built.topo.edges.len(),
                b.built.verts.len(),
                b.built.size,
                if b.built.solid() { " · solid" } else { "" },
                b.built.failed.as_ref().map(|f| format!(" · FAILED step {}: {}", f.0, f.1)).unwrap_or_default()
            ),
            None => eprintln!("  {} {:?}: {}", i + 1, o.tool, o.what),
        }
    }
    if let Some(pe) = &h.st.pen {
        eprintln!("  pen: {} points on {} plane n {:.3},{:.3},{:.3} o {:.2},{:.2},{:.2}", pe.pts.len(), if pe.flat { "a flat" } else { "the tangent" }, pe.frame.n.x, pe.frame.n.y, pe.frame.n.z, pe.frame.o.x, pe.frame.o.y, pe.frame.o.z);
        let on: Vec<String> = pe.pts.iter().map(|q| h.st.project(pe.frame.to3(*q)).map(|s| format!("{:.0},{:.0}", s[0], s[1])).unwrap_or_default()).collect();
        eprintln!("  pen points on screen: {}", on.join(" "));
    }
    if let Some(hv) = &h.st.pen_hover {
        eprintln!("  pen hover: {:?} at {:.2},{:.2},{:.2}", hv.snap, hv.p.x, hv.p.y, hv.p.z);
    }
    if let Some(pl) = &h.st.pen_plane {
        eprintln!("  plane under the pointer: {} n {:.3},{:.3},{:.3}", if pl.flat { "flat" } else { "tangent" }, pl.frame.n.x, pl.frame.n.y, pl.frame.n.z);
    }
    if let Some(t) = &h.st.toast {
        eprintln!("  toast: {}", t.text);
    }
}

/// Runs a RECTIFY_DO script (see the top of this file).
fn script(h: &mut Harness, s: &str) {
    let shift = Modifiers { shift: true, ..Modifiers::NONE };
    let ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    for cmd in s.split('|').map(|c| c.trim()).filter(|c| !c.is_empty()) {
        let mut it = cmd.split_whitespace();
        let op = it.next().unwrap_or("");
        let args: Vec<&str> = it.collect();
        let pts: Vec<[f32; 2]> = args.iter().map(|a| nums(a)).filter(|v| v.len() == 2).map(|v| [v[0], v[1]]).collect();
        let num = args.first().and_then(|a| a.parse::<f64>().ok()).unwrap_or(0.0);
        let t0 = std::time::Instant::now();
        let mods = match op {
            "sclick" | "shover" | "sdrag" | "shold" => shift,
            "cclick" | "cdrag" => ctrl,
            _ => Modifiers::NONE,
        };
        h.mods = mods;
        h.steps(1);
        match op {
            "look" => {
                let v = nums(args.first().copied().unwrap_or(""));
                if v.len() == 6 {
                    look(h, [v[0], v[1], v[2]], v[3], v[4], v[5]);
                    h.steps(2);
                }
            }
            "cam" => {
                let v = nums(args.first().copied().unwrap_or(""));
                if v.len() == 6 {
                    h.st.camera.target = Vec3::new(v[0], v[1], v[2]);
                    h.st.camera.dist = v[3];
                    h.st.camera.yaw = v[4];
                    h.st.camera.pitch = v[5];
                    h.steps(2);
                }
            }
            "tool" => {
                let t = match args.first().copied().unwrap_or("q") {
                    "p" => Tool::Pen,
                    "c" => Tool::Circle,
                    "s" => Tool::Slot,
                    "b" => Tool::Box,
                    _ => Tool::Select,
                };
                h.st.set_tool(t);
                h.steps(2);
            }
            "mode" => {
                let m = match num as i32 {
                    1 => SelMode::Vertex,
                    2 => SelMode::Edge,
                    3 => SelMode::Face,
                    _ => SelMode::Object,
                };
                h.st.set_mode(m);
                h.steps(2);
            }
            "click" | "sclick" | "cclick" => {
                for p in &pts {
                    h.click(Pos2::new(p[0], p[1]));
                }
            }
            "hover" | "shover" => {
                for p in &pts {
                    h.hover(Pos2::new(p[0], p[1]));
                }
            }
            "drag" | "sdrag" | "cdrag" => h.drag_screen(&pts, false),
            "hold" | "shold" => h.drag_screen(&pts, true),
            "release" => {
                h.release();
                h.steps(2);
            }
            "key" => {
                let k = match args.first().copied().unwrap_or("") {
                    "enter" => egui::Key::Enter,
                    "esc" => egui::Key::Escape,
                    "back" => egui::Key::Backspace,
                    "del" => egui::Key::Delete,
                    other => {
                        eprintln!("unknown key {other}");
                        continue;
                    }
                };
                h.key(k);
            }
            "corner" => {
                h.st.set_corners(num, false);
                h.steps(2);
            }
            "fillet" => {
                h.st.make_fillet(num);
                h.st.end_live();
                h.steps(2);
            }
            "thick" => {
                if let Some(o) = h.st.sel.first().map(|c| c.obj()) {
                    h.st.set_thickness(o, num, false);
                }
                h.steps(2);
            }
            "shot" => h.shot(args.first().copied().unwrap_or("x")),
            "bare" | "alone" => {
                h.st.bare = true;
                h.st.hide_scan = op == "alone";
                h.shot(args.first().copied().unwrap_or("x"));
                h.st.bare = false;
                h.st.hide_scan = false;
                h.steps(2);
            }
            "print" => print_state(h, args.first().copied().unwrap_or("")),
            _ => eprintln!("unknown command: {cmd}"),
        }
        h.mods = Modifiers::NONE;
        eprintln!("[{cmd}] {:.2}s", t0.elapsed().as_secs_f64());
    }
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
    if let Ok(s) = std::env::var("RECTIFY_DO") {
        script(&mut h, &s);
        return;
    }
    if want("ui") {
        h.st.camera.preset(ViewPreset::Perspective);
        h.st.frame_all();
        h.shot("ui-0-loaded");
    }
    if want("look") {
        let v = std::env::var("RECTIFY_LOOK").map(|s| nums(&s)).unwrap_or_default();
        let v = if v.len() == 6 { v } else { vec![0.3, 0.72, 0.5, 60.0, 0.35, 0.1] };
        look(&mut h, [v[0], v[1], v[2]], v[3], v[4], v[5]);
        // RECTIFY_SLOT / RECTIFY_BOX: one drag, or several separated by |
        // (RECTIFY_TOOLS="s,b,…" picks the tool of each)
        let (var, tool) = if std::env::var("RECTIFY_BOX").is_ok() { ("RECTIFY_BOX", Tool::Box) } else { ("RECTIFY_SLOT", Tool::Slot) };
        if let Ok(s) = std::env::var(var).map(|s| s.replace(' ', "")) {
            if s.contains('|') {
                let tools: Vec<Tool> = match std::env::var("RECTIFY_TOOLS") {
                    Ok(t) => t.split(',').map(|k| if k.trim() == "b" { Tool::Box } else { Tool::Slot }).collect(),
                    Err(_) => vec![tool],
                };
                let tag = std::env::var("RECTIFY_TAG").unwrap_or_else(|_| "slots".into());
                slot_many(&mut h, &tag, &s, &tools);
                return;
            }
            let pts: Vec<[f32; 2]> = s.split(';').map(nums).filter(|v| v.len() == 2).map(|v| [v[0], v[1]]).collect();
            let tag = std::env::var("RECTIFY_TAG").unwrap_or_else(|_| if tool == Tool::Box { "box".into() } else { "slot".into() });
            slot_drag(&mut h, &tag, &pts, tool);
            return;
        }
        h.shot("look-ui");
        h.st.bare = true;
        h.shot("look");
        h.st.bare = false;
    }
}
