//! Polygon mesh preview (the baked mesh): lit faces, crease-aware normals and
//! an optional wireframe of the real polygon edges (not the triangulation).

use crate::View;
use bytemuck::{Pod, Zeroable};
use forme_core::bake::Baked;
use forme_core::scene::Material;

const SHADER: &str = r#"
struct U {
    eye: vec4<f32>,
    fwd: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    view: vec4<f32>,
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) col: vec4<f32>,
    @location(3) bary: vec4<f32>,
};
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) nrm: vec3<f32>,
    @location(1) col: vec3<f32>,
    @location(2) bary: vec3<f32>,
    @location(3) @interpolate(flat) mask: f32,
    @location(4) wpos: vec3<f32>,
};

@vertex fn vs_main(i: VIn) -> VOut {
    var o: VOut;
    let d = i.pos - u.eye.xyz;
    let z = dot(d, u.fwd.xyz);
    let x = dot(d, u.right.xyz);
    let y = dot(d, u.up.xyz);
    let t = u.eye.w;
    let size = u.view.xy;
    let pp = u.view.zw;
    let near = u.params.x;
    let far = u.params.y;
    o.clip = vec4<f32>(
        x * size.y / (t * size.x) + (2.0 * pp.x / size.x - 1.0) * z,
        y / t + (1.0 - 2.0 * pp.y / size.y) * z,
        clamp((z - near) / (far - near), 0.0, 1.0) * z,
        z,
    );
    o.nrm = i.nrm;
    o.col = i.col.rgb;
    o.bary = i.bary.xyz;
    o.mask = i.bary.w * 255.0;
    o.wpos = i.pos;
    return o;
}

fn aces(x: vec3<f32>) -> vec3<f32> {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment fn fs_main(i: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var n = normalize(i.nrm);
    let v = normalize(u.eye.xyz - i.wpos);
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    let key = normalize(-u.right.xyz * 0.45 + u.up.xyz * 0.8 - u.fwd.xyz * 0.55);
    let fill = normalize(u.right.xyz * 0.7 - u.up.xyz * 0.2 - u.fwd.xyz * 0.6);
    let h = normalize(key + v);
    let sky = 0.5 + 0.5 * dot(n, u.up.xyz);
    let diff = max(dot(n, key), 0.0) * 0.95 + max(dot(n, fill), 0.0) * 0.18 + 0.05 + 0.07 * sky;
    let spec = pow(max(dot(n, h), 0.0), 40.0) * 0.18;
    let rim = pow(1.0 - max(dot(n, v), 0.0), 4.0) * 0.08;
    var c = i.col * diff + vec3<f32>(spec + rim);
    // real polygon edges only (bit k = edge opposite corner k)
    if (u.params.z > 0.5) {
        let m = u32(i.mask + 0.5);
        let w = fwidth(i.bary);
        let e = i.bary / max(w, vec3<f32>(1e-6));
        var d = 1e9;
        if ((m & 1u) != 0u) { d = min(d, e.x); }
        if ((m & 2u) != 0u) { d = min(d, e.y); }
        if ((m & 4u) != 0u) { d = min(d, e.z); }
        let line = 1.0 - smoothstep(0.3, 1.1, d);
        c = mix(c, c * 0.3, line * 0.75);
    }
    let o = pow(aces(c * 0.9), vec3<f32>(1.0 / 2.2));
    return vec4<f32>(o, 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub nrm: [f32; 3],
    pub col: [u8; 4],
    /// One-hot corner (x, y, z) and, in w, the mask of real polygon edges.
    pub bary: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    eye: [f32; 4],
    fwd: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    view: [f32; 4],
    params: [f32; 4],
}

/// Flat vertex list (3 per triangle) of a baked mesh.
pub fn mesh_vertices(b: &Baked, materials: &[Material]) -> Vec<MeshVertex> {
    let lin = |c: f32| (c.max(0.0).powf(2.2) * 255.0).round().clamp(0.0, 255.0) as u8;
    let mut out = Vec::with_capacity(b.triangle_count() * 3);
    for f in &b.faces {
        let m = f.verts.len();
        let c = materials.get(f.mat as usize).map(|m| m.color).unwrap_or([0.6, 0.6, 0.6]);
        let col = [lin(c[0]), lin(c[1]), lin(c[2]), 255];
        for t in b.triangulate_face(f) {
            let real = |a: usize, c: usize| (a + 1) % m == c || (c + 1) % m == a;
            let mask = (real(t[1], t[2]) as u8) | ((real(t[2], t[0]) as u8) << 1) | ((real(t[0], t[1]) as u8) << 2);
            for (k, &ci) in t.iter().enumerate() {
                let p = b.positions[f.verts[ci] as usize];
                let n = f.normals.get(ci).copied().unwrap_or_default();
                let mut bary = [0u8, 0, 0, mask];
                bary[k] = 255;
                out.push(MeshVertex {
                    pos: [p.x as f32, p.y as f32, p.z as f32],
                    nrm: [n.x as f32, n.y as f32, n.z as f32],
                    col,
                    bary,
                });
            }
        }
    }
    out
}

pub struct MeshRenderer {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    vbuf: Option<wgpu::Buffer>,
    count: u32,
    depth: Option<(wgpu::TextureView, [u32; 2])>,
    pub wire: bool,
    lo: glam::Vec3,
    hi: glam::Vec3,
}

const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

impl MeshRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("forme-mesh"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-bg"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("mesh-pl"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Unorm8x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<MeshVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attrs })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        MeshRenderer { pipeline, uniforms, bind, vbuf: None, count: 0, depth: None, wire: true, lo: glam::Vec3::ZERO, hi: glam::Vec3::ZERO }
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, verts: &[MeshVertex]) {
        self.count = verts.len() as u32;
        if verts.is_empty() {
            self.vbuf = None;
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(verts);
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-vertices"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buf, 0, bytes);
        self.vbuf = Some(buf);
        let mut lo = glam::Vec3::splat(f32::MAX);
        let mut hi = glam::Vec3::splat(f32::MIN);
        for v in verts {
            let p = glam::Vec3::from(v.pos);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        self.lo = lo;
        self.hi = hi;
    }

    pub fn has_mesh(&self) -> bool {
        self.vbuf.is_some()
    }

    /// Draws over `target` (keeps its colour, own depth buffer).
    pub fn draw(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, enc: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, size: [u32; 2], view: &View) {
        let Some(vbuf) = &self.vbuf else { return };
        if self.depth.as_ref().map(|d| d.1 != size).unwrap_or(true) {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mesh-depth"),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.depth = Some((tex.create_view(&Default::default()), size));
        }
        let (fwd, right, up) = view.basis();
        // depth range from the mesh bounds
        let mut zmin = f32::MAX;
        let mut zmax = f32::MIN;
        for i in 0..8 {
            let c = glam::Vec3::new(
                if i & 1 == 0 { self.lo.x } else { self.hi.x },
                if i & 2 == 0 { self.lo.y } else { self.hi.y },
                if i & 4 == 0 { self.lo.z } else { self.hi.z },
            );
            let z = (c - view.eye).dot(fwd);
            zmin = zmin.min(z);
            zmax = zmax.max(z);
        }
        let near = (zmin - 1.0).max(0.05);
        let far = (zmax + 1.0).max(near + 1.0);
        let u = Uniforms {
            eye: [view.eye.x, view.eye.y, view.eye.z, (view.fov_y * 0.5).tan()],
            fwd: [fwd.x, fwd.y, fwd.z, 0.0],
            right: [right.x, right.y, right.z, 0.0],
            up: [up.x, up.y, up.z, 0.0],
            view: [view.size[0], view.size[1], view.principal[0], view.principal[1]],
            params: [near, far, if self.wire { 1.0 } else { 0.0 }, 0.0],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));
        let depth = &self.depth.as_ref().unwrap().0;
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mesh"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, vbuf.slice(..));
        pass.draw(0..self.count, 0..1);
    }
}
