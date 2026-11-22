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
    params: vec4<f32>,   // near, far, line width (px), theme (0 dark, 1 light, 2 Studio dark, 3 Studio light)
    sel: vec4<f32>,      // selection colour (linear), mix
    hover: vec4<f32>,    // pre-selection colour (linear), mix
    base: vec4<f32>,     // part colour (linear), w = grid
    misc: vec4<f32>,     // x = world origin shown, y = deviation shown, z = its range (mm), w = its tolerance (mm)
    refc: vec4<f32>,     // reference mesh colour (linear), w = ghost opacity
};
@group(0) @binding(0) var<uniform> u: U;
// per face: its material (linear colour, metal, roughness, specular 0..1
// with 0.5 as the balls come, opacity) and its state
struct FaceGpu {
    color: vec3<f32>,
    flags: u32,
    metal: f32,
    rough: f32,
    spec: f32,
    alpha: f32,
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
    if (u.params.w > 1.5) {
        return studio_bg(frag, ro, rd, px, u.params.w > 2.5);
    }
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
            // Romain, 19 nov.: the grid lines barely there, white at about
            // 10 % (the 50 mm ones a hair more), no tint
            let l = max(l1 * 0.8, l2);
            if (light) {
                gcol = gcol - vec3<f32>(0.045, 0.045, 0.045) * l * fade;
            } else {
                gcol = gcol + vec3<f32>(0.0105, 0.0105, 0.0105) * l * fade;
            }
            // world origin: the main axes (X and Z) and a ring round it in a
            // light, airy grey
            if (u.misc.x > 0.5) {
                let w = foot * 1.3 + 0.12;
                let ax = 1.0 - smoothstep(0.0, w, abs(gp.z));
                let az = 1.0 - smoothstep(0.0, w, abs(gp.x));
                let ring = 1.0 - smoothstep(0.0, w, abs(length(gp.xz) - 2.5));
                var grey = vec3<f32>(0.068, 0.069, 0.072);
                if (light) {
                    grey = vec3<f32>(0.22, 0.225, 0.235);
                }
                let f = 0.35 + 0.65 * fade;
                gcol = mix(gcol, grey, max(ax, az) * f);
                gcol = mix(gcol, grey, ring * 0.8);
            }
        }
        let fog = clamp(tg / 2600.0, 0.0, 1.0);
        col = mix(gcol, col, fog);
    }
    return vec4<f32>(aces(col * 1.9), 1.0);
}

// The Studio interface's background (the mock-up D3, Romain 22 nov.): a
// soft radial studio light behind the part, the ground not drawn, only its
// grid lines fading out with the distance and towards the screen's edges.
// Its colours are the mock-up's own (sRGB), so no tone mapping.
fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
}

fn studio_bg(frag: vec2<f32>, ro: vec3<f32>, rd: vec3<f32>, px: f32, light: bool) -> vec4<f32> {
    let uv = frag / u.view.xy;
    let d = clamp(length((uv - vec2<f32>(0.5, 0.42)) / 0.75), 0.0, 1.0);
    var c0 = vec3<f32>(48.0, 52.0, 59.0) / 255.0;
    var c1 = vec3<f32>(35.0, 38.0, 43.0) / 255.0;
    var c2 = vec3<f32>(24.0, 25.0, 29.0) / 255.0;
    var line = vec3<f32>(54.0, 58.0, 66.0) / 255.0;
    var axis = vec3<f32>(74.0, 79.0, 88.0) / 255.0;
    if (light) {
        c0 = vec3<f32>(250.0, 251.0, 252.0) / 255.0;
        c1 = vec3<f32>(238.0, 240.0, 243.0) / 255.0;
        c2 = vec3<f32>(223.0, 227.0, 232.0) / 255.0;
        line = vec3<f32>(205.0, 210.0, 217.0) / 255.0;
        axis = vec3<f32>(179.0, 186.0, 195.0) / 255.0;
    }
    var col = mix(c0, c1, smoothstep(0.0, 0.6, d));
    col = mix(col, c2, smoothstep(0.6, 1.0, d));
    if (rd.y < -1e-4 && u.base.w > 0.5) {
        let tg = -ro.y / rd.y;
        let gp = ro + rd * tg;
        let foot = tg * px * 1.2;
        let c1g = abs(fract(gp.xz / 10.0 + 0.5) - 0.5) * 10.0;
        let l1 = 1.0 - smoothstep(0.0, foot + 0.04, min(c1g.x, c1g.y));
        let c2g = abs(fract(gp.xz / 50.0 + 0.5) - 0.5) * 50.0;
        let l2 = 1.0 - smoothstep(0.0, foot + 0.06, min(c2g.x, c2g.y));
        // fading with the distance, and towards the screen's edges (the
        // mock-up's mask)
        let mask = 1.0 - smoothstep(0.55, 1.05, length((uv - vec2<f32>(0.5, 0.58)) / 0.5));
        let fade = exp(-tg * 0.0022) * mask;
        col = mix(col, line, max(l1 * 0.75, l2) * fade);
        if (u.misc.x > 0.5) {
            let w = foot * 1.3 + 0.1;
            let ax = 1.0 - smoothstep(0.0, w, abs(gp.z));
            let az = 1.0 - smoothstep(0.0, w, abs(gp.x));
            col = mix(col, axis, max(ax, az) * fade);
        }
    }
    return vec4<f32>(to_linear(col), 1.0);
}

// ------------------------------------------------------------------ mesh

struct MIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) face: u32,
    @location(3) dev: f32,
};
struct MOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) nrm: vec3<f32>,
    @location(1) wpos: vec3<f32>,
    @location(2) @interpolate(flat) face: u32,
    @location(3) dev: f32,
};

@vertex fn vs_mesh(i: MIn) -> MOut {
    var o: MOut;
    o.clip = project(i.pos);
    // a surface with no thickness laid on the scan: pulled a little
    // towards the eye so the scan under it does not hide it
    if (i.face != 0xFFFFFFFFu && (faces[i.face].flags & 4u) != 0u) {
        let d = u.eye.xyz - i.pos;
        o.clip = project(i.pos + d * 0.02);
    }
    o.nrm = i.nrm;
    o.wpos = i.pos;
    o.face = i.face;
    o.dev = i.dev;
    return o;
}

// the part's distance to the reference (mm, + outside it): green within
// the tolerance, then yellow to red outside, cyan to blue inside (linear)
fn deviation_color(d: f32) -> vec3<f32> {
    let range = max(u.misc.z, 1e-4);
    let tol = u.misc.w;
    let green = vec3<f32>(0.05, 0.45, 0.10);
    if (abs(d) <= tol) {
        return green;
    }
    let t = clamp((abs(d) - tol) / max(range - tol, 1e-4), 0.0, 1.0);
    if (d > 0.0) {
        let yellow = vec3<f32>(0.80, 0.62, 0.02);
        let red = vec3<f32>(0.75, 0.04, 0.03);
        return select(mix(yellow, red, (t - 0.5) * 2.0), mix(green, yellow, t * 2.0), t < 0.5);
    }
    let cyan = vec3<f32>(0.03, 0.50, 0.70);
    let blue = vec3<f32>(0.04, 0.08, 0.75);
    return select(mix(cyan, blue, (t - 0.5) * 2.0), mix(green, cyan, t * 2.0), t < 0.5);
}

// the light on a face: what it scatters and its highlights (linear)
struct Lit {
    diff: vec3<f32>,
    spec: vec3<f32>,
};

fn tone(c: vec3<f32>) -> vec3<f32> {
    return pow(aces(c * 0.9), vec3<f32>(1.0 / 2.2));
}

// the part's faces, or the reference mesh (face 0xFFFFFFFF: its colour,
// matte)
fn light(i: MOut) -> Lit {
    var n = normalize(i.nrm);
    let v = normalize(u.eye.xyz - i.wpos);
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    var base = u.refc.rgb;
    var metal = 0.0;
    var rough = 0.85;
    // the material's specular: 1 as the balls come, 0 none, 2 twice
    var k = 1.0;
    // how much light it lets in under its surface (the flags' 2nd byte)
    var trans = 0.0;
    if (i.face != 0xFFFFFFFFu) {
        let fc = faces[i.face];
        base = fc.color;
        if (u.misc.y > 0.5 && abs(i.dev) < 1e8) {
            base = deviation_color(i.dev);
        }
        let flags = fc.flags;
        if ((flags & 1u) != 0u) {
            base = mix(base, u.sel.rgb, u.sel.a);
        } else if ((flags & 2u) != 0u) {
            base = mix(base, u.hover.rgb, u.hover.a);
        }
        metal = fc.metal;
        rough = fc.rough;
        k = fc.spec * 2.0;
        trans = f32((flags >> 8u) & 255u) / 255.0;
    }
    let key = normalize(-u.right.xyz * 0.45 + u.up.xyz * 0.8 - u.fwd.xyz * 0.55);
    let fill = normalize(u.right.xyz * 0.7 - u.up.xyz * 0.2 - u.fwd.xyz * 0.6);
    let h = normalize(key + v);
    let sky = 0.5 + 0.5 * dot(n, u.up.xyz);
    // a translucent material: the key light wraps further round it (the
    // light that went in comes out past the terminator)
    let wrap = 0.4 * trans;
    let dk = max((dot(n, key) + wrap) / (1.0 + wrap), 0.0);
    let diff = dk * 0.95 + max(dot(n, fill), 0.0) * 0.18 + 0.05 + 0.07 * sky;
    // sharper, stronger and tinted highlights on metals; a sky / ground
    // reflection gives them their look. Past its usual specular a
    // material gets glossier: a sharper highlight and a clear coat
    // reflecting the sky at grazing angles
    let gloss = max(k - 1.0, 0.0);
    let shine = mix(110.0, 8.0, rough) * (1.0 + 2.0 * gloss);
    let spec = pow(max(dot(n, h), 0.0), shine) * mix(0.18, 0.9, metal) * (1.0 - 0.6 * rough) * k * (1.0 + gloss);
    // a translucent material has no white rim (its edges go darker, below)
    let rim = pow(1.0 - max(dot(n, v), 0.0), 4.0) * 0.08 * (1.0 - trans);
    let r = reflect(-v, n);
    let env = mix(vec3<f32>(0.10, 0.10, 0.11), vec3<f32>(0.85, 0.87, 0.9), clamp(0.5 + 0.5 * dot(r, u.up.xyz), 0.0, 1.0));
    let spec_col = mix(vec3<f32>(1.0), base, metal);
    let fres = pow(1.0 - max(dot(n, v), 0.0), 5.0);
    let coat = env * (0.04 + 0.6 * fres * (1.0 - 0.85 * trans)) * gloss * (1.0 - metal) * 0.7;
    var o: Lit;
    o.diff = base * diff * (1.0 - 0.75 * metal) + base * env * metal * 0.85 * mix(0.35, 1.0, min(k, 1.0)) + vec3<f32>(rim);
    if (trans > 0.0) {
        // under the surface (the mock-up D3's clay, Romain 22 nov., 10 h 32
        // and 11 h 18: "c'est l'inverse le fresnel… pas blanc sur les
        // bords mais plus sombre"): the light that went in comes out where
        // the surface faces the eye, so the middle glows, and the shade is
        // lifted in a deeper tint of the colour (its hue at full
        // strength); towards the outline the light crosses more of it and
        // comes out deeper and darker, a fresnel turned round
        let top = max(max(base.r, base.g), max(base.b, 1e-3));
        let deep = base * (base / top);
        let nv = max(dot(n, v), 0.0);
        let shade = 1.0 - clamp(dk, 0.0, 1.0);
        let inner = mix(base, vec3<f32>(top), 0.2) * pow(nv, 2.0) * (0.5 + 0.5 * sky);
        o.diff = o.diff + trans * (deep * shade * 0.10 + inner * 0.16);
        let edge = pow(1.0 - nv, 1.25);
        o.diff = mix(o.diff, deep * (0.20 + 0.22 * dk), clamp(edge * trans * 0.95, 0.0, 1.0));
    }
    o.spec = spec_col * spec + coat;
    return o;
}

fn shade(i: MOut) -> vec3<f32> {
    let l = light(i);
    return tone(l.diff + l.spec);
}

@fragment fn fs_mesh(i: MOut) -> @location(0) vec4<f32> {
    // a transparent material is drawn after, see-through (`glass`)
    if (i.face != 0xFFFFFFFFu && faces[i.face].alpha < 0.995) {
        discard;
    }
    // a surface with no thickness is seen from both sides: light the side facing the eye
    var j = i;
    if (dot(j.nrm, u.eye.xyz - j.wpos) < 0.0) { j.nrm = -j.nrm; }
    return vec4<f32>(shade(j), 1.0);
}

// a transparent material (Romain, 20 nov., 15 h 42): its faces turned
// away from the eye first, then those facing it, blended over what the
// opaque faces left (premultiplied: the highlights stay bright); denser at
// grazing angles, as glass is
fn glass(i: MOut, front: bool) -> vec4<f32> {
    if (i.face == 0xFFFFFFFFu) {
        discard;
    }
    let fc = faces[i.face];
    if (fc.alpha >= 0.995) {
        discard;
    }
    var j = i;
    let facing = dot(j.nrm, u.eye.xyz - j.wpos) >= 0.0;
    if (facing != front) {
        discard;
    }
    if (!facing) { j.nrm = -j.nrm; }
    let l = light(j);
    let nv = abs(dot(normalize(j.nrm), normalize(u.eye.xyz - j.wpos)));
    var a = fc.alpha + (1.0 - fc.alpha) * pow(1.0 - nv, 3.0) * 0.55;
    if (!front) {
        a = a * 0.7;
    }
    let body = tone(l.diff + l.spec);
    let hl = tone(l.spec);
    return vec4<f32>(body * a + hl * (1.0 - a), a);
}

@fragment fn fs_glass_back(i: MOut) -> @location(0) vec4<f32> {
    return glass(i, false);
}

@fragment fn fs_glass_front(i: MOut) -> @location(0) vec4<f32> {
    return glass(i, true);
}

// X-ray: the part's nearest layer see-through, a little denser at its
// outline, so the scan behind it shows
@fragment fn fs_xray(i: MOut) -> @location(0) vec4<f32> {
    var j = i;
    if (dot(j.nrm, u.eye.xyz - j.wpos) < 0.0) { j.nrm = -j.nrm; }
    let n = normalize(j.nrm);
    let v = normalize(u.eye.xyz - j.wpos);
    let edge = pow(1.0 - abs(dot(n, v)), 2.0);
    return vec4<f32>(shade(j), mix(0.38, 0.7, edge));
}

// the reference seen through (Ghost): faint face on, denser at its outline
@fragment fn fs_ref_ghost(i: MOut) -> @location(0) vec4<f32> {
    let n = normalize(i.nrm);
    let v = normalize(u.eye.xyz - i.wpos);
    let edge = pow(1.0 - abs(dot(n, v)), 2.0);
    return vec4<f32>(shade(i), mix(u.refc.a, min(u.refc.a * 3.0, 0.8), edge));
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

// pulled a little towards the eye so a line wins over the faces it lies on:
// by about three pixels' worth at its depth (it was 0.03 mm + 0.4 % of the
// distance, about ten pixels: an edge just behind a face near its outline
// showed through it; Romain 20 nov., 9 h 30)
fn pulled(p: vec3<f32>) -> vec4<f32> {
    let d = u.eye.xyz - p;
    let dist = length(d);
    let z = max(dot(p - u.eye.xyz, u.fwd.xyz), 1e-3);
    let px = 2.0 * z * u.eye.w / max(u.view.y, 1.0);
    return project(p + d / max(dist, 1e-6) * (0.005 + 3.0 * px));
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
    /// The distance to the reference mesh (mm, + outside it), `NO_DEV`
    /// when none.
    pub dev: f32,
}

/// A vertex with no distance to the reference (not coloured).
pub const NO_DEV: f32 = 1e9;

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
    refc: [f32; 4],
}

/// Face flags read by the shader.
pub const FACE_SELECTED: u32 = 1;
pub const FACE_HOVERED: u32 = 2;
/// A surface with no thickness (drawn a little towards the eye).
pub const FACE_SHEET: u32 = 4;

/// One face as the shader reads it: its material (linear colour) and flags.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FaceGpu {
    pub color: [f32; 3],
    pub flags: u32,
    pub metal: f32,
    pub rough: f32,
    /// The highlights' strength, 0..1 (0.5 as the balls come).
    pub spec: f32,
    /// Opacity (below 1: drawn see-through after the opaque faces).
    pub alpha: f32,
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
    /// The Studio interface: its soft studio background (`fs_bg`).
    pub studio: bool,
    pub grid: bool,
    pub origin: bool,
    /// Part colour (sRGB 0..1).
    pub base: [f32; 3],
    /// Selection and pre-selection tints (sRGB 0..1) and their strength.
    pub sel: ([f32; 3], f32),
    pub hover: ([f32; 3], f32),
    /// The reference mesh: not drawn, solid, or see-through (Ghost); its
    /// colour (sRGB 0..1).
    pub reference: RefDraw,
    pub ref_color: [f32; 3],
    /// X-ray: the part see-through (its nearest layer), what is behind it shown.
    pub xray: bool,
    /// The part coloured by its distance to the reference: the range and
    /// the tolerance (mm).
    pub deviation: Option<(f32, f32)>,
    pub paint: &'a [egui::ClippedPrimitive],
    pub textures: &'a mut egui::TexturesDelta,
}

/// How the reference mesh is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefDraw {
    Hidden,
    Solid,
    Ghost,
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
    /// The reference: solid (pushed back a little in depth, so the part's
    /// faces laid on it win), its depth alone, and seen through.
    ref_solid: wgpu::RenderPipeline,
    ref_depth: wgpu::RenderPipeline,
    ref_ghost: wgpu::RenderPipeline,
    /// X-ray: the part's depth, then its colour see-through.
    xray_depth: wgpu::RenderPipeline,
    xray: wgpu::RenderPipeline,
    /// Transparent materials: their back faces, then their front faces.
    glass_back: wgpu::RenderPipeline,
    glass_front: wgpu::RenderPipeline,
    /// Some face has a transparent material.
    glass: bool,
    uniforms: wgpu::Buffer,
    faces: wgpu::Buffer,
    faces_cap: u64,
    bind: wgpu::BindGroup,
    mesh_buf: Buf,
    edge_buf: Buf,
    over_buf: Buf,
    ref_buf: Buf,
    /// Bounds of what is uploaded (depth range).
    mesh_bounds: Option<([f32; 3], [f32; 3])>,
    over_bounds: Option<([f32; 3], [f32; 3])>,
    ref_bounds: Option<([f32; 3], [f32; 3])>,
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
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
        let pushed = |write: bool| {
            Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(write),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: 8, slope_scale: 1.5, clamp: 0.0 },
            })
        };
        let mesh_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32, 3 => Float32];
        let line_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Unorm8x4];
        let pipeline_mask = |label: &str, vs: &str, fs: &str, attrs: Option<(&[wgpu::VertexAttribute], u64)>, blend: Option<wgpu::BlendState>, ds: Option<wgpu::DepthStencilState>, write_mask: wgpu::ColorWrites| {
            let layouts: Vec<Option<wgpu::VertexBufferLayout>> = match attrs {
                Some((a, stride)) => vec![Some(wgpu::VertexBufferLayout { array_stride: stride, step_mode: wgpu::VertexStepMode::Vertex, attributes: a })],
                None => vec![],
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), buffers: &layouts, compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some(fs), targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask })], compilation_options: Default::default() }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: ds,
                multisample: ms,
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = |label: &str, vs: &str, fs: &str, attrs: Option<(&[wgpu::VertexAttribute], u64)>, blend: Option<wgpu::BlendState>, ds: Option<wgpu::DepthStencilState>| pipeline_mask(label, vs, fs, attrs, blend, ds, wgpu::ColorWrites::ALL);
        let mstride = std::mem::size_of::<MeshVertex>() as u64;
        let lstride = std::mem::size_of::<LineVertex>() as u64;
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        let bg = pipeline("background", "vs_bg", "fs_bg", None, None, depth(false, wgpu::CompareFunction::Always));
        let mesh = pipeline("mesh", "vs_mesh", "fs_mesh", Some((&mesh_attrs, mstride)), None, depth(true, wgpu::CompareFunction::Less));
        let lines = pipeline("edges", "vs_line", "fs_line", Some((&line_attrs, lstride)), alpha, depth(false, wgpu::CompareFunction::LessEqual));
        let over_hidden = pipeline("outline-hidden", "vs_line", "fs_line_hidden", Some((&line_attrs, lstride)), alpha, depth(false, wgpu::CompareFunction::Greater));
        let over_visible = pipeline("outline", "vs_line", "fs_line", Some((&line_attrs, lstride)), alpha, depth(false, wgpu::CompareFunction::LessEqual));
        let ref_solid = pipeline("reference", "vs_mesh", "fs_mesh", Some((&mesh_attrs, mstride)), None, pushed(true));
        let ref_depth = pipeline_mask("reference-depth", "vs_mesh", "fs_mesh", Some((&mesh_attrs, mstride)), None, pushed(true), wgpu::ColorWrites::empty());
        let ref_ghost = pipeline("reference-ghost", "vs_mesh", "fs_ref_ghost", Some((&mesh_attrs, mstride)), alpha, depth(false, wgpu::CompareFunction::LessEqual));
        let xray_depth = pipeline_mask("xray-depth", "vs_mesh", "fs_mesh", Some((&mesh_attrs, mstride)), None, depth(true, wgpu::CompareFunction::Less), wgpu::ColorWrites::empty());
        let xray = pipeline("xray", "vs_mesh", "fs_xray", Some((&mesh_attrs, mstride)), alpha, depth(false, wgpu::CompareFunction::LessEqual));
        let pre = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let glass_back = pipeline("glass-back", "vs_mesh", "fs_glass_back", Some((&mesh_attrs, mstride)), pre, depth(false, wgpu::CompareFunction::LessEqual));
        let glass_front = pipeline("glass-front", "vs_mesh", "fs_glass_front", Some((&mesh_attrs, mstride)), pre, depth(false, wgpu::CompareFunction::LessEqual));
        Renderer {
            format,
            layout,
            bg,
            mesh,
            lines,
            over_hidden,
            over_visible,
            ref_solid,
            ref_depth,
            ref_ghost,
            xray_depth,
            xray,
            glass_back,
            glass_front,
            glass: false,
            uniforms,
            faces,
            faces_cap,
            bind,
            mesh_buf: Buf::new(),
            edge_buf: Buf::new(),
            over_buf: Buf::new(),
            ref_buf: Buf::new(),
            mesh_bounds: None,
            over_bounds: None,
            ref_bounds: None,
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

    /// The reference mesh's triangles (empty: none).
    pub fn upload_reference(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, verts: &[MeshVertex]) {
        self.ref_buf.upload(device, queue, bytemuck::cast_slice(verts), verts.len() as u32, "reference");
        self.ref_bounds = bounds_of(verts.iter().map(|v| &v.pos));
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
        self.glass = flags.iter().any(|f| f.alpha < 0.995);
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
        let refb = if f.reference == RefDraw::Hidden { None } else { self.ref_bounds };
        for (lo, hi) in [self.mesh_bounds, self.over_bounds, refb].into_iter().flatten() {
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
            params: [near, far, LINE_PT * f.points_to_px, f.light as u32 as f32 + if f.studio { 2.0 } else { 0.0 }],
            sel: [s[0], s[1], s[2], sa],
            hover: [h[0], h[1], h[2], ha],
            base: [b[0], b[1], b[2], f.grid as u32 as f32],
            misc: match f.deviation {
                Some((range, tol)) => [f.origin as u32 as f32, 1.0, range, tol],
                None => [f.origin as u32 as f32, 0.0, 1.0, 0.0],
            },
            refc: {
                let c = l3(f.ref_color);
                [c[0], c[1], c[2], 0.22]
            },
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
            let refm = match (&self.ref_buf.buf, self.ref_buf.count) {
                (Some(buf), n) if n > 0 => Some((buf, n)),
                _ => None,
            };
            if let (Some((buf, n)), RefDraw::Solid) = (refm, f.reference) {
                pass.set_pipeline(&self.ref_solid);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..n, 0..1);
            }
            if let (Some(buf), n) = (&self.mesh_buf.buf, self.mesh_buf.count) {
                if n > 0 && f.xray {
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.set_pipeline(&self.xray_depth);
                    pass.draw(0..n, 0..1);
                    pass.set_pipeline(&self.xray);
                    pass.draw(0..n, 0..1);
                } else if n > 0 {
                    pass.set_pipeline(&self.mesh);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..n, 0..1);
                    if self.glass {
                        pass.set_pipeline(&self.glass_back);
                        pass.draw(0..n, 0..1);
                        pass.set_pipeline(&self.glass_front);
                        pass.draw(0..n, 0..1);
                    }
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
            // seen through: its nearest layer only (its depth first), over
            // the part and the outlines
            if let (Some((buf, n)), RefDraw::Ghost) = (refm, f.reference) {
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.set_pipeline(&self.ref_depth);
                pass.draw(0..n, 0..1);
                pass.set_pipeline(&self.ref_ghost);
                pass.draw(0..n, 0..1);
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
