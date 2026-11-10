// Forme viewport: raymarches the scene bytecode (see forme-core/src/program.rs).
// The interpreter below must stay in sync with Program::eval on the CPU.

struct Globals {
    eye: vec4<f32>,       // xyz eye, w tan(fov_y / 2)
    fwd: vec4<f32>,       // xyz forward
    right: vec4<f32>,     // xyz right, w selection program start (vec4 index), -1 = none
    up: vec4<f32>,        // xyz up, w selection mode (0 none, 1 tint, 2 ghost cutter)
    viewport: vec4<f32>,  // width, height (px), principal point x, y (px)
    scene_lo: vec4<f32>,  // xyz bounds min, w ground height
    scene_hi: vec4<f32>,  // xyz bounds max, w show grid (0/1)
    sel_color: vec4<f32>,
    ghost_color: vec4<f32>,
    params: vec4<f32>,    // x max steps, y main program present, z shadows (0/1), w ao (0/1)
    mats: array<vec4<f32>, 16>,  // rgb, metal
    mats2: array<vec4<f32>, 16>, // roughness, -, -, -
    light: vec4<f32>,     // xyz towards the key light (unit), w intensity
    extra: vec4<f32>,     // xy sub-pixel jitter (anti-aliasing), z exposure
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var<storage, read> code: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> aux: array<vec4<f32>>;

const OP_END: u32 = 0u;
const OP_EMPTY: u32 = 1u;
const OP_RESET: u32 = 2u;
const OP_AFFINE: u32 = 3u;
const OP_RADIAL: u32 = 4u;
const OP_LINEAR: u32 = 5u;
const OP_MIRROR: u32 = 6u;
const OP_PRIM: u32 = 7u;
const OP_COMBINE: u32 = 8u;
const OP_BOUND: u32 = 9u;
const EMPTY_DIST: f32 = 100000.0;
const SQRT1_2: f32 = 0.70710678;
const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318531;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

fn extrude(d2: f32, h: f32) -> f32 {
    let w = vec2<f32>(d2, h);
    return min(max(w.x, w.y), 0.0) + length(max(w, vec2<f32>(0.0)));
}

fn aux_pt(k: u32) -> vec2<f32> {
    let v = aux[k / 2u];
    if ((k & 1u) == 0u) {
        return v.xy;
    }
    return v.zw;
}

fn sd_polygon(off: u32, count: u32, p: vec2<f32>) -> f32 {
    let v0 = aux_pt(off);
    var d = dot(p - v0, p - v0);
    var s = 1.0;
    var j = count - 1u;
    for (var i = 0u; i < count; i = i + 1u) {
        let vi = aux_pt(off + i);
        let vj = aux_pt(off + j);
        let e = vj - vi;
        let w = p - vi;
        let b = w - e * clamp(dot(w, e) / max(dot(e, e), 1e-12), 0.0, 1.0);
        d = min(d, dot(b, b));
        let c1 = p.y >= vi.y;
        let c2 = p.y < vj.y;
        let c3 = e.x * w.y > e.y * w.x;
        if ((c1 && c2 && c3) || (!c1 && !c2 && !c3)) {
            s = -s;
        }
        j = i;
    }
    return s * sqrt(d);
}

fn sd_regular_polygon(p: vec2<f32>, r: f32, n: f32) -> f32 {
    let an = PI / n;
    let a = atan2(p.x, p.y);
    let m = 2.0 * an;
    let bn = (a - m * floor(a / m)) - an;
    let l = length(p);
    var q = vec2<f32>(l * cos(bn), l * abs(sin(bn)));
    q = q - r * vec2<f32>(cos(an), sin(an));
    q.y = q.y + clamp(-q.y, 0.0, r * sin(an));
    return length(q) * sign(q.x);
}

fn min3(v: vec3<f32>) -> f32 {
    return min(v.x, min(v.y, v.z));
}

// Per-edge fillets (radius > 0) or chamfers (< 0), stored in aux.
fn edge_r(off: u32, i: u32) -> f32 {
    let v = aux[off / 2u + i / 4u];
    return v[i % 4u];
}

fn wedge(qb: f32, qc: f32, r: f32, cap: f32) -> f32 {
    if (r < 0.0) {
        return max(max(qb, qc), (qb + qc - r) * SQRT1_2);
    }
    let rr = min(r, cap);
    let v = vec2<f32>(qb + rr, qc + rr);
    return length(max(v, vec2<f32>(0.0))) + min(max(v.x, v.y), 0.0) - rr;
}

fn oct_k(sb: f32, sc: f32) -> u32 {
    if (sb >= 0.0) {
        if (sc >= 0.0) { return 0u; }
        return 3u;
    }
    if (sc >= 0.0) { return 1u; }
    return 2u;
}

fn box_edges(p: vec3<f32>, size: vec3<f32>, off: u32) -> f32 {
    let q = abs(p) - size;
    let r0 = edge_r(off, oct_k(p.y, p.z));
    let r1 = edge_r(off, 4u + oct_k(p.z, p.x));
    let r2 = edge_r(off, 8u + oct_k(p.x, p.y));
    var d = max(wedge(q.y, q.z, r0, min(size.y, size.z)), max(wedge(q.z, q.x, r1, min(size.z, size.x)), wedge(q.x, q.y, r2, min(size.x, size.y))));
    let rm = min(min(r0, r1), min(r2, min3(size)));
    if (rm > 0.0) {
        let qs = q + vec3<f32>(rm);
        d = max(d, length(max(qs, vec3<f32>(0.0))) + min(max(qs.x, max(qs.y, qs.z)), 0.0) - rm);
    }
    return d;
}

fn cyl_edges(p: vec3<f32>, size: vec3<f32>, r: f32) -> f32 {
    let dx = length(p.xz) - size.x;
    let dy = abs(p.y) - size.y;
    if (r < 0.0) {
        return max(extrude(dx, dy), (dx + dy - r) * SQRT1_2);
    }
    let rr = min(r, min(size.x, size.y));
    return extrude(dx + rr, dy + rr) - rr;
}

fn eval_prim(pc: u32, p_in: vec3<f32>) -> f32 {
    let w0 = code[pc];
    let w1 = code[pc + 1u];
    let w2 = code[pc + 2u];
    let w3 = code[pc + 3u];
    let kind = u32(w0.y);
    let size = w1.xyz;
    let rnd = max(w1.w, 0.0);
    let chamfer = max(w2.x, 0.0);
    let taper = w2.y;
    let shell = w2.z;
    let sides = w2.w;
    var p = p_in;
    var lip = 1.0;
    if (abs(taper) > 1e-4 && kind != 8u) {
        let hy = max(size.y, 1e-3);
        let t = clamp(p.y / (2.0 * hy) + 0.5, 0.0, 1.0);
        let f = max(1.0 - taper * t, 0.05);
        p.x = p.x / f;
        p.z = p.z / f;
        lip = min(f, 1.0) * 0.85;
    }
    var d = EMPTY_DIST;
    let per_edge = w3.w > 0.5;
    let eoff = u32(w3.x);
    switch kind {
        case 0u: {
          if (per_edge) {
            d = box_edges(p, size, eoff);
          } else {
            let r = min(rnd, min3(size));
            let q = abs(p) - (size - vec3<f32>(r));
            d = length(max(q, vec3<f32>(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0) - r;
            if (chamfer > 0.0) {
                let q2 = abs(p) - size;
                d = max(d, (q2.x + q2.y + chamfer) * SQRT1_2);
                d = max(d, (q2.x + q2.z + chamfer) * SQRT1_2);
                d = max(d, (q2.y + q2.z + chamfer) * SQRT1_2);
            }
          }
        }
        case 1u: {
          if (per_edge) {
            var r = edge_r(eoff, 1u);
            if (p.y >= 0.0) { r = edge_r(eoff, 0u); }
            d = cyl_edges(p, size, r);
          } else {
            let r = min(rnd, min(size.x, size.y));
            let dx = length(p.xz) - size.x;
            let dy = abs(p.y) - size.y;
            d = extrude(dx + r, dy + r) - r;
            if (chamfer > 0.0) {
                d = max(d, (dx + dy + chamfer) * SQRT1_2);
            }
          }
        }
        case 2u: {
            let r = max(size, vec3<f32>(1e-3));
            let k0 = length(p / r);
            let k1 = length(p / (r * r));
            if (k1 < 1e-6) {
                d = -min3(r);
            } else {
                d = k0 * (k0 - 1.0) / k1;
            }
        }
        case 3u: {
            var q = p;
            q.y = q.y - clamp(q.y, -size.y, size.y);
            d = length(q) - size.x;
        }
        case 4u: {
            let q = vec2<f32>(length(p.xz) - size.x, p.y);
            d = length(q) - size.z;
        }
        case 5u: {
            let r = min(rnd, size.y);
            let h = size.y - r;
            let r1 = max(size.x - r, 0.0);
            let r2 = max(size.z - r, 0.0);
            let q = vec2<f32>(length(p.xz), p.y);
            let k1 = vec2<f32>(r2, h);
            let k2 = vec2<f32>(r2 - r1, 2.0 * h);
            var rr = r2;
            if (q.y < 0.0) {
                rr = r1;
            }
            let ca = vec2<f32>(q.x - min(q.x, rr), abs(q.y) - h);
            let cb = q - k1 + k2 * clamp(dot(k1 - q, k2) / max(dot(k2, k2), 1e-9), 0.0, 1.0);
            var s = 1.0;
            if (cb.x < 0.0 && ca.y < 0.0) {
                s = -1.0;
            }
            d = s * sqrt(min(dot(ca, ca), dot(cb, cb))) - r;
        }
        case 6u: {
            let r = min(rnd, size.y);
            let d2 = sd_regular_polygon(p.xz, size.x - r, max(sides, 3.0));
            d = extrude(d2, abs(p.y) - size.y + r) - r;
        }
        case 7u: {
            let r = rnd;
            let mid = (size.x + size.z) * 0.5;
            let hw = max((size.x - size.z) * 0.5, 1e-3);
            let d2 = abs(length(p.xz) - mid) - hw;
            d = extrude(d2 + r, abs(p.y) - size.y + r) - r;
        }
        case 8u: {
            let off = u32(w3.x);
            let count = u32(w3.y);
            if (count >= 3u) {
                if (w3.z > 0.5) {
                    let q = vec2<f32>(length(p.xz), p.y);
                    d = sd_polygon(off, count, q) - rnd;
                } else {
                    let d2 = sd_polygon(off, count, p.xy);
                    d = extrude(d2 + rnd, abs(p.z) - size.z + rnd) - rnd;
                }
            }
        }
        default: {}
    }
    if (shell > 0.0) {
        d = abs(d) - shell;
    }
    return d * lip;
}

fn combine(a: vec2<f32>, b: vec2<f32>, w0: vec4<f32>, w1: vec4<f32>) -> vec2<f32> {
    let mode = u32(w0.y);
    let k = w0.z;
    let chamfer = w0.w > 0.5;
    let depth = w1.x;
    let width = w1.y;
    let da = a.x;
    let db = b.x;
    if (mode == 0u) {
        var m = a.y;
        if (db < da) {
            m = b.y;
        }
        var d = min(da, db);
        if (k > 0.0) {
            if (chamfer) {
                d = min(d, (da + db - k) * SQRT1_2);
            } else {
                let h = max(k - abs(da - db), 0.0) / k;
                d = d - h * h * k * 0.25;
            }
        }
        return vec2<f32>(d, m);
    }
    if (mode == 1u || mode == 2u) {
        var nb = db;
        if (mode == 1u) {
            nb = -db;
        }
        var m = a.y;
        if (nb > da) {
            m = b.y;
        }
        var d = max(da, nb);
        if (k > 0.0) {
            if (chamfer) {
                d = max(d, (da + nb + k) * SQRT1_2);
            } else {
                let h = max(k - abs(da - nb), 0.0) / k;
                d = d + h * h * k * 0.25;
            }
        }
        return vec2<f32>(d, m);
    }
    if (mode == 3u) {
        return vec2<f32>(max(da, min(da + depth, width - abs(db))), a.y);
    }
    if (mode == 4u) {
        let t = max(da - depth, abs(db) - width);
        var m = a.y;
        if (t < da) {
            m = b.y;
        }
        return vec2<f32>(min(da, t), m);
    }
    return a;
}

// Runs the program starting at vec4 index `start`. Returns (distance, material).
fn run(start: u32, pw: vec3<f32>) -> vec2<f32> {
    var stack: array<vec2<f32>, 16>;
    var sp = 0u;
    var p = pw;
    var sc = 1.0;
    var pc = start;
    let n = arrayLength(&code);
    for (var guard = 0u; guard < 4096u; guard = guard + 1u) {
        if (pc + 4u > n) {
            break;
        }
        let w0 = code[pc];
        let op = u32(w0.x);
        if (op == OP_END) {
            break;
        }
        var next = pc + 4u;
        switch op {
            case 1u: {
                if (sp < 16u) {
                    stack[sp] = vec2<f32>(EMPTY_DIST, 0.0);
                    sp = sp + 1u;
                }
            }
            case 9u: {
                let b = code[pc + 1u];
                let bd = length(pw - b.xyz) - b.w;
                var a = EMPTY_DIST;
                if (sp > 0u) {
                    a = stack[sp - 1u].x;
                }
                if (bd > abs(a) + w0.z + 0.5) {
                    var neutral = EMPTY_DIST;
                    if (u32(w0.y) == 2u) {
                        neutral = bd;
                    }
                    if (sp < 16u) {
                        stack[sp] = vec2<f32>(neutral, 0.0);
                        sp = sp + 1u;
                    }
                    next = pc + (u32(w0.w) + 1u) * 4u;
                }
            }
            case 2u: {
                p = pw;
                sc = 1.0;
            }
            case 3u: {
                let r0 = code[pc + 1u];
                let r1 = code[pc + 2u];
                let r2 = code[pc + 3u];
                let p4 = vec4<f32>(p, 1.0);
                p = vec3<f32>(dot(r0, p4), dot(r1, p4), dot(r2, p4));
                sc = sc * w0.y;
            }
            case 4u: {
                let cnt = max(w0.y, 1.0);
                let axis = u32(w0.z);
                var u = p.x;
                var v = p.y;
                if (axis == 0u) {
                    u = p.y;
                    v = p.z;
                } else if (axis == 1u) {
                    u = p.z;
                    v = p.x;
                }
                let sector = TAU / cnt;
                let a = atan2(v, u);
                let a2 = a + sector * 0.5;
                var a3 = a2 - sector * floor(a2 / sector) - sector * 0.5;
                if (w0.w > 0.5 && cnt > 1.5) {
                    a3 = a3 - select(-sector, sector, a3 >= 0.0);
                }
                let l = sqrt(u * u + v * v);
                let nu = l * cos(a3);
                let nv = l * sin(a3);
                if (axis == 0u) {
                    p.y = nu;
                    p.z = nv;
                } else if (axis == 1u) {
                    p.z = nu;
                    p.x = nv;
                } else {
                    p.x = nu;
                    p.y = nv;
                }
            }
            case 5u: {
                let cnt = max(w0.y, 1.0);
                let stp = code[pc + 1u].xyz;
                let origin = code[pc + 2u].xyz;
                let ss = dot(stp, stp);
                if (ss > 1e-9) {
                    let uu = dot(p - origin, stp) / ss;
                    var t = clamp(round(uu), 0.0, cnt - 1.0);
                    if (w0.z > 0.5) {
                        t = clamp(t + select(-1.0, 1.0, uu > t), 0.0, cnt - 1.0);
                    }
                    p = p - stp * t;
                }
            }
            case 6u: {
                if (w0.y > 0.5) {
                    p.x = abs(p.x);
                }
                if (w0.z > 0.5) {
                    p.y = abs(p.y);
                }
                if (w0.w > 0.5) {
                    p.z = abs(p.z);
                }
            }
            case 7u: {
                let d = eval_prim(pc, p) * sc;
                if (sp < 16u) {
                    stack[sp] = vec2<f32>(d, w0.z);
                    sp = sp + 1u;
                }
            }
            case 8u: {
                if (sp >= 2u) {
                    let b = stack[sp - 1u];
                    let a = stack[sp - 2u];
                    sp = sp - 1u;
                    stack[sp - 1u] = combine(a, b, w0, code[pc + 1u]);
                }
            }
            default: {}
        }
        pc = next;
    }
    if (sp == 0u) {
        return vec2<f32>(EMPTY_DIST, 0.0);
    }
    return stack[0];
}

fn map(p: vec3<f32>) -> vec2<f32> {
    return run(0u, p);
}

fn calc_normal(p: vec3<f32>, eps: f32) -> vec3<f32> {
    let k1 = vec3<f32>(1.0, -1.0, -1.0);
    let k2 = vec3<f32>(-1.0, -1.0, 1.0);
    let k3 = vec3<f32>(-1.0, 1.0, -1.0);
    let k4 = vec3<f32>(1.0, 1.0, 1.0);
    let n = k1 * map(p + k1 * eps).x + k2 * map(p + k2 * eps).x + k3 * map(p + k3 * eps).x + k4 * map(p + k4 * eps).x;
    return normalize(n);
}

fn sel_normal(start: u32, p: vec3<f32>, eps: f32) -> vec3<f32> {
    let k1 = vec3<f32>(1.0, -1.0, -1.0);
    let k2 = vec3<f32>(-1.0, -1.0, 1.0);
    let k3 = vec3<f32>(-1.0, 1.0, -1.0);
    let k4 = vec3<f32>(1.0, 1.0, 1.0);
    let n = k1 * run(start, p + k1 * eps).x + k2 * run(start, p + k2 * eps).x + k3 * run(start, p + k3 * eps).x + k4 * run(start, p + k4 * eps).x;
    return normalize(n);
}

fn box_hit(ro: vec3<f32>, rd: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> vec2<f32> {
    let inv = 1.0 / rd;
    let t0 = (lo - ro) * inv;
    let t1 = (hi - ro) * inv;
    let tmin = min(t0, t1);
    let tmax = max(t0, t1);
    let a = max(max(tmin.x, tmin.y), max(tmin.z, 0.0));
    let b = min(min(tmax.x, tmax.y), tmax.z);
    return vec2<f32>(a, b);
}

fn soft_shadow(ro: vec3<f32>, rd: vec3<f32>, tmax_in: f32) -> f32 {
    // only the part of the ray inside the scene bounds can be shadowed: rays
    // that miss them (most of the ground) cost nothing
    let span = box_hit(ro, rd, g.scene_lo.xyz - vec3<f32>(2.0), g.scene_hi.xyz + vec3<f32>(2.0));
    if (span.x >= span.y) {
        return 1.0;
    }
    let tmax = min(tmax_in, span.y);
    var res = 1.0;
    var t = max(0.8, span.x);
    for (var i = 0; i < 28; i = i + 1) {
        let h = map(ro + rd * t).x;
        res = min(res, 10.0 * h / t);
        t = t + clamp(h, 0.6, 12.0);
        if (res < 0.02 || t > tmax) {
            break;
        }
    }
    return clamp(res, 0.0, 1.0);
}

fn calc_ao(p: vec3<f32>, n: vec3<f32>) -> f32 {
    var occ = 0.0;
    var sca = 1.0;
    for (var i = 1; i <= 4; i = i + 1) {
        let h = 0.6 + 2.6 * f32(i);
        let d = map(p + n * h).x;
        occ = occ + (h - d) * sca;
        sca = sca * 0.72;
    }
    return clamp(1.0 - occ * 0.045, 0.0, 1.0);
}

fn background(rd: vec3<f32>, frag: vec2<f32>) -> vec3<f32> {
    let v = frag.y / g.viewport.y;
    let top = vec3<f32>(0.0102, 0.0109, 0.0124);
    let bottom = vec3<f32>(0.0074, 0.0079, 0.0090);
    return mix(top, bottom, v);
}

fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn shade(p: vec3<f32>, n: vec3<f32>, rd: vec3<f32>, mat: u32) -> vec3<f32> {
    let mi = min(mat, 15u);
    let base = g.mats[mi].rgb;
    let metal = g.mats[mi].w;
    let rough = clamp(g.mats2[mi].x, 0.05, 1.0);
    let key = g.light.xyz;
    let kint = g.light.w;
    let fill = normalize(vec3<f32>(0.8, 0.25, 0.35));
    let rim = normalize(vec3<f32>(0.3, 0.5, -0.85));
    var sh = 1.0;
    if (g.params.z > 0.5) {
        sh = soft_shadow(p + n * 0.4, key, 400.0);
    }
    var ao = 1.0;
    if (g.params.w > 0.5) {
        ao = calc_ao(p, n);
    }
    let v = -rd;
    let ndl = max(dot(n, key), 0.0);
    let ndv = max(dot(n, v), 0.0);
    let fres = pow(1.0 - ndv, 5.0);
    let diffuse = ndl * sh * vec3<f32>(1.0, 0.97, 0.92) * 1.15 * kint
        + max(dot(n, fill), 0.0) * vec3<f32>(0.55, 0.62, 0.75) * 0.35
        + (0.5 + 0.5 * n.y) * vec3<f32>(0.32, 0.34, 0.38) * 0.45 * ao
        + max(dot(n, rim), 0.0) * vec3<f32>(0.6, 0.65, 0.75) * 0.25;
    let shin = mix(160.0, 8.0, rough);
    let h = normalize(key + v);
    let spec = pow(max(dot(n, h), 0.0), shin) * (shin + 2.0) / 8.0 * ndl * sh * kint;
    let f0 = mix(vec3<f32>(0.04), base, metal);
    let fr = f0 + (vec3<f32>(1.0) - f0) * fres * (1.0 - rough * 0.7);
    let refl = reflect(rd, n);
    let env = mix(vec3<f32>(0.06, 0.06, 0.07), vec3<f32>(0.55, 0.58, 0.64), smoothstep(-0.2, 0.8, refl.y));
    var col = base * diffuse * (1.0 - metal) * ao;
    col = col + spec * fr * 0.9;
    col = col + env * fr * ao * mix(0.35, 1.0, metal) * (1.0 - rough * 0.6);
    return col;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let frag = in.pos.xy + g.extra.xy;
    let half_h = g.viewport.y * 0.5;
    let sx = (frag.x - g.viewport.z) / half_h;
    let sy = -(frag.y - g.viewport.w) / half_h;
    let tan_half = g.eye.w;
    let ro = g.eye.xyz;
    let rd = normalize(g.fwd.xyz + g.right.xyz * sx * tan_half + g.up.xyz * sy * tan_half);
    let px = tan_half / half_h; // ray spread per pixel per unit distance

    var col = background(rd, frag);
    let ground_y = g.scene_lo.w;
    var t_ground = 1e9;
    if (rd.y < -1e-4) {
        t_ground = (ground_y - ro.y) / rd.y;
    }

    // --- scene ---
    var t_hit = 1e9;
    var hit = false;
    var mat = 0u;
    if (g.params.y > 0.5) {
        let span = box_hit(ro, rd, g.scene_lo.xyz - vec3<f32>(2.0), g.scene_hi.xyz + vec3<f32>(2.0));
        if (span.x < span.y) {
            var t = span.x;
            let steps = u32(g.params.x);
            for (var i = 0u; i < steps; i = i + 1u) {
                let r = map(ro + rd * t);
                if (r.x < max(0.004, t * px * 0.6)) {
                    hit = true;
                    mat = u32(r.y + 0.5);
                    break;
                }
                t = t + r.x * 0.92;
                if (t > span.y) {
                    break;
                }
            }
            if (hit) {
                t_hit = t;
            }
        }
    }

    // --- ground with grid and contact shadow ---
    if (t_ground < t_hit && t_ground > 0.0) {
        let gp = ro + rd * t_ground;
        var gcol = vec3<f32>(0.0125, 0.0133, 0.0150);
        if (g.scene_hi.w > 0.5) {
            let foot = t_ground * px * 1.5;
            let c1 = abs(fract(gp.xz / 10.0 + 0.5) - 0.5) * 10.0;
            let l1 = 1.0 - smoothstep(0.0, foot + 0.05, min(c1.x, c1.y));
            let c2 = abs(fract(gp.xz / 50.0 + 0.5) - 0.5) * 50.0;
            let l2 = 1.0 - smoothstep(0.0, foot + 0.08, min(c2.x, c2.y));
            let fade = exp(-t_ground * 0.0016);
            gcol = gcol + vec3<f32>(0.010, 0.011, 0.013) * l1 * fade + vec3<f32>(0.022, 0.024, 0.028) * l2 * fade;
        }
        if (g.params.y > 0.5) {
            let d = map(gp + vec3<f32>(0.0, 0.5, 0.0)).x;
            let contact = clamp(d / 24.0, 0.0, 1.0);
            var shadow = 1.0;
            if (g.params.z > 0.5) {
                shadow = soft_shadow(gp + vec3<f32>(0.0, 0.3, 0.0), g.light.xyz, 300.0);
            }
            gcol = gcol * mix(0.25, 1.0, contact) * mix(0.45, 1.0, shadow);
        }
        let fog = clamp(t_ground / 2600.0, 0.0, 1.0);
        col = mix(gcol, col, fog);
    }

    if (hit) {
        let p = ro + rd * t_hit;
        let n = calc_normal(p, max(0.01, t_hit * px * 0.5));
        col = shade(p, n, rd, mat);
        let sel_start = g.right.w;
        if (sel_start >= 0.0 && g.up.w > 0.5) {
            let ds = run(u32(sel_start), p).x;
            let band = 0.25 + t_hit * px * 1.2;
            if (abs(ds) < band) {
                let lum = 0.35 + 0.65 * max(dot(n, g.light.xyz), 0.0);
                col = mix(col, g.sel_color.rgb * lum, 0.42);
            }
        }
    }

    // --- ghost of the selected cutter (carve, groove...) ---
    let sel_start2 = g.right.w;
    if (sel_start2 >= 0.0 && g.up.w > 1.5) {
        let s0 = u32(sel_start2);
        let span = box_hit(ro, rd, g.scene_lo.xyz - vec3<f32>(60.0), g.scene_hi.xyz + vec3<f32>(60.0));
        var t = max(span.x, 0.0);
        var ghit = false;
        for (var i = 0; i < 96; i = i + 1) {
            let d = run(s0, ro + rd * t).x;
            if (d < max(0.004, t * px * 0.6)) {
                ghit = true;
                break;
            }
            t = t + d * 0.95;
            if (t > min(span.y, 4000.0)) {
                break;
            }
        }
        if (ghit) {
            let gn = sel_normal(s0, ro + rd * t, max(0.01, t * px * 0.5));
            let edge = pow(1.0 - abs(dot(gn, -rd)), 2.0);
            var alpha = 0.18 + 0.55 * edge;
            if (t > t_hit + 0.5) {
                alpha = alpha * 0.45; // hidden part, seen through the matter
            }
            col = mix(col, g.ghost_color.rgb * (0.6 + 0.6 * edge), alpha);
        }
    }

    return vec4<f32>(aces(col * 1.9 * g.extra.z), 1.0);
}
