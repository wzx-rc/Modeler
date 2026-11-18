//! The 3D view: background and ground grid, the part's mesh (as the kernel
//! made it, each triangle knowing its B-rep face), its edges as thin
//! anti-aliased lines, and outlines over it (the selected cutter, the shape
//! being created; faint where the part hides them). Then the UI on top.
//!
//! The faces' state (selected, under the pointer) is a small buffer read by
//! the shader: selecting never re-uploads the mesh.

use crate::view::View;
use bytemuck::{Pod, Zeroable};

const SHADER: &str = r#"
struct U {
    eye: vec4<f32>,      // xyz, w = tan(fov / 2)
    fwd: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    view: vec4<f32>,     // size (px), projection centre (px)
    params: vec4<f32>,   // near, far, line width (px), light theme
    sel: vec4<f32>,      // selection colour (linear), mix
    hover: vec4<f32>,    // pre-selection colour (linear), mix
    base: vec4<f32>,     // part colour (linear), w = grid
    misc: vec4<f32>,     // x = world origin shown, y = zebra stripes
};
@group(0) @binding(0) var<uniform> u: U;
// per face: its material (linear colour, metal, roughness) and its state
struct FaceGpu {
    color: vec3<f32>,
    flags: u32,
    metal: f32,
    rough: f32,
    pad: vec2<f32>,
};
@group(0) @binding(1) var<storage, read> faces: array<FaceGpu>;

fn project(p: vec3<f32>) -> vec4<f32> {
    let d = p - u.eye.xyz;
    let z = dot(d, u.fwd.xyz);
    let x = dot(d, u.right.xyz);
    let y = dot(d, u.up.xyz);
    let t = u.eye.w;
    let size = u.view.xy;
    let pp = u.view.zw;
    let near = u.params.x;
    let far = u.params.y;
    // perspective depth (affine in 1/z): exact across long triangles
    let zd = far * (z - near) / (far - near);
    return vec4<f32>(
        x * size.y / (t * size.x) + (2.0 * pp.x / size.x - 1.0) * z,
        y / t + (1.0 - 2.0 * pp.y / size.y) * z,
        min(max(zd, 0.0), max(z, 0.0)),
        z,
    );
}

fn aces(x: vec3<f32>) -> vec3<f32> {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
}

// ------------------------------------------------------------ background

struct BgOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex fn vs_bg(@builtin(vertex_index) i: u32) -> BgOut {
    var o: BgOut;
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 1.0, 1.0);
    return o;
}

@fragment fn fs_bg(i: BgOut) -> @location(0) vec4<f32> {
    let frag = i.pos.xy;
    let half_h = u.view.y * 0.5;
    let sx = (frag.x - u.view.z) / half_h;
    let sy = -(frag.y - u.view.w) / half_h;
    let t = u.eye.w;
    let ro = u.eye.xyz;
    let rd = normalize(u.fwd.xyz + u.right.xyz * sx * t + u.up.xyz * sy * t);
    let px = t / half_h;
    let light = u.params.w > 0.5;
    let v = frag.y / u.view.y;
    var col = mix(vec3<f32>(0.0102, 0.0109, 0.0124), vec3<f32>(0.0074, 0.0079, 0.0090), v);
    if (light) {
        col = mix(vec3<f32>(0.47, 0.48, 0.50), vec3<f32>(0.40, 0.41, 0.43), v);
    }
    if (rd.y < -1e-4) {
        let tg = -ro.y / rd.y;
        let gp = ro + rd * tg;
        var gcol = vec3<f32>(0.0125, 0.0133, 0.0150);
        if (light) {
            gcol = vec3<f32>(0.40, 0.41, 0.43);
        }
        if (u.base.w > 0.5) {
            let foot = tg * px * 1.5;
            let c1 = abs(fract(gp.xz / 10.0 + 0.5) - 0.5) * 10.0;
            let l1 = 1.0 - smoothstep(0.0, foot + 0.05, min(c1.x, c1.y));
            let c2 = abs(fract(gp.xz / 50.0 + 0.5) - 0.5) * 50.0;
            let l2 = 1.0 - smoothstep(0.0, foot + 0.08, min(c2.x, c2.y));
            let fade = exp(-tg * 0.0016);
            if (light) {
                gcol = gcol - vec3<f32>(0.035, 0.035, 0.034) * l1 * fade - vec3<f32>(0.075, 0.075, 0.072) * l2 * fade;
            } else {
                gcol = gcol + vec3<f32>(0.010, 0.011, 0.013) * l1 * fade + vec3<f32>(0.022, 0.024, 0.028) * l2 * fade;
            }
            // world origin: X axis red, Z axis blue, a ring around it
            if (u.misc.x > 0.5) {
                let w = foot * 1.3 + 0.12;
                let ax = 1.0 - smoothstep(0.0, w, abs(gp.z));
                let az = 1.0 - smoothstep(0.0, w, abs(gp.x));
                let ring = 1.0 - smoothstep(0.0, w, abs(length(gp.xz) - 2.5));
                var red = vec3<f32>(0.42, 0.075, 0.07);
                var blue = vec3<f32>(0.07, 0.15, 0.45);
                var white = vec3<f32>(0.55, 0.56, 0.58);
                if (light) {
                    red = vec3<f32>(0.78, 0.2, 0.17);
                    blue = vec3<f32>(0.17, 0.36, 0.82);
                    white = vec3<f32>(0.1, 0.1, 0.11);
                }
                let f = 0.35 + 0.65 * fade;
                gcol = mix(gcol, red, ax * f);
                gcol = mix(gcol, blue, az * f);
                gcol = mix(gcol, white, ring);
            }
        }
        let fog = clamp(tg / 2600.0, 0.0, 1.0);
        col = mix(gcol, col, fog);
    }
    return vec4<f32>(aces(col * 1.9), 1.0);
}

// ------------------------------------------------------------------ mesh

struct MIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) face: u32,
};
struct MOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) nrm: vec3<f32>,
    @location(1) wpos: vec3<f32>,
    @location(2) @interpolate(flat) face: u32,
};

@vertex fn vs_mesh(i: MIn) -> MOut {
    var o: MOut;
    o.clip = project(i.pos);
    o.nrm = i.nrm;
    o.wpos = i.pos;
    o.face = i.face;
    return o;
}

@fragment fn fs_mesh(i: MOut) -> @location(0) vec4<f32> {
    var n = normalize(i.nrm);
    let v = normalize(u.eye.xyz - i.wpos);
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    let fc = faces[i.face];
    var base = fc.color;
    let flags = fc.flags;
    if ((flags & 1u) != 0u) {
        base = mix(base, u.sel.rgb, u.sel.a);
    } else if ((flags & 2u) != 0u) {
        base = mix(base, u.hover.rgb, u.hover.a);
    }
    let metal = fc.metal;
    let rough = fc.rough;
    let key = normalize(-u.right.xyz * 0.45 + u.up.xyz * 0.8 - u.fwd.xyz * 0.55);
    let fill = normalize(u.right.xyz * 0.7 - u.up.xyz * 0.2 - u.fwd.xyz * 0.6);
    let h = normalize(key + v);
    let sky = 0.5 + 0.5 * dot(n, u.up.xyz);
    let diff = max(dot(n, key), 0.0) * 0.95 + max(dot(n, fill), 0.0) * 0.18 + 0.05 + 0.07 * sky;
    // sharper, stronger and tinted highlights on metals; a sky / ground
    // reflection gives them their look
    let shine = mix(110.0, 8.0, rough);
    let spec = pow(max(dot(n, h), 0.0), shine) * mix(0.18, 0.9, metal) * (1.0 - 0.6 * rough);
    let rim = pow(1.0 - max(dot(n, v), 0.0), 4.0) * 0.08;
    let r = reflect(-v, n);
    let env = mix(vec3<f32>(0.10, 0.10, 0.11), vec3<f32>(0.85, 0.87, 0.9), clamp(0.5 + 0.5 * dot(r, u.up.xyz), 0.0, 1.0));
    let spec_col = mix(vec3<f32>(1.0), base, metal);
    let c = base * diff * (1.0 - 0.75 * metal) + base * env * metal * 0.85 + spec_col * spec + vec3<f32>(rim);
    if (u.misc.y > 0.5) {
        // zebra: stripes of the reflected direction (a surface check: flat
        // and true surfaces give straight, even stripes)
        let k = asin(clamp(dot(r, u.up.xyz), -1.0, 1.0)) * 36.0;
        let st = sin(k);
        let w = max(fwidth(st), 1e-4);
        let band = clamp(st / w * 0.5 + 0.5, 0.0, 1.0);
        let z = mix(vec3<f32>(0.035, 0.037, 0.04), vec3<f32>(0.82, 0.83, 0.85), band) * (0.75 + 0.25 * max(dot(n, v), 0.0));
        let tinted = mix(z, z * base * 1.6, select(0.0, 0.5, (flags & 3u) != 0u));
        return vec4<f32>(pow(tinted, vec3<f32>(1.0 / 2.2)), 1.0);
    }
    return vec4<f32>(pow(aces(c * 0.9), vec3<f32>(1.0 / 2.2)), 1.0);
}

// ----------------------------------------------------------------- lines

struct LIn {
    @location(0) pos: vec3<f32>,
    @location(1) other: vec3<f32>,
    @location(2) col: vec4<f32>,
    @location(3) flags: vec4<f32>,
};
struct LOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) col: vec4<f32>,
    // signed distance across the line (px), half width
    @location(1) @interpolate(linear) across: vec2<f32>,
};

// pulled a little towards the eye so a line wins over the faces it lies on
fn pulled(p: vec3<f32>) -> vec4<f32> {
    let d = u.eye.xyz - p;
    let dist = length(d);
    return project(p + d / max(dist, 1e-6) * (0.03 + 0.004 * dist));
}

// a segment is a quad `width` pixels wide plus one pixel of fringe on each
// side: each corner holds its own end (pos), the other end, its side
// (flags.x) and whether it is the far end (flags.y); flags.z scales the width
@vertex fn vs_line(i: LIn) -> LOut {
    var o: LOut;
    let a = pulled(i.pos);
    let b = pulled(i.other);
    let size = u.view.xy;
    var dir = (b.xy / b.w - a.xy / a.w) * size;
    if (dot(dir, dir) < 1e-10) {
        dir = vec2<f32>(1.0, 0.0);
    }
    dir = normalize(dir) * select(1.0, -1.0, i.flags.y > 0.5);
    let side = i.flags.x * 2.0 - 1.0;
    let half = u.params.z * 0.5 * max(i.flags.z * 4.0, 0.25);
    let reach = side * (half + 1.0);
    let off = vec2<f32>(-dir.y, dir.x) * reach * 2.0 / size;
    o.clip = vec4<f32>(a.xy + off * a.w, a.z, a.w);
    o.col = i.col;
    o.across = vec2<f32>(reach, half);
    return o;
}

@fragment fn fs_line(i: LOut) -> @location(0) vec4<f32> {
    let cover = clamp(i.across.y + 0.5 - abs(i.across.x), 0.0, 1.0);
    return vec4<f32>(i.col.rgb, i.col.a * cover);
}

// the hidden part of an outline, seen through the part
@fragment fn fs_line_hidden(i: LOut) -> @location(0) vec4<f32> {
    let cover = clamp(i.across.y + 0.5 - abs(i.across.x), 0.0, 1.0);
    return vec4<f32>(i.col.rgb, i.col.a * cover * 0.32);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub nrm: [f32; 3],
    pub face: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub other: [f32; 3],
    pub col: [u8; 4],
    /// side, far end, width (× 1/4 of the base width: 64 = base), unused
    pub flags: [u8; 4],
}

/// Quads of the polylines (`col` sRGB + opacity, `width` × the base width).
pub fn line_vertices(lines: &[Vec<[f32; 3]>], col: [u8; 4], width: f32, out: &mut Vec<LineVertex>) {
    let w = (width * 64.0).round().clamp(1.0, 255.0) as u8;
    for l in lines {
        for s in l.windows(2) {
            let (p, q) = (s[0], s[1]);
            let v = |at: [f32; 3], other: [f32; 3], side: u8, far: u8| LineVertex { pos: at, other, col, flags: [side, far, w, 0] };
            let (p0, p1, q0, q1) = (v(p, q, 0, 0), v(p, q, 255, 0), v(q, p, 0, 255), v(q, p, 255, 255));
            out.extend_from_slice(&[p0, p1, q1, p0, q1, q0]);
        }
    }
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
    sel: [f32; 4],
    hover: [f32; 4],
    base: [f32; 4],
    misc: [f32; 4],
}

/// Face flags read by the shader.
pub const FACE_SELECTED: u32 = 1;
pub const FACE_HOVERED: u32 = 2;

/// One face as the shader reads it: its material (linear colour) and flags.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FaceGpu {
    pub color: [f32; 3],
    pub flags: u32,
    pub metal: f32,
    pub rough: f32,
    pub pad: [f32; 2],
}

/// sRGB 0..1 to linear.
pub fn linear(c: [f32; 3]) -> [f32; 3] {
    c.map(lin)
}

/// Edge line width in points (`Frame::points_to_px` makes pixels of it).
const LINE_PT: f32 = 1.1;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const MSAA: u32 = 4;

/// What one image shows, besides the buffers already uploaded.
pub struct Frame<'a> {
    pub view: View,
    pub points_to_px: f32,
    pub light: bool,
    pub grid: bool,
    pub origin: bool,
    /// Zebra stripes instead of the clay.
    pub zebra: bool,
    /// Part colour (sRGB 0..1).
    pub base: [f32; 3],
    /// The second mesh drawn instead of the part (the original, Space held).
    pub alt: bool,
    /// Selection and pre-selection tints (sRGB 0..1) and their strength.
    pub sel: ([f32; 3], f32),
    pub hover: ([f32; 3], f32),
    pub paint: &'a [egui::ClippedPrimitive],
    pub textures: &'a mut egui::TexturesDelta,
}

struct Targets {
    size: [u32; 2],
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
}

struct Buf {
    buf: Option<wgpu::Buffer>,
    count: u32,
    cap: u64,
}

impl Buf {
    fn new() -> Buf {
        Buf { buf: None, count: 0, cap: 0 }
    }
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8], count: u32, label: &str) {
        self.count = count;
        if bytes.is_empty() {
            return;
        }
        if self.buf.is_none() || self.cap < bytes.len() as u64 {
            let cap = (bytes.len() as u64).next_power_of_two().max(1024);
            self.buf = Some(device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: cap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
            self.cap = cap;
        }
        queue.write_buffer(self.buf.as_ref().unwrap(), 0, bytes);
    }
}

pub struct Renderer {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    bg: wgpu::RenderPipeline,
    mesh: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    over_hidden: wgpu::RenderPipeline,
    over_visible: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    faces: wgpu::Buffer,
    faces_cap: u64,
    bind: wgpu::BindGroup,
    mesh_buf: Buf,
    alt_buf: Buf,
    edge_buf: Buf,
    over_buf: Buf,
    /// Bounds of what is uploaded (depth range).
    mesh_bounds: Option<([f32; 3], [f32; 3])>,
    over_bounds: Option<([f32; 3], [f32; 3])>,
    targets: Option<Targets>,
    pub egui: egui_wgpu::Renderer,
}

fn bounds_of<'a>(pts: impl Iterator<Item = &'a [f32; 3]>) -> Option<([f32; 3], [f32; 3])> {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut any = false;
    for p in pts {
        any = true;
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    any.then_some((lo, hi))
}

fn lin(c: f32) -> f32 {
    c.max(0.0).powf(2.2)
}

impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Renderer {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("nurbs-view"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("view-uniforms"), size: std::mem::size_of::<Uniforms>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let faces_cap = 1024u64;
        let faces = device.create_buffer(&wgpu::BufferDescriptor { label: Some("faces"), size: faces_cap * std::mem::size_of::<FaceGpu>() as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let bind = Self::bind_group(device, &layout, &uniforms, &faces);
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("view-pl"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let ms = wgpu::MultisampleState { count: MSAA, mask: !0, alpha_to_coverage_enabled: false };
        let depth = |write: bool, cmp: wgpu::CompareFunction| {
            Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(write), depth_compare: Some(cmp), stencil: Default::default(), bias: Default::default() })
        };
        let mesh_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32];
        let line_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Unorm8x4];
        let pipeline = |label: &str, vs: &str, fs: &str, attrs: Option<(&[wgpu::VertexAttribute], u64)>, blend: Option<wgpu::BlendState>, ds: Option<wgpu::DepthStencilState>| {
            let layouts: Vec<Option<wgpu::VertexBufferLayout>> = match attrs {
                Some((a, stride)) => vec![Some(wgpu::VertexBufferLayout { array_stride: stride, step_mode: wgpu::VertexStepMode::Vertex, attributes: a })],
                None => vec![],
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), buffers: &layouts, compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some(fs), targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: ds,
                multisample: ms,
                multiview_mask: None,
                cache: None,
            })
        };
        let mstride = std::mem::size_of::<MeshVertex>() as u64;
        let lstride = std::mem::size_of::<LineVertex>() as u64;
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        let bg = pipeline("background", "vs_bg", "fs_bg", None, None, depth(false, wgpu::CompareFunction::Always));
        let mesh = pipeline("mesh", "vs_mesh", "fs_mesh", Some((&mesh_attrs, mstride)), None, depth(true, wgpu::CompareFunction::Less));
        let lines = pipeline("edges", "vs_line", "fs_line", Some((&line_attrs, lstride)), alpha, depth(false, wgpu::CompareFunction::LessEqual));
        let over_hidden = pipeline("outline-hidden", "vs_line", "fs_line_hidden", Some((&line_attrs, lstride)), alpha, depth(false, wgpu::CompareFunction::Greater));
        let over_visible = pipeline("outline", "vs_line", "fs_line", Some((&line_attrs, lstride)), alpha, depth(false, wgpu::CompareFunction::LessEqual));
        Renderer {
            format,
            layout,
            bg,
            mesh,
            lines,
            over_hidden,
            over_visible,
            uniforms,
            faces,
            faces_cap,
            bind,
            mesh_buf: Buf::new(),
            alt_buf: Buf::new(),
            edge_buf: Buf::new(),
            over_buf: Buf::new(),
            mesh_bounds: None,
            over_bounds: None,
            targets: None,
            egui: egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default()),
        }
    }

    fn bind_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, uniforms: &wgpu::Buffer, faces: &wgpu::Buffer) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view-bg"),
            layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: faces.as_entire_binding() }],
        })
    }

    /// The part: triangles (each with its face index), and its edges.
    pub fn upload_part(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, verts: &[MeshVertex], edges: &[LineVertex]) {
        self.mesh_buf.upload(device, queue, bytemuck::cast_slice(verts), verts.len() as u32, "part-mesh");
        self.edge_buf.upload(device, queue, bytemuck::cast_slice(edges), edges.len() as u32, "part-edges");
        self.mesh_bounds = bounds_of(verts.iter().map(|v| &v.pos));
    }

    /// Part triangles from `first_vertex` on rewritten (same count).
    pub fn patch_part(&mut self, queue: &wgpu::Queue, first_vertex: u32, verts: &[MeshVertex]) {
        if let Some(buf) = &self.mesh_buf.buf {
            if first_vertex as usize + verts.len() <= self.mesh_buf.count as usize && !verts.is_empty() {
                queue.write_buffer(buf, first_vertex as u64 * std::mem::size_of::<MeshVertex>() as u64, bytemuck::cast_slice(verts));
            }
        }
    }

    /// The second mesh (drawn instead of the part when `Frame::alt`).
    pub fn upload_alt(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, verts: &[MeshVertex]) {
        self.alt_buf.upload(device, queue, bytemuck::cast_slice(verts), verts.len() as u32, "alt-mesh");
    }

    /// Each face's material and state (FACE_SELECTED, FACE_HOVERED).
    pub fn upload_faces(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, flags: &[FaceGpu]) {
        let size = std::mem::size_of::<FaceGpu>() as u64;
        let n = flags.len().max(1) as u64;
        if n > self.faces_cap {
            self.faces_cap = n.next_power_of_two();
            self.faces = device.create_buffer(&wgpu::BufferDescriptor { label: Some("faces"), size: self.faces_cap * size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            self.bind = Self::bind_group(device, &self.layout, &self.uniforms, &self.faces);
        }
        if !flags.is_empty() {
            queue.write_buffer(&self.faces, 0, bytemuck::cast_slice(flags));
        }
    }

    /// Outlines drawn over the part (empty: none).
    pub fn upload_overlay(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, lines: &[LineVertex]) {
        self.over_buf.upload(device, queue, bytemuck::cast_slice(lines), lines.len() as u32, "overlay");
        self.over_bounds = bounds_of(lines.iter().map(|v| &v.pos));
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.targets.as_ref().map(|t| t.size == size).unwrap_or(false) {
            return;
        }
        let tex = |format: wgpu::TextureFormat, label: &str| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: MSAA,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        self.targets = Some(Targets { size, color: tex(self.format, "view-msaa"), depth: tex(DEPTH, "view-depth") });
    }

    /// Draws the view and the UI into `target` (`size` pixels).
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, size: [u32; 2], f: Frame) {
        self.ensure_targets(device, size);
        let view = f.view;
        let (fwd, right, up) = view.basis();
        // depth range from what is drawn
        let mut zmin = f32::MAX;
        let mut zmax = f32::MIN;
        for (lo, hi) in [self.mesh_bounds, self.over_bounds].into_iter().flatten() {
            for i in 0..8 {
                let c = glam::Vec3::new(if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] });
                let z = (c - view.eye).dot(fwd);
                zmin = zmin.min(z);
                zmax = zmax.max(z);
            }
        }
        if zmin > zmax {
            (zmin, zmax) = (1.0, 2.0);
        }
        let near = (zmin - 1.0).max(0.05);
        let far = (zmax + 1.0).max(near + 1.0);
        let l3 = |c: [f32; 3]| [lin(c[0]), lin(c[1]), lin(c[2])];
        let (s, sa) = (l3(f.sel.0), f.sel.1);
        let (h, ha) = (l3(f.hover.0), f.hover.1);
        let b = l3(f.base);
        let u = Uniforms {
            eye: [view.eye.x, view.eye.y, view.eye.z, (view.fov_y * 0.5).tan()],
            fwd: [fwd.x, fwd.y, fwd.z, 0.0],
            right: [right.x, right.y, right.z, 0.0],
            up: [up.x, up.y, up.z, 0.0],
            view: [view.size[0], view.size[1], view.principal[0], view.principal[1]],
            params: [near, far, LINE_PT * f.points_to_px, f.light as u32 as f32],
            sel: [s[0], s[1], s[2], sa],
            hover: [h[0], h[1], h[2], ha],
            base: [b[0], b[1], b[2], f.grid as u32 as f32],
            misc: [f.origin as u32 as f32, f.zebra as u32 as f32, 0.0, 0.0],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));
        for (id, deltas) in &f.textures.set {
            for delta in deltas.iter() {
                self.egui.update_texture(device, queue, *id, delta);
            }
        }
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: f.points_to_px };
        let extra = self.egui.update_buffers(device, queue, &mut enc, f.paint, &sd);
        {
            let t = self.targets.as_ref().unwrap();
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("view"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.color,
                    depth_slice: None,
                    resolve_target: Some(target),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Discard },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &t.depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_pipeline(&self.bg);
            pass.draw(0..3, 0..1);
            let part = if f.alt && self.alt_buf.count > 0 { &self.alt_buf } else { &self.mesh_buf };
            if let (Some(buf), n) = (&part.buf, part.count) {
                if n > 0 {
                    pass.set_pipeline(&self.mesh);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..n, 0..1);
                }
            }
            if let (Some(buf), n) = (&self.edge_buf.buf, self.edge_buf.count) {
                if n > 0 {
                    pass.set_pipeline(&self.lines);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..n, 0..1);
                }
            }
            if let (Some(buf), n) = (&self.over_buf.buf, self.over_buf.count) {
                if n > 0 {
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.set_pipeline(&self.over_hidden);
                    pass.draw(0..n, 0..1);
                    pass.set_pipeline(&self.over_visible);
                    pass.draw(0..n, 0..1);
                }
            }
        }
        {
            let mut pass = enc
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.egui.render(&mut pass, f.paint, &sd);
        }
        queue.submit(extra.into_iter().chain(std::iter::once(enc.finish())));
        for id in &f.textures.free {
            self.egui.free_texture(id);
        }
        f.textures.clear();
    }
}
