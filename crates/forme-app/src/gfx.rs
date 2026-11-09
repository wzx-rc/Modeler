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
"#;

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
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Quick image (while moving, and as a preview) and full-quality image.
    low: Option<Offscreen>,
    high: Option<Offscreen>,
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
    pub renders: u64,
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
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit"),
            layout: Some(&pl),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        FrameRenderer {
            scene: Renderer::new(device, format),
            mesh: forme_render::mesh::MeshRenderer::new(device, format),
            mesh_version: 0,
            egui: egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default()),
            blit,
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
            renders: 0,
        }
    }

    fn make_offscreen(&self, device: &wgpu::Device, size: [u32; 2]) -> Offscreen {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("forme-scene"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
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

        if self.mesh_version != st.bake.version {
            if let Some(b) = &st.bake.result {
                let verts = forme_render::mesh::mesh_vertices(b, &st.scene.materials);
                self.mesh.upload(device, queue, &verts);
            }
            self.mesh_version = st.bake.version;
        }
        let show_bake = st.bake.show && self.mesh.has_mesh();
        self.mesh.wire = st.bake.wire;

        // 2. scene image (only when something changed)
        let moving = !force_full && (st.interacting || st.placement.is_some() || st.handle_drag.is_some());
        let now = std::time::Instant::now();
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
        let cam = st.camera;
        let state = StateKey {
            scene: st.scene_version,
            view: st.view_version,
            sel,
            grid: st.grid,
            cam: [cam.target.x.to_bits(), cam.target.y.to_bits(), cam.yaw.to_bits(), cam.pitch.to_bits(), cam.dist.to_bits(), cam.ortho as u32],
            bake: (show_bake, self.mesh_version, st.bake.wire),
        };
        let px = size_px[0] as f32 * size_px[1] as f32;
        let budget = |px_budget: f32, cap: f32| if px_budget > 0.0 { (px_budget / px).sqrt().min(cap) } else { cap };
        let full_scale = budget(self.full_px, self.full_scale);
        let low_scale = (budget(self.interactive_px, self.interactive_scale) * self.adapt).min(full_scale);
        let dims = |sc: f32| [((size_px[0] as f32 * sc) as u32).max(16), ((size_px[1] as f32 * sc) as u32).max(16)];
        let (low_size, high_size) = (dims(low_scale), dims(full_scale));
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        // what to draw this frame: (full quality?, rows from, rows to)
        let mut job: Option<(bool, u32, u32)> = None;
        if force_full {
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
                let mut settings = self.settings;
                settings.grid = st.grid;
                settings.shadows = full && self.settings.shadows;
                settings.ao = full && self.settings.ao;
                if !full {
                    settings.max_steps = 110;
                }
                settings.hide_scene = show_bake;
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
            pass.set_pipeline(&self.blit);
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
        !self.show_high
    }
}
