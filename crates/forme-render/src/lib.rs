//! Forme GPU viewport: a fullscreen pass that raymarches the scene bytecode.
//! wgpu runs it on Metal (iPad), Vulkan, DX12 or GL.

use bytemuck::{Pod, Zeroable};
use forme_core::{program::INSTR_WORDS, Program, Scene};
use glam::Vec3;

pub const SHADER: &str = include_str!("sdf.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Globals {
    pub eye: [f32; 4],
    pub fwd: [f32; 4],
    pub right: [f32; 4],
    pub up: [f32; 4],
    pub viewport: [f32; 4],
    pub scene_lo: [f32; 4],
    pub scene_hi: [f32; 4],
    pub sel_color: [f32; 4],
    pub ghost_color: [f32; 4],
    pub params: [f32; 4],
    pub mats: [[f32; 4]; 16],
    pub mats2: [[f32; 4]; 16],
}

/// Camera and framing for one frame. Sizes in physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub eye: Vec3,
    pub target: Vec3,
    pub fov_y: f32,
    pub size: [f32; 2],
    /// Projection centre in pixels (lets the object sit in the free area between panels).
    pub principal: [f32; 2],
}

impl View {
    pub fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let fwd = (self.target - self.eye).normalize_or(Vec3::NEG_Z);
        let mut right = fwd.cross(Vec3::Y);
        if right.length_squared() < 1e-8 {
            right = Vec3::X;
        }
        let right = right.normalize();
        let up = right.cross(fwd).normalize();
        (fwd, right, up)
    }

    /// Ray through a pixel (physical pixels, origin top-left).
    pub fn ray(&self, px: f32, py: f32) -> (Vec3, Vec3) {
        let (fwd, right, up) = self.basis();
        let half_h = self.size[1] * 0.5;
        let sx = (px - self.principal[0]) / half_h;
        let sy = -(py - self.principal[1]) / half_h;
        let t = (self.fov_y * 0.5).tan();
        (self.eye, (fwd + right * sx * t + up * sy * t).normalize())
    }

    /// Projects a world point to pixels. None when behind the camera.
    pub fn project(&self, p: Vec3) -> Option<[f32; 2]> {
        let (fwd, right, up) = self.basis();
        let d = p - self.eye;
        let z = d.dot(fwd);
        if z <= 1e-3 {
            return None;
        }
        let t = (self.fov_y * 0.5).tan();
        let half_h = self.size[1] * 0.5;
        let sx = d.dot(right) / (z * t);
        let sy = d.dot(up) / (z * t);
        Some([self.principal[0] + sx * half_h, self.principal[1] - sy * half_h])
    }

    /// World size of one pixel at distance `dist` from the eye.
    pub fn pixel_size_at(&self, dist: f32) -> f32 {
        (self.fov_y * 0.5).tan() * dist / (self.size[1] * 0.5)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelMode {
    None,
    /// Tint the selected shape where it shows on the surface.
    Tint,
    /// Tint + translucent volume (for cutters: carve, groove...).
    Ghost,
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub max_steps: u32,
    pub shadows: bool,
    pub ao: bool,
    pub grid: bool,
    pub sel_color: [f32; 3],
    pub ghost_color: [f32; 3],
    /// Background and ground only (another pass draws the content).
    pub hide_scene: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            max_steps: 160,
            shadows: true,
            ao: true,
            grid: true,
            sel_color: [0.24, 0.55, 1.0],
            ghost_color: [1.0, 0.45, 0.12],
            hide_scene: false,
        }
    }
}

pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    code: wgpu::Buffer,
    aux: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    code_cap: u64,
    aux_cap: u64,
    sel_start: Option<u32>,
    sel_mode: SelMode,
    has_scene: bool,
    bounds: (Vec3, Vec3),
    ground_y: f32,
    mats: [[f32; 4]; 16],
    mats2: [[f32; 4]; 16],
}

fn storage(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(256),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("forme-sdf"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("forme-layout"),
            entries: &[
                entry(0, wgpu::BufferBindingType::Uniform),
                entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
                entry(2, wgpu::BufferBindingType::Storage { read_only: true }),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("forme-pl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("forme-raymarch"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forme-globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let code_cap = 64 * 1024;
        let aux_cap = 16 * 1024;
        let code = storage(device, "forme-code", code_cap);
        let aux = storage(device, "forme-aux", aux_cap);
        let bind_group = Self::make_bind_group(device, &layout, &globals, &code, &aux);
        Renderer {
            pipeline,
            layout,
            globals,
            code,
            aux,
            bind_group,
            code_cap,
            aux_cap,
            sel_start: None,
            sel_mode: SelMode::None,
            has_scene: false,
            bounds: (Vec3::splat(-1.0), Vec3::splat(1.0)),
            ground_y: 0.0,
            mats: [[0.6, 0.6, 0.6, 0.0]; 16],
            mats2: [[0.5, 0.0, 0.0, 0.0]; 16],
        }
    }

    fn make_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        code: &wgpu::Buffer,
        aux: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("forme-bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: code.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: aux.as_entire_binding() },
            ],
        })
    }

    /// Uploads the compiled scene (+ the selection volume, if any).
    pub fn set_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        main: &Program,
        selection: Option<(&Program, SelMode)>,
    ) {
        let mut code: Vec<[f32; 4]> = main.code.clone();
        let mut aux: Vec<[f32; 4]> = main.aux.clone();
        self.sel_start = None;
        self.sel_mode = SelMode::None;
        if let Some((sel, mode)) = selection {
            if !sel.is_empty() && mode != SelMode::None {
                let start = code.len() as u32;
                let shift = (aux.len() * 2) as f32;
                let mut sc = sel.code.clone();
                // shift profile point offsets of the selection program
                for i in (0..sc.len()).step_by(INSTR_WORDS) {
                    if sc[i][0] as u32 == forme_core::program::OP_PRIM && sc[i][1] as u32 == 8 {
                        sc[i + 3][0] += shift;
                    }
                }
                code.extend(sc);
                aux.extend_from_slice(&sel.aux);
                self.sel_start = Some(start);
                self.sel_mode = mode;
            }
        }
        if aux.is_empty() {
            aux.push([0.0; 4]);
        }
        let code_bytes: &[u8] = bytemuck::cast_slice(&code);
        let aux_bytes: &[u8] = bytemuck::cast_slice(&aux);
        let mut rebuild = false;
        if code_bytes.len() as u64 > self.code_cap {
            self.code_cap = (code_bytes.len() as u64).next_power_of_two();
            self.code = storage(device, "forme-code", self.code_cap);
            rebuild = true;
        }
        if aux_bytes.len() as u64 > self.aux_cap {
            self.aux_cap = (aux_bytes.len() as u64).next_power_of_two();
            self.aux = storage(device, "forme-aux", self.aux_cap);
            rebuild = true;
        }
        if rebuild {
            self.bind_group = Self::make_bind_group(device, &self.layout, &self.globals, &self.code, &self.aux);
        }
        // pad the code with END so the tail of the buffer is harmless
        let mut padded = code_bytes.to_vec();
        padded.resize(self.code_cap as usize, 0);
        queue.write_buffer(&self.code, 0, &padded);
        queue.write_buffer(&self.aux, 0, aux_bytes);
        self.has_scene = !main.is_empty();
        self.bounds = scene.bounds();
        self.ground_y = self.bounds.0.y.min(0.0);
        for (i, m) in scene.materials.iter().take(16).enumerate() {
            let lin = |c: f32| c.max(0.0).powf(2.2);
            self.mats[i] = [lin(m.color[0]), lin(m.color[1]), lin(m.color[2]), m.metal];
            self.mats2[i] = [m.rough, 0.0, 0.0, 0.0];
        }
    }

    pub fn update_view(&self, queue: &wgpu::Queue, view: &View, settings: &Settings) {
        let (fwd, right, up) = view.basis();
        let g = Globals {
            eye: [view.eye.x, view.eye.y, view.eye.z, (view.fov_y * 0.5).tan()],
            fwd: [fwd.x, fwd.y, fwd.z, 0.0],
            right: [right.x, right.y, right.z, self.sel_start.map(|s| s as f32).unwrap_or(-1.0)],
            up: [
                up.x,
                up.y,
                up.z,
                match self.sel_mode {
                    SelMode::None => 0.0,
                    SelMode::Tint => 1.0,
                    SelMode::Ghost => 2.0,
                },
            ],
            viewport: [view.size[0], view.size[1], view.principal[0], view.principal[1]],
            scene_lo: [self.bounds.0.x, self.bounds.0.y, self.bounds.0.z, self.ground_y],
            scene_hi: [self.bounds.1.x, self.bounds.1.y, self.bounds.1.z, if settings.grid { 1.0 } else { 0.0 }],
            sel_color: [settings.sel_color[0], settings.sel_color[1], settings.sel_color[2], 1.0],
            ghost_color: [settings.ghost_color[0], settings.ghost_color[1], settings.ghost_color[2], 1.0],
            params: [
                settings.max_steps as f32,
                if self.has_scene && !settings.hide_scene { 1.0 } else { 0.0 },
                if settings.shadows { 1.0 } else { 0.0 },
                if settings.ao { 1.0 } else { 0.0 },
            ],
            mats: self.mats,
            mats2: self.mats2,
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    pub fn bounds(&self) -> (Vec3, Vec3) {
        self.bounds
    }
}

pub mod headless;
pub mod mesh;
