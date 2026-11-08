//! Forme app entry point (iPad via winit's UIKit backend, and desktop for development).

use forme_app::gfx::FrameRenderer;
use forme_app::state::{AppState, PointerKind};
use forme_app::{files, theme};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{Force, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

struct Gfx {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    fr: FrameRenderer,
}

struct TouchTrack {
    start: Instant,
    pos0: (f64, f64),
    moved: bool,
}

struct App {
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    st: AppState,
    ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    t0: Instant,
    touches: HashMap<u64, TouchTrack>,
    gesture_touches: usize,
    gesture_start: Option<Instant>,
    gesture_moved: bool,
    redraw_until: f64,
    thumb_touch: Option<u64>,
    ctrl_touch: Option<u64>,
}

impl App {
    fn new() -> Self {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let st = match files::load(&files::project_path("Autosauvegarde")) {
            Ok(scene) => {
                let mut s = AppState::new(scene, "Autosauvegarde");
                s.frame_all();
                s
            }
            Err(_) => AppState::default(),
        };
        App {
            window: None,
            gfx: None,
            st,
            ctx,
            egui_state: None,
            t0: Instant::now(),
            touches: HashMap::new(),
            gesture_touches: 0,
            gesture_start: None,
            gesture_moved: false,
            redraw_until: 0.0,
            thumb_touch: None,
            ctrl_touch: None,
        }
    }

    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    /// Two-finger tap = undo, three-finger tap = redo (Procreate style).
    fn track_touch(&mut self, id: u64, phase: TouchPhase, pos: (f64, f64)) {
        match phase {
            TouchPhase::Started => {
                if self.touches.is_empty() {
                    self.gesture_touches = 0;
                    self.gesture_start = Some(Instant::now());
                    self.gesture_moved = false;
                }
                self.touches.insert(id, TouchTrack { start: Instant::now(), pos0: pos, moved: false });
                self.gesture_touches = self.gesture_touches.max(self.touches.len());
            }
            TouchPhase::Moved => {
                if let Some(t) = self.touches.get_mut(&id) {
                    if (t.pos0.0 - pos.0).hypot(t.pos0.1 - pos.1) > 12.0 {
                        t.moved = true;
                        self.gesture_moved = true;
                    }
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if let Some(t) = self.touches.remove(&id) {
                    let _ = t.start;
                }
                if self.touches.is_empty() {
                    let quick = self.gesture_start.map(|s| s.elapsed().as_secs_f32() < 0.35).unwrap_or(false);
                    if quick && !self.gesture_moved {
                        match self.gesture_touches {
                            2 => self.st.undo(),
                            3 => self.st.redo(),
                            _ => {}
                        }
                    }
                    self.gesture_touches = 0;
                }
            }
        }
    }

    fn frame(&mut self) {
        let (Some(window), Some(gfx), Some(es)) = (self.window.clone(), self.gfx.as_mut(), self.egui_state.as_mut()) else {
            return;
        };
        let raw = es.take_egui_input(&window);
        let now = self.t0.elapsed().as_secs_f64();
        self.st.tick(now);
        let st = &mut self.st;
        let mut out = self.ctx.run_pass(raw, |ctx| forme_app::run(ctx, st));
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
        gfx.fr.render(
            &gfx.device,
            &gfx.queue,
            &view,
            [gfx.config.width, gfx.config.height],
            ppp,
            &mut self.st,
            &paint,
            &mut out.textures_delta,
            false,
        );
        window.pre_present_notify();
        gfx.queue.present(frame);
        let busy = self.st.interacting || self.st.export_job.is_some() || self.st.toast.is_some() || gfx.fr.needs_refine();
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
        let attrs = Window::default_attributes().with_title("Forme").with_inner_size(winit::dpi::LogicalSize::new(1194.0, 834.0));
        let window = Arc::new(el.create_window(attrs).expect("window"));
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("forme"),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .expect("device");
        let size = window.inner_size();
        let mut config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).expect("surface config");
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().copied().find(|f| f.is_srgb()) {
            config.format = f;
        }
        surface.configure(&device, &config);
        #[allow(unused_mut)]
        let mut fr = FrameRenderer::new(&device, config.format);
        #[cfg(target_os = "ios")]
        {
            fr.full_scale = 0.85;
            fr.interactive_scale = 0.5;
        }
        let es = egui_winit::State::new(
            self.ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        self.egui_state = Some(es);
        self.gfx = Some(Gfx { surface, device, queue, config, fr });
        self.window = Some(window.clone());
        window.request_redraw();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else { return };
        // The thumb Carve button is handled here, outside egui: holding it with a
        // finger must not turn the Pencil touch into a two-finger camera gesture.
        if let WindowEvent::Touch(t) = &event {
            let sf = window.scale_factor();
            let size = window.inner_size().to_logical::<f32>(sf);
            let p = t.location.to_logical::<f32>(sf);
            let tr = forme_app::ui::thumb_rect(size.width, size.height);
            let pencil = matches!(t.force, Some(Force::Calibrated { altitude_angle: Some(_), .. }));
            match t.phase {
                TouchPhase::Started if !pencil && self.thumb_touch.is_none() && tr.expand(10.0).contains(egui::pos2(p.x, p.y)) => {
                    self.thumb_touch = Some(t.id);
                    self.st.carve_held = true;
                    window.request_redraw();
                    return;
                }
                TouchPhase::Ended | TouchPhase::Cancelled if self.thumb_touch == Some(t.id) => {
                    self.thumb_touch = None;
                    self.st.carve_held = false;
                    window.request_redraw();
                    return;
                }
                _ if self.thumb_touch == Some(t.id) => return,
                _ => {}
            }
            // same for the CTRL button (held while the Pencil taps edges)
            let cr = forme_app::ui::ctrl_rect(size.width, size.height);
            match t.phase {
                TouchPhase::Started if !pencil && self.ctrl_touch.is_none() && cr.expand(10.0).contains(egui::pos2(p.x, p.y)) => {
                    self.ctrl_touch = Some(t.id);
                    self.st.ctrl_held = true;
                    window.request_redraw();
                    return;
                }
                TouchPhase::Ended | TouchPhase::Cancelled if self.ctrl_touch == Some(t.id) => {
                    self.ctrl_touch = None;
                    self.st.ctrl_held = false;
                    window.request_redraw();
                    return;
                }
                _ if self.ctrl_touch == Some(t.id) => return,
                _ => {}
            }
        }
        match &event {
            WindowEvent::Touch(t) => {
                // Apple Pencil reports an altitude angle; fingers do not.
                let pencil = matches!(t.force, Some(Force::Calibrated { altitude_angle: Some(_), .. }));
                self.st.pointer_kind = if pencil { PointerKind::Pencil } else { PointerKind::Finger };
                if !pencil {
                    self.track_touch(t.id, t.phase, (t.location.x, t.location.y));
                }
            }
            WindowEvent::CursorMoved { .. } | WindowEvent::MouseInput { .. } => {
                if self.touches.is_empty() && !cfg!(target_os = "ios") {
                    self.st.pointer_kind = PointerKind::Mouse;
                }
            }
            _ => {}
        }
        if let Some(es) = self.egui_state.as_mut() {
            let _ = es.on_window_event(&window, &event);
        }
        match event {
            WindowEvent::CloseRequested => {
                let _ = files::save(&self.st.scene, "Autosauvegarde");
                el.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(g) = self.gfx.as_mut() {
                    g.config.width = size.width.max(1);
                    g.config.height = size.height.max(1);
                    g.surface.configure(&g.device, &g.config);
                }
                self.st.view_changed();
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

fn main() {
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("run");
}
