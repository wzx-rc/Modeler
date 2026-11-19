// Forme NURBS: the desktop app (the iPad will come with the kernel linked
// in the app, no child process there).

use nurbs_app::draw::Drawer;
use nurbs_app::state::AppState;
use nurbs_app::{files, theme};
use nurbs_kernel::client::{kernel_exe, Kernel};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
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
}

impl App {
    fn new() -> Self {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let kernel = Kernel::start(kernel_exe());
        // the last session if there is one
        let session = files::path_of(files::SESSION);
        let mut st = match files::load(&session) {
            Ok(doc) => AppState::new(doc, "Untitled", kernel),
            Err(_) => AppState::new(Default::default(), "Untitled", kernel),
        };
        st.settings = nurbs_app::settings::Settings::load();
        if !st.doc.features.is_empty() {
            st.frame_all();
        }
        // a mesh named on the command line: the reference to model on
        if let Some(p) = std::env::args().nth(1).filter(|a| a.to_lowercase().ends_with(".obj")) {
            let p = std::path::absolute(&p).map(|a| a.to_string_lossy().into_owned()).unwrap_or(p);
            st.import_reference(&p);
        }
        if st.kernel.exe().is_none() {
            st.toast("Kernel not found: build forme-nurbs-kernel next to the app", true);
        }
        App { window: None, gfx: None, st, ctx, egui_state: None, t0: Instant::now(), redraw_until: 0.0 }
    }

    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    fn frame(&mut self) {
        let (Some(window), Some(gfx), Some(es)) = (self.window.clone(), self.gfx.as_mut(), self.egui_state.as_mut()) else {
            return;
        };
        let raw = es.take_egui_input(&window);
        let now = self.t0.elapsed().as_secs_f64();
        self.st.tick(now);
        let st = &mut self.st;
        let mut out = self.ctx.run_pass(raw, |ctx| nurbs_app::run(ctx, st));
        es.handle_platform_output(&window, std::mem::take(&mut out.platform_output));
        let ppp = out.pixels_per_point;
        let paint = self.ctx.tessellate(std::mem::take(&mut out.shapes), ppp);
        let frame = match gfx.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                gfx.surface.configure(&gfx.device, &gfx.config);
                window.request_redraw();
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        gfx.drawer.render(&gfx.device, &gfx.queue, &view, [gfx.config.width, gfx.config.height], ppp, &self.st, &paint, &mut out.textures_delta);
        window.pre_present_notify();
        gfx.queue.present(frame);
        // the kernel is working: keep looking for its answer
        let busy = self.st.interacting || self.st.toast.is_some() || self.st.building() || self.st.ref_busy() || self.st.dev_busy();
        let egui_wants = out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay.as_secs_f32() < 0.05).unwrap_or(false);
        if busy || egui_wants || now < self.redraw_until {
            window.request_redraw();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title(nurbs_app::version_title()).with_inner_size(winit::dpi::LogicalSize::new(1600.0, 900.0)).with_window_icon(theme::window_icon());
        let window = Arc::new(el.create_window(attrs).expect("window"));
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: Some(&surface), ..Default::default() })).expect("adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("forme-nurbs"), required_limits: adapter.limits(), ..Default::default() })).expect("device");
        let size = window.inner_size();
        let mut config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).expect("surface config");
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().copied().find(|f| f.is_srgb()) {
            config.format = f;
        }
        surface.configure(&device, &config);
        let drawer = Drawer::new(&device, config.format);
        // a bit tighter than egui's default (Romain works on a 4K screen)
        self.ctx.set_zoom_factor(std::env::var("FORME_ZOOM").ok().and_then(|v| v.parse().ok()).unwrap_or(0.9));
        let es = egui_winit::State::new(self.ctx.clone(), egui::ViewportId::ROOT, &window, Some(window.scale_factor() as f32), None, Some(device.limits().max_texture_dimension_2d as usize));
        self.egui_state = Some(es);
        self.gfx = Some(Gfx { surface, device, queue, config, drawer });
        self.window = Some(window.clone());
        window.request_redraw();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else { return };
        if let Some(es) = self.egui_state.as_mut() {
            let _ = es.on_window_event(&window, &event);
        }
        match event {
            WindowEvent::CloseRequested => {
                let _ = files::save(&self.st.doc, files::SESSION);
                el.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(g) = self.gfx.as_mut() {
                    g.config.width = size.width.max(1);
                    g.config.height = size.height.max(1);
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

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        el.set_control_flow(ControlFlow::Wait);
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
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("run");
}
