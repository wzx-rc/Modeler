use forme_core::{compile, demo};
use forme_render::{headless::Gpu, Renderer, Settings, View};
use glam::Vec3;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let w: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(960);
    let h: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(670);
    let out = args.get(3).cloned().unwrap_or("grenade.png".into());
    let gpu = Gpu::new().expect("gpu");
    eprintln!("adapter: {}", gpu.adapter_name());
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(&gpu.device, fmt);
    let scene = demo::grenade();
    let prog = compile(&scene);
    r.set_scene(&gpu.device, &gpu.queue, &scene, &prog, None);
    let target = Vec3::new(10.0, 78.0, 0.0);
    let yaw: f32 = 0.42;
    let pitch: f32 = 0.16;
    let dist = 330.0;
    let eye = target + Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos()) * dist;
    let view = View { eye, target, fov_y: 0.52, size: [w as f32, h as f32], principal: [w as f32 * 0.5, h as f32 * 0.5] };
    r.update_view(&gpu.queue, &view, &Settings::default());
    let tex = gpu.target(w, h, fmt);
    let tv = tex.create_view(&Default::default());
    let t0 = Instant::now();
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &tv,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        r.draw(&mut pass);
    }
    gpu.queue.submit([enc.finish()]);
    let px = gpu.read_rgba(&tex);
    eprintln!("frame: {:.2?} ({} prims)", t0.elapsed(), prog.prim_count);
    image::save_buffer(&out, &px, w, h, image::ExtendedColorType::Rgba8).unwrap();
    eprintln!("saved {out}");
}
