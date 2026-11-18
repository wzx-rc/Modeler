#![allow(deprecated)]
//! forme-rectify-shot: runs the real app headless (egui + wgpu, a software
//! GPU is fine) on a mesh, drives its tools with real mouse events and saves
//! PNGs.
//!
//! cargo run --release -p rectify-app --bin forme-rectify-shot -- OUT_DIR MESH.obj [scenario prefix…]
//!
//! RECTIFY_LOOK="fx,fy,fz,dist,yaw,pitch": the camera of the `look`
//! scenario (a point of the mesh's box in fractions); RECTIFY_CIRCLE=
//! "x,y;x,y": a Circle drag there (rim to rim, screen points of the bare
//! view), the NURBS surface shown and kept (RECTIFY_HEIGHT, RECTIFY_LEVELS,
//! RECTIFY_PROFILE=cylinder|cone|curve to set it first, RECTIFY_LOOK2 a
//! second camera, RECTIFY_TAG the file names). RECTIFY_PATH="x,y;x,y;…"
//! (add ";close" to close it): Path points clicked there (screen points of
//! the bare view), RECTIFY_SMOOTH, RECTIFY_PLANES=0; RECTIFY_HOVER="x,y;…":
//! what would catch the pointer there, printed. RECTIFY_SLOT="x,y;x,y": a
//! Slot drag (end to end, screen points of the bare view), with
//! RECTIFY_DEPTH, RECTIFY_LEVELS, RECTIFY_PROFILE=straight|tapered|curve;
//! RECTIFY_BOX the same with the Box tool. Several drags separated by `|`
//! are each kept and shown together, RECTIFY_TOOLS="b,c,s,…" the tool of
//! each (Box, Circle, Slot). RECTIFY_LINE="x,y;x,y;…|…": pencil strokes
//! with the Line tool (each a polyline through those points), then
//! RECTIFY_PATCH="0;1;…" the paths kept clicked with the Patch tool (their
//! places in the list), or "x,y" points of the bare view clicked (inside a
//! cell of the paths: its patch, the one before kept).

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

    #[allow(dead_code)]
    fn wait(&mut self) {
        while self.st.job.is_some() {
            self.step();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        self.steps(2);
    }

    fn hover(&mut self, p: Pos2) {
        self.pending.push(Event::PointerMoved(p));
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
    h.st.set_tool(Tool::Navigate);
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
    h.st.set_tool(Tool::Navigate);
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
        h.st.set_tool(Tool::Navigate);
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

/// Pencil strokes with the Line tool ("x,y;x,y;…|…", screen points of the
/// bare view, each stroke a polyline drawn through them at 4-point steps
/// with a slight tremble): each made, set, then kept by the next; all of
/// them shown at the end (and from RECTIFY_LOOK2).
fn line_strokes(h: &mut Harness, name: &str, s: &str) {
    use rectify_core::path::{PieceKind, SnapKind};
    h.st.set_tool(Tool::Navigate);
    h.st.bare = true;
    h.shot(&format!("{name}-0-before"));
    // each stroke densified on screen, then put on the scan
    let strokes: Vec<Vec<glam::Vec3>> = s
        .split('|')
        .map(|d| {
            let pts: Vec<[f32; 2]> = d.split(';').map(nums).filter(|v| v.len() == 2).map(|v| [v[0], v[1]]).collect();
            let mut dense: Vec<[f32; 2]> = vec![];
            let mut k = 0usize;
            for w in pts.windows(2) {
                let l = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
                let n = (l / 4.0).ceil().max(1.0) as usize;
                let (dx, dy) = ((w[1][0] - w[0][0]) / l.max(1e-6), (w[1][1] - w[0][1]) / l.max(1e-6));
                for q in 0..n {
                    let t = q as f32 / n as f32;
                    let tremble = (k as f32 * 0.7).sin() * 0.8;
                    k += 1;
                    dense.push([w[0][0] + (w[1][0] - w[0][0]) * t - dy * tremble, w[0][1] + (w[1][1] - w[0][1]) * t + dx * tremble]);
                }
            }
            if let Some(l) = pts.last() {
                dense.push(*l);
            }
            dense.iter().filter_map(|p| h.st.pick_hit(*p).map(|(_, x)| x.as_vec3())).collect()
        })
        .collect();
    h.st.bare = false;
    h.st.set_tool(Tool::Line);
    h.steps(2);
    for (k, w) in strokes.iter().enumerate() {
        let view = h.st.view();
        let scr: Vec<[f32; 2]> = w.iter().filter_map(|x| view.project(*x)).collect();
        let t = std::time::Instant::now();
        h.drag_screen(&scr, false);
        match &h.st.path {
            Some(pe) => {
                let count = |kd: SnapKind| pe.pts.iter().filter(|s| s.kind == kd && !s.through).count();
                let through = pe.pts.iter().filter(|s| s.through).count();
                let kinds = pe.built.as_ref().map(|b| {
                    let c = |x: PieceKind| b.kinds.iter().filter(|&&y| y == x).count();
                    format!("{} line {} fold {} scan · {:.1} mm · {} ctrl · off {:.3}", c(PieceKind::Line), c(PieceKind::Fold), c(PieceKind::Scan), b.length, b.curve.pts.len(), b.off)
                });
                eprintln!(
                    "LINE {name} #{k}: {} pts ({} corner {} edge {} face {} scan {} joined, {through} through){} · {} · {:.2}s",
                    pe.pts.len(),
                    count(SnapKind::Corner),
                    count(SnapKind::Edge),
                    count(SnapKind::Plane),
                    count(SnapKind::Scan),
                    count(SnapKind::Path),
                    if pe.params.closed { " closed" } else { "" },
                    kinds.unwrap_or_default(),
                    t.elapsed().as_secs_f64()
                );
            }
            None => eprintln!("LINE {name} #{k}: {}", h.st.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default()),
        }
        if k + 1 == strokes.len() {
            h.shot(&format!("{name}-1-last"));
        }
    }
    h.st.path_apply();
    h.shot(&format!("{name}-2-kept"));
    let views = |h: &mut Harness, tag: &str| {
        h.st.bare = true;
        h.shot(&format!("{name}-{tag}"));
        h.st.hide_scan = true;
        h.shot(&format!("{name}-{tag}-alone"));
        h.st.hide_scan = false;
        h.st.bare = false;
    };
    views(h, "3-view");
    // RECTIFY_PATCH="0;1;…": the paths kept (their places in the list)
    // clicked with the Patch tool, at their middle; "x,y" a point of the
    // bare view clicked instead
    let mut tag = "3-view-2";
    if let Ok(s) = std::env::var("RECTIFY_PATCH") {
        h.st.set_tool(Tool::Patch);
        h.steps(2);
        for e in s.split(';').map(|e| e.trim()).filter(|e| !e.is_empty()) {
            let world = if let Ok(i) = e.parse::<usize>() {
                match h.st.objects.get(i).map(|o| &o.shape) {
                    Some(rectify_app::state::Shape::Curve(c)) => {
                        let (a, b) = c.domain();
                        Some(c.eval((a + b) * 0.5).as_vec3())
                    }
                    _ => None,
                }
            } else {
                let v = nums(e);
                h.st.bare = true;
                h.steps(1);
                let w = if v.len() == 2 { h.st.pick_hit([v[0], v[1]]).map(|(_, x)| x.as_vec3()) } else { None };
                h.st.bare = false;
                h.steps(1);
                w
            };
            let Some(x) = world else {
                eprintln!("PATCH {name}: nothing at {e}");
                continue;
            };
            let Some(sp) = h.st.view().project(x) else { continue };
            let on = h.st.curve_at(sp);
            let t = std::time::Instant::now();
            h.click(Pos2::new(sp[0], sp[1]));
            match (&h.st.patch, on) {
                (Some(pe), None) => match &pe.made {
                    Ok(pt) => eprintln!("PATCH {name} inside {e}: paths {:?} · {}x{} spans · off {:.3} · gap {:.3} · {:.2}s", pe.picked, pt.spans[0], pt.spans[1], pt.off, pt.gap, t.elapsed().as_secs_f64()),
                    Err(er) => eprintln!("PATCH {name} inside {e}: paths {:?} · {er}", pe.picked),
                },
                (None, None) => eprintln!("PATCH {name} inside {e}: {}", h.st.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default()),
                (_, Some(i)) => eprintln!("PATCH {name} on path {i}"),
            }
        }
        let t = std::time::Instant::now();
        h.st.patch_rebuild();
        let el = t.elapsed().as_secs_f64();
        match &h.st.patch {
            Some(pe) => match &pe.made {
                Ok(pt) => eprintln!("PATCH {name}: {} paths · {}x{} spans · off {:.3} · gap {:.3} · {:.2}s", pe.picked.len(), pt.spans[0], pt.spans[1], pt.off, pt.gap, el),
                Err(e) => eprintln!("PATCH {name}: {} paths · {e}", pe.picked.len()),
            },
            None => eprintln!("PATCH {name}: no path picked"),
        }
        h.shot(&format!("{name}-4-patch"));
        h.st.patch_apply();
        views(h, "5-view");
        tag = "5-view-2";
    }
    if let Some(v) = std::env::var("RECTIFY_LOOK2").map(|s| nums(&s)).ok().filter(|v| v.len() == 6) {
        look(h, [v[0], v[1], v[2]], v[3], v[4], v[5]);
        views(h, tag);
    }
}

/// Points clicked with the Path tool (screen points of the bare view): the
/// next segment shown before the last click, the curve placed (with the
/// interface, then the view alone), kept.
fn path_clicks(h: &mut Harness, name: &str, pts: &[[f32; 2]], close: bool) {
    h.st.set_tool(Tool::Navigate);
    h.st.bare = true;
    h.shot(&format!("{name}-0-before"));
    let world: Vec<glam::Vec3> = pts.iter().filter_map(|p| h.st.pick_hit(*p).map(|(_, x)| x.as_vec3())).collect();
    h.st.bare = false;
    let env = |k: &str| std::env::var(k).ok().and_then(|s| s.parse::<f64>().ok());
    if let Some(v) = env("RECTIFY_SMOOTH") {
        h.st.path_params.smooth = v;
    }
    if env("RECTIFY_PLANES") == Some(0.0) {
        h.st.path_params.planes = false;
    }
    h.st.set_tool(Tool::Path);
    h.steps(2);
    let view = h.st.view();
    let scr: Vec<Pos2> = world.iter().filter_map(|x| view.project(*x)).map(|a| Pos2::new(a[0], a[1])).collect();
    let t = std::time::Instant::now();
    for (i, &p) in scr.iter().enumerate() {
        h.hover(p);
        if i + 1 == scr.len() && i > 0 {
            h.shot(&format!("{name}-1-next"));
        }
        h.click(p);
    }
    if close && scr.len() >= 3 {
        // on the first point as drawn (where it went)
        let f = h.st.path_screen()[0];
        h.hover(Pos2::new(f[0], f[1]));
        h.click(Pos2::new(f[0], f[1]));
    }
    eprintln!("{} points placed in {:.2}s", scr.len(), t.elapsed().as_secs_f64());
    // the pointer away from the path
    h.hover(Pos2::new(h.st.view_rect[2] - 30.0, h.st.view_rect[3] - 30.0));
    match &h.st.path {
        Some(pe) => {
            for (i, s) in pe.pts.iter().enumerate() {
                eprintln!("  point {i}: {:?} {:.0}° at {:.3},{:.3},{:.3} ({:.3} mm from the click) planes {}", s.kind, s.angle, s.p.x, s.p.y, s.p.z, (s.p - s.hit).length(), s.planes.iter().map(|p| format!("[n {:.2},{:.2},{:.2} rms {:.3} area {:.1}]", p.n.x, p.n.y, p.n.z, p.rms, p.area)).collect::<Vec<_>>().join(" "));
            }
            if let Some(b) = &pe.built {
                eprintln!("PATH {name}: {:?} · {} control points · {:.2} mm · off {:.3} · closed {}", b.kinds, b.curve.pts.len(), b.length, b.off, pe.params.closed);
            }
        }
        None => eprintln!("PATH {name}: none ({})", h.st.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default()),
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
    views(h, "3-path");
    if let Some(v) = std::env::var("RECTIFY_LOOK2").map(|s| nums(&s)).ok().filter(|v| v.len() == 6) {
        let cam = (h.st.camera.target, h.st.camera.dist, h.st.camera.yaw, h.st.camera.pitch);
        look(h, [v[0], v[1], v[2]], v[3], v[4], v[5]);
        h.shot(&format!("{name}-2-placed-2"));
        views(h, "3-path-2");
        h.st.camera.target = cam.0;
        h.st.camera.dist = cam.1;
        h.st.camera.yaw = cam.2;
        h.st.camera.pitch = cam.3;
    }
    if h.st.path.is_some() {
        h.st.path_apply();
        if let Some(t) = &h.st.toast {
            eprintln!("{}", t.text);
        }
        h.shot(&format!("{name}-4-kept"));
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
    if want("ui") {
        h.st.camera.preset(ViewPreset::Perspective);
        h.st.frame_all();
        h.shot("ui-0-loaded");
    }
    if want("look") {
        let v = std::env::var("RECTIFY_LOOK").map(|s| nums(&s)).unwrap_or_default();
        let v = if v.len() == 6 { v } else { vec![0.3, 0.72, 0.5, 60.0, 0.35, 0.1] };
        look(&mut h, [v[0], v[1], v[2]], v[3], v[4], v[5]);
        if let Ok(s) = std::env::var("RECTIFY_HOVER") {
            h.st.bare = true;
            h.steps(2);
            for v in s.split(';').map(nums).filter(|v| v.len() == 2) {
                let t = std::time::Instant::now();
                match h.st.snap_at([v[0], v[1]], false) {
                    Some(sn) => eprintln!("HOVER {},{}: {:?} {:.0}° at {:.3},{:.3},{:.3} ({:.3} mm off) {} planes [{}] in {:.3}s", v[0], v[1], sn.kind, sn.angle, sn.p.x, sn.p.y, sn.p.z, (sn.p - sn.hit).length(), sn.planes.len(), sn.planes.iter().map(|p| format!("n {:.2},{:.2},{:.2} rms {:.3} area {:.1}", p.n.x, p.n.y, p.n.z, p.rms, p.area)).collect::<Vec<_>>().join(" | "), t.elapsed().as_secs_f64()),
                    None => eprintln!("HOVER {},{}: nothing", v[0], v[1]),
                }
            }
            h.st.bare = false;
        }
        if let Ok(s) = std::env::var("RECTIFY_PATH") {
            let close = s.contains("close");
            let pts: Vec<[f32; 2]> = s.split(';').map(nums).filter(|v| v.len() == 2).map(|v| [v[0], v[1]]).collect();
            let tag = std::env::var("RECTIFY_TAG").unwrap_or_else(|_| "path".into());
            path_clicks(&mut h, &tag, &pts, close);
            return;
        }
        if let Ok(s) = std::env::var("RECTIFY_LINE") {
            let tag = std::env::var("RECTIFY_TAG").unwrap_or_else(|_| "line".into());
            line_strokes(&mut h, &tag, &s.replace(' ', ""));
            return;
        }
        // RECTIFY_SLOT / RECTIFY_BOX: one drag, or several separated by |
        // (RECTIFY_TOOLS="s,b,…" picks the tool of each)
        let (var, tool) = if std::env::var("RECTIFY_BOX").is_ok() { ("RECTIFY_BOX", Tool::Box) } else { ("RECTIFY_SLOT", Tool::Slot) };
        if let Ok(s) = std::env::var(var).map(|s| s.replace(' ', "")) {
            if s.contains('|') {
                let tools: Vec<Tool> = match std::env::var("RECTIFY_TOOLS") {
                    Ok(t) => t.split(',').map(|k| match k.trim() { "b" => Tool::Box, "c" => Tool::Circle, _ => Tool::Slot }).collect(),
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
        if let Ok(s) = std::env::var("RECTIFY_CIRCLE") {
            let pts: Vec<[f32; 2]> = s.split(';').map(nums).filter(|v| v.len() == 2).map(|v| [v[0], v[1]]).collect();
            let tag = std::env::var("RECTIFY_TAG").unwrap_or_else(|_| "circle".into());
            slot_drag(&mut h, &tag, &pts, Tool::Circle);
            return;
        }
        h.shot("look-ui");
        h.st.bare = true;
        h.shot("look");
        h.st.bare = false;
    }
}
