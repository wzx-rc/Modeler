// Temporary: CPU raymarch of Romain's scene (SDF), as is and with Cube 3
// moved after the cylinder.
use forme_core::bake::field::Field;
use forme_core::glam::{DVec3};
use forme_core::program::compile;
use forme_core::*;
fn shot(f: &Field, path: &str, eye_dir: DVec3, target: DVec3, extent: f64, w: u32, h: u32) {
    let fwd = -eye_dir.normalize();
    let right = fwd.cross(DVec3::Y).normalize();
    let up = right.cross(fwd);
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([24, 26, 30]));
    let light = DVec3::new(0.4, 0.8, 0.5).normalize();
    for j in 0..h {
        for i in 0..w {
            let scale = w.min(h) as f64 / extent;
            let sx = (i as f64 - w as f64 * 0.5) / scale;
            let sy = (h as f64 * 0.5 - j as f64) / scale;
            let o = target + right * sx + up * sy - fwd * 200.0;
            let mut t = 0.0;
            let mut hit = None;
            for _ in 0..400 {
                let p = o + fwd * t;
                let d = f.dist(p);
                if d < 1e-3 {
                    hit = Some(p);
                    break;
                }
                t += d.max(1e-3) * 0.9;
                if t > 400.0 {
                    break;
                }
            }
            if let Some(p) = hit {
                let e = 1e-3;
                let n = DVec3::new(f.dist(p + DVec3::X * e) - f.dist(p - DVec3::X * e), f.dist(p + DVec3::Y * e) - f.dist(p - DVec3::Y * e), f.dist(p + DVec3::Z * e) - f.dist(p - DVec3::Z * e)).normalize();
                let s = 0.25 + 0.75 * n.dot(light).max(0.0);
                let c = [0.62 * s, 0.66 * s, 0.72 * s];
                img.put_pixel(i, j, image::Rgb([(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8]));
            }
        }
    }
    img.save(path).unwrap();
}
fn main() {
    let out = std::env::var("OUT").unwrap();
    let a = Scene::from_json(include_str!("scenes/romain-cylindre-arrondi.forme")).unwrap();
    let mut json: serde_json::Value = serde_json::from_str(include_str!("scenes/romain-cylindre-arrondi.forme")).unwrap();
    json["nodes"]["0"]["kind"]["Group"]["children"] = serde_json::json!([5, 10, 12, 11]);
    let b = Scene::from_json(&json.to_string()).unwrap();
    for (name, sc) in [("tel-quel", &a), ("cube3-apres", &b)] {
        let f = Field::new(&compile(sc));
        shot(&f, &format!("{out}/sdf-{name}.png"), DVec3::new(0.6, 0.8, 0.9), DVec3::new(39.0, 43.0, 47.0), 18.0, 1400, 1050);
        shot(&f, &format!("{out}/sdf-{name}-arriere.png"), DVec3::new(-0.35, 1.0, -0.5), DVec3::new(39.0, 43.0, 45.0), 17.0, 1400, 1050);
    }
}
