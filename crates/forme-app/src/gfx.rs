//! Frame composition: the raymarched scene goes to offscreen textures,
//! re-rendered only when something changed, then it is blitted and egui is
//! drawn on top.
//!
//! The scene never holds the UI back: while the user moves things, a small
//! image (its resolution adapts to the measured frame time) is rendered each
//! frame; once still, the full-quality image is refined band by band across
//! frames (a pixel budget per frame), and swapped in when complete.

use crate::state::AppState;
use forme_core::NodeId;
use forme_render::{Renderer, Settings, View};

const BLIT: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var o: VsOut;
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}
@fragment fn fs_main(i: VsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, i.uv);
}

// FXAA (console variant): smooths the edges of an image with one sample per
// pixel, used while the image moves (the still image accumulates samples).
fn luma(c: vec3<f32>) -> f32 {
    return dot(sqrt(max(c, vec3<f32>(0.0))), vec3<f32>(0.299, 0.587, 0.114));
}
fn at(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(tex, samp, uv, 0.0).rgb;
}
@fragment fn fs_fxaa(i: VsOut) -> @location(0) vec4<f32> {
    let px = 1.0 / vec2<f32>(textureDimensions(tex));
    let uv = i.uv;
    let m = at(uv);
    let l_nw = luma(at(uv + vec2<f32>(-0.5, -0.5) * px));
    let l_ne = luma(at(uv + vec2<f32>(0.5, -0.5) * px));
    let l_sw = luma(at(uv + vec2<f32>(-0.5, 0.5) * px));
    let l_se = luma(at(uv + vec2<f32>(0.5, 0.5) * px));
    let l_m = luma(m);
    let l_min = min(l_m, min(min(l_nw, l_ne), min(l_sw, l_se)));
    let l_max = max(l_m, max(max(l_nw, l_ne), max(l_sw, l_se)));
    if (l_max - l_min < max(0.03, l_max * 0.1)) {
        return vec4<f32>(m, 1.0);
    }
    var dir = vec2<f32>(-((l_nw + l_ne) - (l_sw + l_se)), (l_nw + l_sw) - (l_ne + l_se));
    let reduce = max((l_nw + l_ne + l_sw + l_se) * 0.03125, 1.0 / 128.0);
    let rcp = 1.0 / (min(abs(dir.x), abs(dir.y)) + reduce);
    dir = clamp(dir * rcp, vec2<f32>(-8.0), vec2<f32>(8.0)) * px;
    let a = 0.5 * (at(uv + dir * (1.0 / 3.0 - 0.5)) + at(uv + dir * (2.0 / 3.0 - 0.5)));
    let b = a * 0.5 + 0.25 * (at(uv - dir * 0.5) + at(uv + dir * 0.5));
    let l_b = luma(b);
    if (l_b < l_min || l_b > l_max) {
        return vec4<f32>(a, 1.0);
    }
    return vec4<f32>(b, 1.0);
}
"#;

/// Scene images are kept in half floats: jittered samples accumulate
/// without banding, and the blit converts to the screen format.
const SCENE_FMT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Radical inverse in base `b` (Halton sequence), in [0, 1).
fn halton(mut i: u32, b: u32) -> f32 {
    let (mut f, mut r) = (1.0f32, 0.0f32);
    while i > 0 {
        f /= b as f32;
        r += f * (i % b) as f32;
        i /= b;
    }
    r
}

struct Offscreen {
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
    size: [u32; 2],
}

/// Everything the scene image depends on (besides its size and quality).
#[derive(Clone, Copy, PartialEq)]
struct StateKey {
    scene: u64,
    view: u64,
    sel: Option<NodeId>,
    grid: bool,
    cam: [u32; 6],
    bake: (bool, u64, bool),
    /// Display settings (light panel): shadows, AO, light, exposure.
    display: [u32; 6],
}

#[derive(Clone, Copy, PartialEq)]
struct RenderKey {
    state: StateKey,
    size: [u32; 2],
    full: bool,
}

pub struct FrameRenderer {
    pub scene: Renderer,
    /// Baked mesh preview.
    pub mesh: forme_render::mesh::MeshRenderer,
    mesh_version: u64,
    pub egui: egui_wgpu::Renderer,
    blit: wgpu::RenderPipeline,
    /// Same blit with FXAA: edges stay smooth while the image moves.
    blit_fxaa: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Quick image (while moving, and as a preview) and full-quality image.
    low: Option<Offscreen>,
    high: Option<Offscreen>,
    #[allow(dead_code)]
    format: wgpu::TextureFormat,
    uploaded: (u64, Option<NodeId>),
    key_low: Option<RenderKey>,
    key_high: Option<RenderKey>,
    /// Rows of the full-quality image already rendered.
    high_rows: u32,
    /// Which image the shared view uniforms were last written for.
    uniforms: Option<RenderKey>,
    show_high: bool,
    pub settings: Settings,
    /// Fraction of full resolution used while interacting (upper bound).
    pub interactive_scale: f32,
    pub full_scale: f32,
    /// Pixel budgets (0 = use the scales): full image, quick image, and
    /// pixels refined per frame.
    pub full_px: f32,
    pub interactive_px: f32,
    pub band_px: f32,
    /// Adaptive factor on the quick image (from the measured frame time).
    adapt: f32,
    frame_ms: f32,
    last_frame: Option<std::time::Instant>,
    last_moving: bool,
    /// Last scene state seen, and when it last changed: "moving" means the
    /// image changes from frame to frame, not that a tool is active.
    last_state: Option<StateKey>,
    last_change: Option<std::time::Instant>,
    pub renders: u64,
    /// Desktop: one full-resolution image with the same shading while moving
    /// (no quick low-res image, no band refinement), then jittered samples
    /// accumulated while still (anti-aliasing). Off on iPad (pixel budgets).
    pub stable: bool,
    /// Samples accumulated in the current image, and how many to reach
    /// (stable mode).
    accum: u32,
    accum_target: u32,
}

impl FrameRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("forme-blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT.into()),
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit-pl"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });
        let make = |entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let blit = make("fs_main");
        let blit_fxaa = make("fs_fxaa");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        FrameRenderer {
            scene: Renderer::new(device, SCENE_FMT),
            mesh: forme_render::mesh::MeshRenderer::new(device, SCENE_FMT),
            mesh_version: 0,
            egui: egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default()),
            blit,
            blit_fxaa,
            blit_layout,
            sampler,
            low: None,
            high: None,
            format,
            uploaded: (0, None),
            key_low: None,
            key_high: None,
            high_rows: 0,
            uniforms: None,
            show_high: false,
            settings: Settings::default(),
            interactive_scale: 0.5,
            full_scale: 1.0,
            full_px: 0.0,
            interactive_px: 0.0,
            band_px: 1.5e6,
            adapt: 1.0,
            frame_ms: 16.0,
            last_frame: None,
            last_moving: false,
            last_state: None,
            last_change: None,
            renders: 0,
            stable: !cfg!(target_os = "ios"),
            accum: 0,
            accum_target: 1,
        }
    }

    fn make_offscreen(&self, device: &wgpu::Device, size: [u32; 2]) -> Offscreen {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("forme-scene"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_FMT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit-bg"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        Offscreen { view, bind, size }
    }

    /// Renders one frame into `target` (size in physical pixels).
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        size_px: [u32; 2],
        ppp: f32,
        st: &mut AppState,
        paint: &[egui::ClippedPrimitive],
        textures: &mut egui::TexturesDelta,
        force_full: bool,
    ) {
        // 1. scene upload
        let sel = st.selection;
        if self.uploaded != (st.scene_version, sel) {
            let main = st.program().clone();
            let (sp, mode) = st.selection_program();
            let sp = sp.clone();
            self.scene.set_scene(device, queue, &st.scene, &main, Some((&sp, mode)));
            self.uploaded = (st.scene_version, sel);
        }

        // diagnostic colours: the faces' role in the constructive bake
        let roles = st.bake.diag.roles && st.bake.open && st.bake.result.as_ref().is_some_and(|b| b.stats.constructive);
        let key = st.bake.version * 2 + roles as u64;
        if self.mesh_version != key {
            if let Some(b) = &st.bake.result {
                let verts = if roles {
                    forme_render::mesh::mesh_vertices_colored(b, &|f| crate::theme::bake_role_color(f.patch))
                } else {
                    forme_render::mesh::mesh_vertices(b, &st.scene.materials)
                };
                self.mesh.upload(device, queue, &verts);
            }
            self.mesh_version = key;
        }
        let show_bake = st.bake.show && self.mesh.has_mesh();
        self.mesh.wire = st.bake.wire;

        // 2. scene image (only when something changed)
        let now = std::time::Instant::now();
        let cam = st.camera;
        let state = StateKey {
            scene: st.scene_version,
            view: st.view_version,
            sel,
            grid: st.grid,
            cam: [cam.target.x.to_bits(), cam.target.y.to_bits(), cam.yaw.to_bits(), cam.pitch.to_bits(), cam.dist.to_bits(), cam.ortho as u32],
            bake: (show_bake, self.mesh_version, st.bake.wire),
            display: {
                let d = st.settings.display;
                [
                    d.shadows as u32 | (d.ao as u32) << 1,
                    d.light_azimuth.to_bits(),
                    d.light_elevation.to_bits(),
                    d.light_intensity.to_bits(),
                    d.exposure.to_bits(),
                    0,
                ]
            },
        };
        if self.last_state != Some(state) {
            self.last_state = Some(state);
            self.last_change = Some(now);
        }
        // fingers on the screen, or the image changed less than 0.12 s ago
        let recent = self.last_change.map(|t| now.duration_since(t).as_secs_f32() < 0.12).unwrap_or(false);
        let moving = !force_full && (st.interacting || st.handle_drag.is_some() || recent);
        if let Some(t) = self.last_frame {
            let dt = now.duration_since(t).as_secs_f32() * 1000.0;
            if moving && self.last_moving {
                // the quick image follows the measured frame time
                self.frame_ms = self.frame_ms * 0.7 + dt.min(200.0) * 0.3;
                if self.frame_ms > 30.0 {
                    self.adapt = (self.adapt * 0.92).max(0.35);
                } else if self.frame_ms < 18.0 {
                    self.adapt = (self.adapt * 1.04).min(1.0);
                }
            } else if !moving && !self.last_moving && self.key_high.is_some() && !self.show_high && self.high_rows > 0 {
                // refinement: as many pixels per frame as keeps the UI fluid
                if dt > 40.0 {
                    self.band_px = (self.band_px * 0.7).max(4.0e4);
                } else if dt < 22.0 {
                    self.band_px = (self.band_px * 1.15).min(8.0e6);
                }
            }
        }
        self.last_frame = Some(now);
        self.last_moving = moving;
        let px = size_px[0] as f32 * size_px[1] as f32;
        let budget = |px_budget: f32, cap: f32| if px_budget > 0.0 { (px_budget / px).sqrt().min(cap) } else { cap };
        let full_scale = budget(self.full_px, self.full_scale);
        let low_scale = (budget(self.interactive_px, self.interactive_scale) * self.adapt).min(full_scale);
        let dims = |sc: f32| [((size_px[0] as f32 * sc) as u32).max(16), ((size_px[1] as f32 * sc) as u32).max(16)];
        let (low_size, high_size) = (dims(low_scale), dims(full_scale));
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        // what to draw this frame: (full quality?, rows from, rows to)
        let mut job: Option<(bool, u32, u32)> = None;
        if self.stable {
            // heavy scenes only: if frames get slow while moving, fewer pixels
            // (same shading); back to full resolution as soon as it stops
            let size = if moving && !force_full && self.adapt < 0.999 { dims(full_scale * self.adapt.max(0.5)) } else { high_size };
            self.render_stable(device, queue, size_px, ppp, st, size, state, moving, force_full, show_bake);
        } else if force_full {
            self.key_high = Some(RenderKey { state, size: high_size, full: true });
            if self.high.as_ref().map(|o| o.size) != Some(high_size) {
                self.high = Some(self.make_offscreen(device, high_size));
            }
            if self.uniforms != self.key_high || self.high_rows < high_size[1] || !self.show_high {
                job = Some((true, 0, high_size[1]));
            }
        } else if moving {
            let k = RenderKey { state, size: low_size, full: false };
            if self.key_low != Some(k) {
                job = Some((false, 0, low_size[1]));
            }
            self.show_high = false;
        } else {
            let kh = RenderKey { state, size: high_size, full: true };
            if self.key_high != Some(kh) {
                // new image to refine: a quick preview first if the quick one is stale
                self.key_high = Some(kh);
                self.high_rows = 0;
                self.show_high = false;
                if self.key_low.map(|k| k.state) != Some(state) {
                    job = Some((false, 0, low_size[1]));
                }
            }
            if self.key_high == Some(kh) && self.high_rows >= high_size[1] {
                // still image already complete (e.g. a touch that changed nothing)
                self.show_high = true;
            }
            if job.is_none() && self.high_rows < high_size[1] {
                let rows = ((self.band_px / high_size[0] as f32) as u32).clamp(8, high_size[1]);
                job = Some((true, self.high_rows, (self.high_rows + rows).min(high_size[1])));
            }
        }

        if let Some((full, r0, r1)) = job {
            let size = if full { high_size } else { low_size };
            let key = RenderKey { state, size, full };
            let slot = if full { &mut self.high } else { &mut self.low };
            if slot.as_ref().map(|o| o.size) != Some(size) {
                let o = self.make_offscreen(device, size);
                if full {
                    self.high = Some(o);
                } else {
                    self.low = Some(o);
                }
            }
            let pv = st.view();
            let k = (size[0] as f32) / (size_px[0] as f32 / ppp);
            let view = View {
                eye: pv.eye,
                target: pv.target,
                fov_y: pv.fov_y,
                size: [size[0] as f32, size[1] as f32],
                principal: [pv.principal[0] * k, pv.principal[1] * k],
            };
            if self.uniforms != Some(key) {
                let mut settings = self.scene_settings(st, show_bake, [0.0, 0.0]);
                settings.shadows &= full;
                settings.ao &= full;
                if !full {
                    settings.max_steps = 110;
                }
                self.scene.update_view(queue, &view, &settings);
                self.uniforms = Some(key);
            }
            let off = if full { self.high.as_ref().unwrap() } else { self.low.as_ref().unwrap() };
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &off.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if r0 == 0 { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_scissor_rect(0, r0, size[0], r1 - r0);
            self.scene.draw(&mut pass);
            drop(pass);
            let complete = r1 >= size[1];
            if complete && show_bake {
                self.mesh.draw(device, queue, &mut enc, &off.view, size, &view);
            }
            if full {
                self.high_rows = r1;
                if complete {
                    self.show_high = true;
                }
            } else {
                self.key_low = Some(key);
                if !force_full && !moving {
                    // the next frames refine the full image
                    self.high_rows = 0;
                }
            }
            self.renders += 1;
        }

        // 3. egui resources
        for (id, deltas) in &textures.set {
            for delta in deltas.iter() {
                self.egui.update_texture(device, queue, *id, delta);
            }
        }
        let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: size_px, pixels_per_point: ppp };
        let extra = self.egui.update_buffers(device, queue, &mut enc, paint, &sd);

        // 4. blit + UI
        {
            let shown = if self.show_high { self.high.as_ref() } else { self.low.as_ref().or(self.high.as_ref()) };
            let off = shown.unwrap();
            let mut pass = enc
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            // few accumulated samples (moving image, baked mesh): FXAA on top
            let fxaa = self.stable && self.accum < 4;
            pass.set_pipeline(if fxaa { &self.blit_fxaa } else { &self.blit });
            pass.set_bind_group(0, &off.bind, &[]);
            pass.draw(0..3, 0..1);
            self.egui.render(&mut pass, paint, &sd);
        }
        queue.submit(extra.into_iter().chain(std::iter::once(enc.finish())));
        for id in &textures.free {
            self.egui.free_texture(id);
        }
        textures.clear();
    }

    /// Renderer settings from the app state and the light panel.
    fn scene_settings(&self, st: &AppState, show_bake: bool, jitter: [f32; 2]) -> Settings {
        let d = st.settings.display;
        let mut s = self.settings;
        s.grid = st.grid;
        s.shadows = self.settings.shadows && d.shadows;
        s.ao = self.settings.ao && d.ao;
        s.hide_scene = show_bake;
        s.light_dir = d.light_dir();
        s.light_intensity = d.light_intensity;
        s.exposure = d.exposure;
        s.jitter = jitter;
        s
    }

    /// View for an image of `size` pixels shown over a `size_px` target.
    fn image_view(st: &AppState, size: [u32; 2], size_px: [u32; 2], ppp: f32) -> View {
        let pv = st.view();
        let k = (size[0] as f32) / (size_px[0] as f32 / ppp);
        View {
            eye: pv.eye,
            target: pv.target,
            fov_y: pv.fov_y,
            size: [size[0] as f32, size[1] as f32],
            principal: [pv.principal[0] * k, pv.principal[1] * k],
        }
    }

    /// Stable mode: a new image is rendered at full resolution with the full
    /// shading as soon as anything changes; once still, jittered samples are
    /// blended into it (sample k with weight 1 / (k + 1)) up to the
    /// anti-aliasing count of the light panel.
    #[allow(clippy::too_many_arguments)]
    fn render_stable(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size_px: [u32; 2],
        ppp: f32,
        st: &AppState,
        size: [u32; 2],
        state: StateKey,
        moving: bool,
        force_full: bool,
        show_bake: bool,
    ) {
        let key = RenderKey { state, size, full: true };
        let target = if show_bake { 1 } else { st.settings.display.aa.clamp(1, 64) };
        self.accum_target = target;
        let fresh = self.key_low != Some(key);
        if fresh {
            self.accum = 0;
        }
        if self.low.as_ref().map(|o| o.size) != Some(size) {
            self.low = Some(self.make_offscreen(device, size));
        }
        self.show_high = false;
        let n = if force_full {
            target.saturating_sub(self.accum)
        } else if self.accum < target && (fresh || !moving) {
            1
        } else {
            0
        };
        let view = Self::image_view(st, size, size_px, ppp);
        for _ in 0..n {
            let i = self.accum;
            let jitter = if i == 0 { [0.0, 0.0] } else { [halton(i, 2) - 0.5, halton(i, 3) - 0.5] };
            let settings = self.scene_settings(st, show_bake, jitter);
            self.scene.update_view(queue, &view, &settings);
            let off = self.low.as_ref().unwrap();
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("scene-sample") });
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("scene"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &off.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: if i == 0 { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load },
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                if i == 0 {
                    self.scene.draw(&mut pass);
                } else {
                    self.scene.draw_blend(&mut pass, 1.0 / (i as f32 + 1.0));
                }
            }
            if i == 0 && show_bake {
                self.mesh.draw(device, queue, &mut enc, &off.view, size, &view);
            }
            queue.submit([enc.finish()]);
            self.accum += 1;
            self.renders += 1;
        }
        self.key_low = Some(key);
        self.uniforms = None;
    }

    /// Uploads egui texture changes without drawing (headless intermediate frames).
    pub fn apply_textures(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, textures: &mut egui::TexturesDelta) {
        for (id, deltas) in &textures.set {
            for delta in deltas.iter() {
                self.egui.update_texture(device, queue, *id, delta);
            }
        }
        for id in &textures.free {
            self.egui.free_texture(id);
        }
        textures.clear();
    }

    /// True while the full-quality image is still being refined.
    pub fn needs_refine(&self) -> bool {
        if self.stable {
            self.accum < self.accum_target
        } else {
            !self.show_high
        }
    }

    /// Smoothed time between frames while the image moves (ms).
    pub fn motion_frame_ms(&self) -> f32 {
        self.frame_ms
    }

    /// Samples accumulated in the image shown (stable mode).
    pub fn samples(&self) -> u32 {
        self.accum
    }
}
