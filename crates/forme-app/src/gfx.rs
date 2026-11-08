//! Frame composition: the raymarched scene goes to an offscreen texture
//! (re-rendered only when something changed, at reduced resolution while the
//! user is moving things), then it is blitted and egui is drawn on top.

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

#[derive(Clone, Copy, PartialEq)]
struct RenderKey {
    scene: u64,
    view: u64,
    sel: Option<NodeId>,
    size: [u32; 2],
    full: bool,
    grid: bool,
    cam: [u32; 6],
}

pub struct FrameRenderer {
    pub scene: Renderer,
    pub egui: egui_wgpu::Renderer,
    blit: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    offscreen: Option<Offscreen>,
    format: wgpu::TextureFormat,
    uploaded: (u64, Option<NodeId>),
    last_key: Option<RenderKey>,
    pub settings: Settings,
    /// Fraction of full resolution used while interacting.
    pub interactive_scale: f32,
    pub full_scale: f32,
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
            egui: egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default()),
            blit,
            blit_layout,
            sampler,
            offscreen: None,
            format,
            uploaded: (0, None),
            last_key: None,
            settings: Settings::default(),
            interactive_scale: 0.5,
            full_scale: 1.0,
            renders: 0,
        }
    }

    fn ensure_offscreen(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.offscreen.as_ref().map(|o| o.size == size).unwrap_or(false) {
            return;
        }
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
        self.offscreen = Some(Offscreen { view, bind, size });
        self.last_key = None;
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

        // 2. scene image (only when something changed)
        let moving = !force_full && (st.interacting || st.placement.is_some() || st.handle_drag.is_some());
        let scale = if moving { self.interactive_scale } else { self.full_scale };
        let w = ((size_px[0] as f32 * scale) as u32).max(16);
        let h = ((size_px[1] as f32 * scale) as u32).max(16);
        self.ensure_offscreen(device, [w, h]);
        let cam = st.camera;
        let key = RenderKey {
            scene: st.scene_version,
            view: st.view_version,
            sel,
            size: [w, h],
            full: !moving,
            grid: st.grid,
            cam: [cam.target.x.to_bits(), cam.target.y.to_bits(), cam.yaw.to_bits(), cam.pitch.to_bits(), cam.dist.to_bits(), cam.ortho as u32],
        };
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        if self.last_key != Some(key) {
            let k = (w as f32) / (size_px[0] as f32 / ppp);
            let pv = st.view();
            let view = View {
                eye: pv.eye,
                target: pv.target,
                fov_y: pv.fov_y,
                size: [w as f32, h as f32],
                principal: [pv.principal[0] * k, pv.principal[1] * k],
            };
            let mut settings = self.settings;
            settings.grid = st.grid;
            settings.shadows = !moving && self.settings.shadows;
            settings.ao = !moving && self.settings.ao;
            if moving {
                settings.max_steps = 110;
            }
            self.scene.update_view(queue, &view, &settings);
            let off = self.offscreen.as_ref().unwrap();
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &off.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            self.scene.draw(&mut pass);
            drop(pass);
            self.last_key = Some(key);
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
            let off = self.offscreen.as_ref().unwrap();
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

    /// True when the last frame used the reduced resolution (a full one is due).
    pub fn needs_refine(&self) -> bool {
        self.last_key.map(|k| !k.full).unwrap_or(false)
    }
}
