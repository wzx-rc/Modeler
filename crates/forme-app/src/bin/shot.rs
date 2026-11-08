//! Headless screenshots of the real app: drives it with synthetic input events
//! (Pencil, fingers, taps, drags) and renders frames with the same code as iPad.

use egui::{pos2, Event, Modifiers, PointerButton, Pos2, RawInput, Rect};
use forme_app::gfx::FrameRenderer;
use forme_app::state::{AppState, PointerKind, Tool};
use forme_app::{theme, AppState as _S};
use forme_core::{demo, CombineMode, NodeId, PrimKind};
use forme_render::headless::Gpu;
use glam::Vec3;
use std::time::Instant;

struct Harness {
    gpu: Gpu,
    fr: FrameRenderer,
    ctx: egui::Context,
    st: AppState,
    t: f64,
    size: [f32; 2],
    ppp: f32,
    pending: Vec<Event>,
    out_dir: String,
}

const FMT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

impl Harness {
    fn new(out_dir: &str) -> Self {
        let gpu = Gpu::new().expect("gpu");
        eprintln!("GPU : {}", gpu.adapter_name());
        let mut fr = FrameRenderer::new(&gpu.device, FMT);
        fr.full_scale = std::env::var("SHOT_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(0.6);
        let ctx = egui::Context::default();
        theme::install(&ctx);
        Harness {
            gpu,
            fr,
            ctx,
            st: AppState::default(),
            t: 0.0,
            size: [1194.0, 834.0],
            ppp: 2.0,
            pending: vec![],
            out_dir: out_dir.to_string(),
        }
    }

    fn reset(&mut self, st: AppState) {
        self.st = st;
        self.st.autosave = false;
        self.st.screen = self.size;
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

    /// Runs one UI frame (logic only, no GPU drawing).
    fn step(&mut self) {
        self.t += 1.0 / 60.0;
        let raw = self.raw();
        self.st.tick(self.t);
        let st = &mut self.st;
        let mut out = self.ctx.run_pass(raw, |ctx| forme_app::run(ctx, st));
        self.fr.apply_textures(&self.gpu.device, &self.gpu.queue, &mut out.textures_delta);
    }

    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    fn press(&mut self, p: Pos2, down: bool) {
        self.pending.push(Event::PointerMoved(p));
        self.pending.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: down, modifiers: Modifiers::NONE });
        self.step();
    }

    fn move_to(&mut self, p: Pos2) {
        self.pending.push(Event::PointerMoved(p));
        self.step();
    }

    fn tap(&mut self, p: Pos2) {
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

    /// Renders the current state at full quality and saves a PNG.
    fn shot(&mut self, name: &str) {
        self.t += 1.0 / 60.0;
        let raw = self.raw();
        self.st.tick(self.t);
        let st = &mut self.st;
        let mut out = self.ctx.run_pass(raw, |ctx| forme_app::run(ctx, st));
        let paint = self.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        let w = (self.size[0] * self.ppp) as u32;
        let h = (self.size[1] * self.ppp) as u32;
        let tex = self.gpu.target(w, h, FMT);
        let view = tex.create_view(&Default::default());
        let t0 = Instant::now();
        self.fr.render(&self.gpu.device, &self.gpu.queue, &view, [w, h], self.ppp, &mut self.st, &paint, &mut out.textures_delta, true);
        let px = self.gpu.read_rgba(&tex);
        let path = format!("{}/{}.png", self.out_dir, name);
        image::save_buffer(&path, &px, w, h, image::ExtendedColorType::Rgba8).expect("png");
        eprintln!("{path}  ({:.1?})", t0.elapsed());
    }

    fn find(&self, name: &str) -> NodeId {
        self.st.scene.nodes.values().find(|n| n.name == name).map(|n| n.id).unwrap_or_else(|| panic!("node {name}"))
    }
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "shots".into());
    let only: Vec<String> = std::env::args().skip(2).collect();
    std::env::set_var("FORME_DOCS", format!("{out}/docs"));
    let mut h = Harness::new(&out);
    let want = |n: &str| only.is_empty() || only.iter().any(|o| n.starts_with(o.as_str()));

    if want("02") {
        // Main screen: the grenade, a boss picked by tapping it.
        h.reset(AppState::default());
        h.st.pointer_kind = PointerKind::Pencil;
        h.steps(3);
        let boss = h.screen_of(Vec3::new(4.6, 70.0, 33.2));
        h.tap(boss);
        let sel = h.st.selection.map(|id| h.st.scene.node(id).unwrap().name.clone());
        eprintln!("tap → sélection : {sel:?}");
        h.steps(2);
        h.shot("02-ecran-principal");
    }
    let _ = (CombineMode::Add, PrimKind::Box, Tool::Select, demo::starter);
    let _: Option<_S> = None;
}
