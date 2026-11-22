// Forme NURBS: the app, on the PC (the kernel in its own process) and on
// the iPad (the kernel linked in the app: iOS has no child processes;
// fingers and Pencil through `touch.rs`).

use nurbs_app::draw::Drawer;
use nurbs_app::state::AppState;
use nurbs_app::{files, theme};
use nurbs_kernel::client::{kernel_exe, Kernel};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{Force, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

struct Gfx {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    drawer: Drawer,
}

struct App {
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    st: AppState,
    ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    t0: Instant,
    redraw_until: f64,
    /// The session saved every few seconds while it changes (a crash
    /// loses little): the version saved and when.
    saved: ((u64, u64), f64),
    /// `FORME_SIM=1`: the scripted run for Apple's simulator (`sim.rs`)
    sim: Option<nurbs_app::sim::Sim>,
    /// another frame wanted (asked in `about_to_wait`)
    again: bool,
    _proxy: EventLoopProxy<()>,
}

/// The kernel: in the app on the iPad (and with the `local-kernel`
/// feature), else the process next to the app.
fn start_kernel() -> Kernel {
    #[cfg(any(target_os = "ios", feature = "local-kernel"))]
    {
        // the crashed step is kept out of Documents (seen in Files)
        let dir = std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join("Library")).unwrap_or_else(|_| files::documents_dir());
        let _ = std::fs::create_dir_all(&dir);
        return Kernel::start_local(&dir);
    }
    #[allow(unreachable_code)]
    Kernel::start(kernel_exe())
}

/// UI scale (egui points per screen point): tighter on the PC's 4K screen;
/// 1 on the iPad, whose layout is drawn for its own 1366 × 1024 points.
fn ui_zoom() -> f32 {
    let default = if cfg!(target_os = "ios") { 1.0 } else { 0.9 };
    std::env::var("FORME_ZOOM").ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

impl App {
    fn new(proxy: EventLoopProxy<()>) -> Self {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let kernel = start_kernel();
        // the last session if there is one
        let session = files::path_of(files::SESSION);
        let mut st = match files::load(&session) {
            Ok(doc) => AppState::new(doc, "Untitled", kernel),
            Err(_) => AppState::new(Default::default(), "Untitled", kernel),
        };
        st.settings = nurbs_app::settings::Settings::load();
        // the debug journal written as it goes (the last run's kept beside)
        let jf = files::documents_dir().join("journal.jsonl");
        let _ = std::fs::rename(&jf, files::documents_dir().join("journal-prev.jsonl"));
        st.journal_file = Some(jf);
        if !st.doc.features.is_empty() {
            st.frame_all();
        }
        // a mesh named on the command line: the reference to model on
        if let Some(p) = std::env::args().nth(1).filter(|a| a.to_lowercase().ends_with(".obj")) {
            let p = std::path::absolute(&p).map(|a| a.to_string_lossy().into_owned()).unwrap_or(p);
            st.import_reference(&p);
        }
        st.touch.on = cfg!(target_os = "ios") || std::env::var("FORME_TOUCH").map(|v| v == "1").unwrap_or(false);
        if !st.kernel.available() {
            st.toast("Kernel not found: build forme-nurbs-kernel next to the app", true);
        }
        // the link with Claude (the MCP server forme-nurbs-mcp connects
        // here): on the PC only
        if !cfg!(target_os = "ios") {
            let px = proxy.clone();
            if let Err(e) = nurbs_app::remote::start(&mut st, Arc::new(move || {
                let _ = px.send_event(());
            })) {
                eprintln!("Claude link off: {e}");
            }
        }
        let saved = ((st.version, st.looks_rev), 0.0);
        App { window: None, gfx: None, st, ctx, egui_state: None, t0: Instant::now(), redraw_until: 0.0, saved, sim: nurbs_app::sim::Sim::from_env(), again: true, _proxy: proxy }
    }

    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    /// Whether the window is minimized (no picture then: Windows sends it
    /// no redraw, and a swapchain of no size can block).
    fn minimized(&self) -> bool {
        self.window.as_ref().map(|w| w.is_minimized() == Some(true) || w.inner_size().width == 0 || w.inner_size().height == 0).unwrap_or(false)
    }

    /// What goes on with or without a picture: the kernel's answers, the
    /// link with Claude, the session saved every 10 s.
    fn idle(&mut self) {
        let now = self.t0.elapsed().as_secs_f64();
        self.st.tick(now);
        nurbs_app::remote::poll(&mut self.st);
        let stamp = (self.st.version, self.st.looks_rev);
        if stamp != self.saved.0 && now - self.saved.1 > 10.0 && !self.st.is_live() && self.st.look_live.is_none() {
            let _ = files::save(&self.st.doc, files::SESSION);
            self.saved = (stamp, now);
        }
    }

    /// Minimized (20 nov., 11 h 28: the app minimized answered neither the
    /// link nor a close, and saved nothing): `idle` on a timer, and the
    /// pictures Claude asks for still rendered offscreen.
    fn frame_minimized(&mut self) {
        self.idle();
        if let Some(gfx) = self.gfx.as_mut() {
            if !self.st.link.captures.is_empty() {
                let (device, queue, format, drawer) = (&gfx.device, &gfx.queue, gfx.config.format, &mut gfx.drawer);
                nurbs_app::remote::render_captures(&mut self.st, |st, w, h| {
                    let tex = nurbs_app::remote::target(device, format, w, h);
                    let view = tex.create_view(&Default::default());
                    drawer.render(device, queue, &view, [w, h], 1.0, st, &[], &mut egui::TexturesDelta::default());
                    nurbs_app::remote::read_rgba(device, queue, &tex)
                });
            }
        }
    }

    fn frame(&mut self) {
        if self.minimized() {
            self.frame_minimized();
            return;
        }
        self.idle();
        let (Some(window), Some(gfx), Some(es)) = (self.window.clone(), self.gfx.as_mut(), self.egui_state.as_mut()) else {
            return;
        };
        if let Some(sim) = self.sim.as_mut() {
            let (i, o) = (window.inner_size(), window.outer_size());
            let info = format!("window inner {}x{} outer {}x{} scale {:.2} surface {}x{}", i.width, i.height, o.width, o.height, window.scale_factor(), gfx.config.width, gfx.config.height);
            sim.tick(&self.ctx, &mut self.st, self.t0.elapsed().as_secs_f64(), &info);
        }
        let mut raw = es.take_egui_input(&window);
        if self.st.touch.on {
            let now = self.t0.elapsed().as_secs_f64();
            nurbs_app::touch::tick(&mut self.st, now);
            self.st.touch.apply(&mut raw);
        }
        let st = &mut self.st;
        let mut out = self.ctx.run_pass(raw, |ctx| nurbs_app::run(ctx, st));
        es.handle_platform_output(&window, std::mem::take(&mut out.platform_output));
        let ppp = out.pixels_per_point;
        let paint = self.ctx.tessellate(std::mem::take(&mut out.shapes), ppp);
        let frame = match gfx.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                gfx.surface.configure(&gfx.device, &gfx.config);
                self.again = true;
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        gfx.drawer.render(&gfx.device, &gfx.queue, &view, [gfx.config.width, gfx.config.height], ppp, &self.st, &paint, &mut out.textures_delta);
        window.pre_present_notify();
        gfx.queue.present(frame);
        // pictures asked by Claude: the view alone, offscreen
        if !self.st.link.captures.is_empty() {
            let (device, queue, format, drawer) = (&gfx.device, &gfx.queue, gfx.config.format, &mut gfx.drawer);
            nurbs_app::remote::render_captures(&mut self.st, |st, w, h| {
                let tex = nurbs_app::remote::target(device, format, w, h);
                let view = tex.create_view(&Default::default());
                drawer.render(device, queue, &view, [w, h], 1.0, st, &[], &mut egui::TexturesDelta::default());
                nurbs_app::remote::read_rgba(device, queue, &tex)
            });
            self.again = true;
        }
        // the kernel is working: keep looking for its answer
        let now = self.now();
        let busy = self.st.interacting || self.st.toast.is_some() || self.st.building() || self.st.ref_busy() || self.st.dev_busy() || self.st.link.busy() || self.st.touch.active() || self.sim.as_ref().is_some_and(|s| !s.done);
        let egui_wants = out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay.as_secs_f32() < 0.05).unwrap_or(false);
        // asked in `about_to_wait`, outside this redraw: on iOS a redraw
        // asked while one is drawn is dropped (winit's view clears it after
        // drawRect), so the app drew once and waited for the next touch (22
        // nov., Apple's simulator: the first frame only, no interface; on the
        // iPad the tabs and the panels a touch behind)
        self.again |= busy || egui_wants || now < self.redraw_until;
    }

    fn save_session(&mut self) {
        let _ = files::save(&self.st.doc, files::SESSION);
        self.saved = ((self.st.version, self.st.looks_rev), self.now());
    }

    /// iPad: a touch goes to `touch.rs` (fingers = camera, Pencil = mouse,
    /// the Shift / Ctrl buttons), not to egui-winit.
    fn touch(&mut self, window: &Window, t: &winit::event::Touch) {
        let ppp = self.ctx.pixels_per_point() as f64;
        let p = egui::pos2((t.location.x / ppp) as f32, (t.location.y / ppp) as f32);
        // the Apple Pencil reports an altitude angle, fingers do not
        let pencil = matches!(t.force, Some(Force::Calibrated { altitude_angle: Some(_), .. }));
        let phase = match t.phase {
            TouchPhase::Started => nurbs_app::touch::Phase::Start,
            TouchPhase::Moved => nurbs_app::touch::Phase::Move,
            TouchPhase::Ended => nurbs_app::touch::Phase::End,
            TouchPhase::Cancelled => nurbs_app::touch::Phase::Cancel,
        };
        let now = self.now();
        nurbs_app::touch::on_touch(&self.ctx, &mut self.st, t.id, phase, p, pencil, now);
        self.redraw_until = now + 0.25;
        window.request_redraw();
    }
}

impl ApplicationHandler<()> for App {
    /// Claude sent a request: a frame reads it.
    fn user_event(&mut self, _el: &ActiveEventLoop, _e: ()) {
        if self.minimized() {
            self.frame_minimized();
        } else if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes().with_title(nurbs_app::version_title()).with_window_icon(theme::window_icon());
        // the PC's window; on the iPad winit makes the UIWindow this size
        // (22 nov., Romain's capture: a window of 1600 × 900 points on an
        // 11" screen of 1194 × 834, its right and bottom off the screen):
        // there it takes the screen's own
        if !cfg!(target_os = "ios") {
            attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(1600.0, 900.0));
        }
        #[cfg(target_os = "ios")]
        {
            // the Shift / Ctrl buttons sit by the bottom edge: a swipe there must not send
            // the app home at once (iPadOS asks for a second swipe)
            use winit::platform::ios::{ScreenEdge, WindowAttributesExtIOS};
            attrs = attrs.with_preferred_screen_edges_deferring_system_gestures(ScreenEdge::BOTTOM).with_prefers_home_indicator_hidden(true).with_prefers_status_bar_hidden(true);
        }
        let window = Arc::new(el.create_window(attrs).expect("window"));
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: Some(&surface), ..Default::default() })).expect("adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("forme-nurbs"), required_limits: adapter.limits(), ..Default::default() })).expect("device");
        // iOS: the whole screen (`inner_size` is its safe area there; egui
        // keeps the islands inside it, the view goes under the edges)
        let size = if cfg!(target_os = "ios") { window.outer_size() } else { window.inner_size() };
        let mut config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).expect("surface config");
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().copied().find(|f| f.is_srgb()) {
            config.format = f;
        }
        surface.configure(&device, &config);
        let drawer = Drawer::new(&device, config.format);
        // a bit tighter than egui's default (Romain works on a 4K screen)
        self.ctx.set_zoom_factor(ui_zoom());
        let es = egui_winit::State::new(self.ctx.clone(), egui::ViewportId::ROOT, &window, Some(window.scale_factor() as f32), None, Some(device.limits().max_texture_dimension_2d as usize));
        self.egui_state = Some(es);
        self.gfx = Some(Gfx { surface, device, queue, config, drawer });
        self.window = Some(window.clone());
        window.request_redraw();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else { return };
        if let (true, WindowEvent::Touch(t)) = (self.st.touch.on, &event) {
            self.touch(&window, t);
            return;
        }
        if let Some(es) = self.egui_state.as_mut() {
            let _ = es.on_window_event(&window, &event);
        }
        match event {
            WindowEvent::CloseRequested => {
                self.save_session();
                el.exit();
            }
            WindowEvent::Resized(size) => {
                // minimized (no size): the swapchain is left as it was
                if let (Some(g), true) = (self.gfx.as_mut(), size.width > 0 && size.height > 0) {
                    g.config.width = size.width;
                    g.config.height = size.height;
                    g.surface.configure(&g.device, &g.config);
                }
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => self.frame(),
            _ => {
                self.redraw_until = self.now() + 0.25;
                window.request_redraw();
            }
        }
    }

    /// iPad: going to the background may be the last chance to save.
    fn suspended(&mut self, _el: &ActiveEventLoop) {
        self.save_session();
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.minimized() {
            // no redraws come: the kernel, the link and the saves on a timer
            self.frame_minimized();
            el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + std::time::Duration::from_millis(100)));
        } else {
            el.set_control_flow(ControlFlow::Wait);
            if std::mem::take(&mut self.again) {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
        }
    }
}

/// A crash (panic) goes to Documents/Forme NURBS/crash.log: the app runs
/// without a console.
fn install_crash_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let text = format!("---- crash · {} · unix {secs}\n{info}\n{}\n", nurbs_app::version_title(), std::backtrace::Backtrace::force_capture());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(files::documents_dir().join("crash.log")) {
            use std::io::Write;
            let _ = f.write_all(text.as_bytes());
        }
        default(info);
    }));
}

fn main() {
    install_crash_log();
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::new(event_loop.create_proxy());
    event_loop.run_app(&mut app).expect("run");
}
