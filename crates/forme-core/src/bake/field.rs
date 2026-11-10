//! Double precision interpreter of the compiled program, with ownership.
//!
//! Same semantics as the GPU shader and `Program::eval`, plus, for every
//! point, the *term* that decides the distance: one copy of one primitive at
//! one iso level (levels other than 0 come from shells, grooves and tongues).
//! `margin` is a lower bound of how far the point is from a change of owner.

use crate::program::{Program, EMPTY_DIST, INSTR_WORDS, OP_AFFINE, OP_BOUND, OP_COMBINE, OP_EMPTY, OP_END, OP_LINEAR, OP_MIRROR, OP_PRIM, OP_RADIAL, OP_RESET};
use glam::{DAffine3, DMat3, DVec2, DVec3};

const SQRT1_2: f64 = std::f64::consts::FRAC_1_SQRT_2;

#[derive(Clone, Copy, Debug)]
pub enum ChainOp {
    /// p' = m p + c, distances scale by `scale`.
    Affine { m: DMat3, c: DVec3, scale: f64 },
    Radial { n: u32, axis: u32, second: bool },
    Linear { n: u32, step: DVec3, origin: DVec3, second: bool },
    Mirror { x: bool, y: bool, z: bool },
}

impl ChainOp {
    fn radix(&self) -> u32 {
        match *self {
            ChainOp::Affine { .. } => 1,
            ChainOp::Radial { n, .. } | ChainOp::Linear { n, .. } => n.max(1),
            ChainOp::Mirror { .. } => 8,
        }
    }
}

/// Primitive parameters, decoded from its instruction.
#[derive(Clone, Debug)]
pub struct PrimDef {
    pub kind: u32,
    pub size: DVec3,
    pub round: f64,
    pub chamfer: f64,
    pub taper: f64,
    pub shell: f64,
    pub sides: f64,
    /// Profile polyline (closed) and mode (true = revolve).
    pub poly: Vec<DVec2>,
    pub revolve: bool,
    /// Per-edge fillets (box 12, cylinder 2; negative = chamfer), empty if uniform.
    pub edges: Vec<f64>,
    pub material: u32,
    pub chain: Vec<ChainOp>,
    /// Product of the affine scales of the chain.
    pub sc: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Owner {
    pub prim: u32,
    pub copy: u32,
    /// World iso level of the primitive's raw distance.
    pub level: f64,
}

impl Owner {
    pub const NONE: Owner = Owner { prim: u32::MAX, copy: 0, level: 0.0 };
    pub fn key(&self) -> TermKey {
        TermKey { prim: self.prim, copy: self.copy, level: (self.level * 1e5).round() as i64 }
    }
    pub fn is_none(&self) -> bool {
        self.prim == u32::MAX
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TermKey {
    pub prim: u32,
    pub copy: u32,
    pub level: i64,
}

impl TermKey {
    pub const NONE: TermKey = TermKey { prim: u32::MAX, copy: 0, level: 0 };
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub d: f64,
    pub mat: u32,
    pub owner: Owner,
    pub margin: f64,
    /// How far smooth blends moved the surface away from the owner's own
    /// surface at this point (0 on plain primitive surfaces).
    pub blend: f64,
}

#[derive(Clone, Copy)]
struct Slot {
    d: f64,
    mat: u32,
    owner: Owner,
    margin: f64,
    blend: f64,
}

#[derive(Clone, Copy, Debug)]
enum Instr {
    Empty,
    Bound { mode: u32, pad: f64, skip: usize, c: DVec3, r: f64 },
    Reset,
    Op(ChainOp),
    Prim(u32),
    Combine { mode: u32, k: f64, chamfer: bool, depth: f64, width: f64 },
    End,
}

/// One copy of a primitive: where it sits in the world.
#[derive(Clone, Copy, Debug)]
pub struct Instance {
    pub prim: u32,
    pub copy: u32,
    pub to_world: DAffine3,
    pub to_local: DAffine3,
}

/// One operation of the modelling history (see `Field::steps`).
#[derive(Clone, Copy, Debug)]
pub struct Step {
    pub prim: u32,
    /// 0 add, 1 carve, 2 intersect, u32::MAX for the body.
    pub mode: u32,
    /// Blend (fillet) size in world units.
    pub k: f64,
    pub chamfer: bool,
    /// Index of the instruction that combines this step (see `Field::prefix`).
    pub end: usize,
}

#[derive(Clone)]
pub struct Field {
    instrs: Vec<Instr>,
    pub prims: Vec<PrimDef>,
    /// Largest amount a blend can move the surface away from its terms.
    pub blend_bound: f64,
}

fn rot_in_plane(axis: u32, phi: f64) -> DMat3 {
    // rotation by phi in the (u, v) plane used by the radial fold
    let (s, c) = phi.sin_cos();
    let rot = |p: DVec3| -> DVec3 {
        let (u, v) = match axis {
            0 => (p.y, p.z),
            1 => (p.z, p.x),
            _ => (p.x, p.y),
        };
        let (nu, nv) = (u * c - v * s, u * s + v * c);
        let mut q = p;
        match axis {
            0 => {
                q.y = nu;
                q.z = nv;
            }
            1 => {
                q.z = nu;
                q.x = nv;
            }
            _ => {
                q.x = nu;
                q.y = nv;
            }
        }
        q
    };
    DMat3::from_cols(rot(DVec3::X), rot(DVec3::Y), rot(DVec3::Z))
}

impl Field {
    pub fn new(prog: &Program) -> Field {
        let code = &prog.code;
        let aux_pt = |k: usize| -> DVec2 {
            let v = prog.aux[k / 2];
            if k % 2 == 0 {
                DVec2::new(v[0] as f64, v[1] as f64)
            } else {
                DVec2::new(v[2] as f64, v[3] as f64)
            }
        };
        let mut instrs = vec![];
        let mut prims = vec![];
        let mut chain: Vec<ChainOp> = vec![];
        let mut blend_bound = 0.0f64;
        let mut pc = 0;
        while pc + INSTR_WORDS <= code.len() {
            let w = &code[pc..pc + INSTR_WORDS];
            let f = |r: usize, c: usize| w[r][c] as f64;
            let op = w[0][0] as u32;
            let ins = match op {
                OP_END => Instr::End,
                OP_EMPTY => Instr::Empty,
                OP_BOUND => Instr::Bound {
                    mode: w[0][1] as u32,
                    pad: f(0, 2),
                    skip: w[0][3] as usize,
                    c: DVec3::new(f(1, 0), f(1, 1), f(1, 2)),
                    r: f(1, 3),
                },
                OP_RESET => {
                    chain.clear();
                    Instr::Reset
                }
                OP_AFFINE => {
                    let rows = [w[1], w[2], w[3]];
                    let m = DMat3::from_cols(
                        DVec3::new(rows[0][0] as f64, rows[1][0] as f64, rows[2][0] as f64),
                        DVec3::new(rows[0][1] as f64, rows[1][1] as f64, rows[2][1] as f64),
                        DVec3::new(rows[0][2] as f64, rows[1][2] as f64, rows[2][2] as f64),
                    );
                    let c = DVec3::new(rows[0][3] as f64, rows[1][3] as f64, rows[2][3] as f64);
                    let o = ChainOp::Affine { m, c, scale: f(0, 1) };
                    chain.push(o);
                    Instr::Op(o)
                }
                OP_RADIAL => {
                    let o = ChainOp::Radial { n: (w[0][1].max(1.0)) as u32, axis: w[0][2] as u32, second: w[0][3] > 0.5 };
                    chain.push(o);
                    Instr::Op(o)
                }
                OP_LINEAR => {
                    let o = ChainOp::Linear {
                        n: (w[0][1].max(1.0)) as u32,
                        step: DVec3::new(f(1, 0), f(1, 1), f(1, 2)),
                        origin: DVec3::new(f(2, 0), f(2, 1), f(2, 2)),
                        second: w[0][2] > 0.5,
                    };
                    chain.push(o);
                    Instr::Op(o)
                }
                OP_MIRROR => {
                    let o = ChainOp::Mirror { x: w[0][1] > 0.5, y: w[0][2] > 0.5, z: w[0][3] > 0.5 };
                    chain.push(o);
                    Instr::Op(o)
                }
                OP_PRIM if w[0][3] > 0.5 && !prims.is_empty() => {
                    // another fold variant of the previous primitive: same term
                    Instr::Prim(prims.len() as u32 - 1)
                }
                OP_PRIM => {
                    let kind = w[0][1] as u32;
                    let off = w[3][0] as usize;
                    let count = w[3][1] as usize;
                    let poly = if kind == 8 { (0..count).map(|i| aux_pt(off + i)).collect() } else { vec![] };
                    let sc = chain
                        .iter()
                        .map(|o| if let ChainOp::Affine { scale, .. } = o { *scale } else { 1.0 })
                        .product();
                    prims.push(PrimDef {
                        kind,
                        size: DVec3::new(f(1, 0), f(1, 1), f(1, 2)),
                        round: f(1, 3).max(0.0),
                        chamfer: f(2, 0).max(0.0),
                        taper: f(2, 1),
                        shell: f(2, 2),
                        sides: f(2, 3),
                        poly,
                        revolve: w[3][2] > 0.5,
                        edges: if w[3][3] > 0.5 {
                            let off = w[3][0] as usize / 2;
                            (0..w[3][1] as usize).map(|i| prog.aux.get(off + i / 4).map(|v| v[i % 4] as f64).unwrap_or(0.0)).collect()
                        } else {
                            vec![]
                        },
                        material: w[0][2] as u32,
                        chain: chain.clone(),
                        sc,
                    });
                    Instr::Prim(prims.len() as u32 - 1)
                }
                OP_COMBINE => {
                    let k = f(0, 2).max(0.0);
                    let chamfer = w[0][3] > 0.5;
                    blend_bound = blend_bound.max(if chamfer { k * 0.3 } else { k * 0.25 });
                    Instr::Combine { mode: w[0][1] as u32, k, chamfer, depth: f(1, 0), width: f(1, 1) }
                }
                _ => {
                    pc += INSTR_WORDS;
                    continue;
                }
            };
            instrs.push(ins);
            pc += INSTR_WORDS;
        }
        Field { instrs, prims, blend_bound }
    }

    pub fn is_empty(&self) -> bool {
        self.prims.is_empty()
    }

    pub fn eval(&self, pw: DVec3) -> Sample {
        let none = Slot { d: EMPTY_DIST as f64, mat: 0, owner: Owner::NONE, margin: f64::INFINITY, blend: 0.0 };
        let mut stack = [none; 16];
        let mut sp = 0usize;
        let mut p = pw;
        let mut sc = 1.0f64;
        let mut code = 0u32;
        let mut fold = f64::INFINITY;
        let mut i = 0usize;
        let n = self.instrs.len();
        while i < n {
            match self.instrs[i] {
                Instr::End => break,
                Instr::Empty => {
                    if sp < 16 {
                        stack[sp] = none;
                        sp += 1;
                    }
                }
                Instr::Bound { mode, pad, skip, c, r } => {
                    let bd = (pw - c).length() - r;
                    let a = if sp > 0 { stack[sp - 1].d } else { EMPTY_DIST as f64 };
                    if bd > a.abs() + pad + 0.5 {
                        let d = if mode == 2 { bd } else { EMPTY_DIST as f64 };
                        if sp < 16 {
                            stack[sp] = Slot { d, ..none };
                            sp += 1;
                        }
                        i += skip + 1;
                        continue;
                    }
                }
                Instr::Reset => {
                    p = pw;
                    sc = 1.0;
                    code = 0;
                    fold = f64::INFINITY;
                }
                Instr::Op(op) => match op {
                    ChainOp::Affine { m, c, scale } => {
                        p = m * p + c;
                        sc *= scale;
                    }
                    ChainOp::Radial { n, axis, second } => {
                        let nf = n as f64;
                        let (u, v) = match axis {
                            0 => (p.y, p.z),
                            1 => (p.z, p.x),
                            _ => (p.x, p.y),
                        };
                        let sector = std::f64::consts::TAU / nf;
                        let a = v.atan2(u);
                        let a2 = a + sector * 0.5;
                        let mut k = (a2 / sector).floor();
                        let mut a3 = a2 - sector * k - sector * 0.5;
                        if second && n > 1 {
                            let s = if a3 >= 0.0 { 1.0 } else { -1.0 };
                            a3 -= s * sector;
                            k += s;
                        }
                        let l = (u * u + v * v).sqrt();
                        let (nu, nv) = (l * a3.cos(), l * a3.sin());
                        match axis {
                            0 => {
                                p.y = nu;
                                p.z = nv;
                            }
                            1 => {
                                p.z = nu;
                                p.x = nv;
                            }
                            _ => {
                                p.x = nu;
                                p.y = nv;
                            }
                        }
                        if n > 1 {
                            let to_edge = sector * 0.5 - a3.abs();
                            fold = fold.min(if to_edge < std::f64::consts::FRAC_PI_2 { l * to_edge.sin() } else { l } * sc);
                        }
                        code = code * n + (k as i64).rem_euclid(n as i64) as u32;
                    }
                    ChainOp::Linear { n, step, origin, second } => {
                        let ss = step.dot(step);
                        let mut t = 0.0;
                        if ss > 1e-12 {
                            let u = (p - origin).dot(step) / ss;
                            t = u.round().clamp(0.0, n as f64 - 1.0);
                            if second {
                                t = (t + if u > t { 1.0 } else { -1.0 }).clamp(0.0, n as f64 - 1.0);
                            }
                            let len = ss.sqrt();
                            if t > 0.0 {
                                fold = fold.min((u - t + 0.5).max(0.0) * len * sc);
                            }
                            if t < n as f64 - 1.0 {
                                fold = fold.min((t + 0.5 - u).max(0.0) * len * sc);
                            }
                            p -= step * t;
                        }
                        code = code * n + t as u32;
                    }
                    ChainOp::Mirror { x, y, z } => {
                        let mut bits = 0;
                        if x {
                            fold = fold.min(p.x.abs() * sc);
                            if p.x < 0.0 {
                                bits |= 1;
                            }
                            p.x = p.x.abs();
                        }
                        if y {
                            fold = fold.min(p.y.abs() * sc);
                            if p.y < 0.0 {
                                bits |= 2;
                            }
                            p.y = p.y.abs();
                        }
                        if z {
                            fold = fold.min(p.z.abs() * sc);
                            if p.z < 0.0 {
                                bits |= 4;
                            }
                            p.z = p.z.abs();
                        }
                        code = code * 8 + bits;
                    }
                },
                Instr::Prim(k) => {
                    let pr = &self.prims[k as usize];
                    let raw = prim_raw(pr, p);
                    let (d, level, m) = if pr.shell > 0.0 {
                        // hollow, a wall `shell` thick inside the outer surface
                        let h = pr.shell * 0.5;
                        let c = raw + h;
                        let s = if c >= 0.0 { 1.0 } else { -1.0 };
                        ((c.abs() - h) * lip(pr, p), s * h * sc - h * sc, c.abs() * sc)
                    } else {
                        (raw * lip(pr, p), 0.0, f64::INFINITY)
                    };
                    if sp < 16 {
                        stack[sp] = Slot { d: d * sc, mat: pr.material, owner: Owner { prim: k, copy: code, level }, margin: fold.min(m), blend: 0.0 };
                        sp += 1;
                    }
                }
                Instr::Combine { mode, k, chamfer, depth, width } => {
                    if sp >= 2 {
                        let b = stack[sp - 1];
                        let a = stack[sp - 2];
                        sp -= 1;
                        stack[sp - 1] = combine(a, b, mode, k, chamfer, depth, width);
                    }
                }
            }
            i += 1;
        }
        if sp == 0 {
            return Sample { d: EMPTY_DIST as f64, mat: 0, owner: Owner::NONE, margin: f64::INFINITY, blend: 0.0 };
        }
        let s = stack[0];
        Sample { d: s.d, mat: s.mat, owner: s.owner, margin: s.margin, blend: s.blend }
    }

    #[inline]
    pub fn dist(&self, p: DVec3) -> f64 {
        self.eval(p).d
    }

    pub fn grad(&self, p: DVec3) -> DVec3 {
        let h = 1e-6;
        let k = [DVec3::new(1.0, -1.0, -1.0), DVec3::new(-1.0, -1.0, 1.0), DVec3::new(-1.0, 1.0, -1.0), DVec3::new(1.0, 1.0, 1.0)];
        let mut g = DVec3::ZERO;
        for v in k {
            g += v * self.dist(p + v * h);
        }
        g / (4.0 * h)
    }

    /// Newton projection on the zero set, from a nearby point.
    pub fn project(&self, p0: DVec3) -> Option<DVec3> {
        let mut p = p0;
        let mut f = self.dist(p);
        for _ in 0..40 {
            if f.abs() < 1e-10 {
                return Some(p);
            }
            let g = self.grad(p);
            let gg = g.length_squared();
            if gg < 1e-10 {
                return None;
            }
            let mut step = g * (f / gg);
            let sl = step.length();
            let cap = f.abs() * 1.5 + 1e-6;
            if sl > cap {
                step *= cap / sl;
            }
            let mut q = p - step;
            let mut fq = self.dist(q);
            // damping when Newton overshoots across a crease
            let mut tries = 0;
            while fq.abs() > f.abs() && tries < 6 {
                step *= 0.5;
                q = p - step;
                fq = self.dist(q);
                tries += 1;
            }
            p = q;
            f = fq;
        }
        if f.abs() < 1e-7 {
            Some(p)
        } else {
            None
        }
    }

    /// Every copy of a primitive made by the folds of its chain.
    pub fn instances(&self, prim: u32) -> Vec<Instance> {
        let pr = &self.prims[prim as usize];
        let mut out = vec![];
        fn rec(ops: &[ChainOp], i: usize, code: u32, fwd: DAffine3, prim: u32, out: &mut Vec<Instance>) {
            if i == ops.len() {
                out.push(Instance { prim, copy: code, to_world: fwd, to_local: fwd.inverse() });
                return;
            }
            let op = ops[i];
            let radix = op.radix();
            match op {
                ChainOp::Affine { m, c, .. } => {
                    let mi = m.inverse();
                    let inv = DAffine3::from_mat3_translation(mi, -(mi * c));
                    rec(ops, i + 1, code, fwd * inv, prim, out);
                }
                ChainOp::Radial { n, axis, .. } => {
                    let sector = std::f64::consts::TAU / n as f64;
                    for k in 0..n {
                        let inv = DAffine3::from_mat3(rot_in_plane(axis, k as f64 * sector));
                        rec(ops, i + 1, code * radix + k, fwd * inv, prim, out);
                    }
                }
                ChainOp::Linear { n, step, .. } => {
                    for t in 0..n {
                        let inv = DAffine3::from_translation(step * t as f64);
                        rec(ops, i + 1, code * radix + t, fwd * inv, prim, out);
                    }
                }
                ChainOp::Mirror { x, y, z } => {
                    for bits in 0..8u32 {
                        if (bits & 1 != 0 && !x) || (bits & 2 != 0 && !y) || (bits & 4 != 0 && !z) {
                            continue;
                        }
                        let s = DVec3::new(
                            if bits & 1 != 0 { -1.0 } else { 1.0 },
                            if bits & 2 != 0 { -1.0 } else { 1.0 },
                            if bits & 4 != 0 { -1.0 } else { 1.0 },
                        );
                        let inv = DAffine3::from_mat3(DMat3::from_diagonal(s));
                        rec(ops, i + 1, code * radix + bits, fwd * inv, prim, out);
                    }
                }
            }
        }
        rec(&pr.chain, 0, 0, DAffine3::IDENTITY, prim, &mut out);
        out
    }

    /// Static analysis: every (primitive, world level) that can own the surface.
    /// The modelling history as a flat list of operations, when the program
    /// is a plain chain (body, then one primitive combined at a time):
    /// the first step is the body (mode u32::MAX).
    pub fn steps(&self) -> Option<Vec<Step>> {
        let mut stack: Vec<Option<u32>> = vec![];
        let mut out: Vec<Step> = vec![];
        for (idx, ins) in self.instrs.iter().enumerate() {
            match *ins {
                Instr::Empty => stack.push(None),
                Instr::Prim(k) => stack.push(Some(k)),
                Instr::Combine { mode, k, chamfer, .. } => {
                    let b = stack.pop()?;
                    let a = stack.pop()?;
                    match (a, b) {
                        // fold variants of one term
                        (Some(x), Some(y)) if x == y => {
                            stack.push(Some(x));
                            if let Some(last) = out.last_mut() {
                                if last.prim == x {
                                    last.end = idx;
                                }
                            }
                        }
                        (None, Some(y)) | (Some(y), None) => {
                            if out.is_empty() {
                                out.push(Step { prim: y, mode: u32::MAX, k: 0.0, chamfer: false, end: idx });
                            }
                            stack.push(Some(u32::MAX - 1));
                        }
                        (Some(x), Some(y)) => {
                            if out.is_empty() {
                                if x >= u32::MAX - 1 {
                                    return None;
                                }
                                out.push(Step { prim: x, mode: u32::MAX, k: 0.0, chamfer: false, end: idx });
                            }
                            if y >= u32::MAX - 1 || mode > 2 {
                                return None;
                            }
                            out.push(Step { prim: y, mode, k, chamfer, end: idx });
                            stack.push(Some(u32::MAX - 1));
                        }
                        _ => return None,
                    }
                }
                Instr::End => break,
                _ => {}
            }
        }
        if out.is_empty() {
            if let Some(Some(k)) = stack.first() {
                if *k < u32::MAX - 1 {
                    out.push(Step { prim: *k, mode: u32::MAX, k: 0.0, chamfer: false, end: self.instrs.len().saturating_sub(1) });
                }
            }
        }
        Some(out)
    }

    /// The model after its first `n` steps only.
    pub fn prefix(&self, steps: &[Step], n: usize) -> Field {
        let mut f = self.clone();
        if n >= 1 && n <= steps.len() {
            let end = steps[n - 1].end.min(f.instrs.len().saturating_sub(1));
            f.instrs.truncate(end + 1);
            f.instrs.push(Instr::End);
        }
        f
    }
}

fn combine(a: Slot, b: Slot, mode: u32, k: f64, chamfer: bool, depth: f64, width: f64) -> Slot {
    let (da, db) = (a.d, b.d);
    match mode {
        0 => {
            let d = if k <= 0.0 {
                da.min(db)
            } else if chamfer {
                da.min(db).min((da + db - k) * SQRT1_2)
            } else {
                let h = (k - (da - db).abs()).max(0.0) / k;
                da.min(db) - h * h * k * 0.25
            };
            let w = if db < da { b } else { a };
            Slot { d, mat: w.mat, owner: w.owner, margin: w.margin.min((da - db).abs() * 0.5), blend: w.blend + (da.min(db) - d).abs() }
        }
        1 | 2 => {
            let nb = if mode == 1 { -db } else { db };
            let d = if k <= 0.0 {
                da.max(nb)
            } else if chamfer {
                da.max(nb).max((da + nb + k) * SQRT1_2)
            } else {
                let h = (k - (da - nb).abs()).max(0.0) / k;
                da.max(nb) + h * h * k * 0.25
            };
            let w = if nb > da { b } else { a };
            Slot { d, mat: w.mat, owner: w.owner, margin: w.margin.min((da - nb).abs() * 0.5), blend: w.blend + (da.max(nb) - d).abs() }
        }
        3 => {
            let f1 = da + depth;
            let f2 = width - db.abs();
            let inner = f1.min(f2);
            let d = da.max(inner);
            if da >= inner {
                Slot { d, mat: a.mat, owner: a.owner, margin: a.margin.min((da - inner) * 0.5), blend: a.blend }
            } else if f1 <= f2 {
                let o = Owner { level: a.owner.level - depth, ..a.owner };
                Slot { d, mat: a.mat, owner: o, margin: a.margin.min((inner - da) * 0.5).min((f2 - f1) * 0.5), blend: a.blend }
            } else {
                let s = if db >= 0.0 { 1.0 } else { -1.0 };
                let o = Owner { level: b.owner.level + s * width, ..b.owner };
                Slot { d, mat: a.mat, owner: o, margin: b.margin.min((inner - da) * 0.5).min((f1 - f2) * 0.5).min(db.abs()), blend: b.blend }
            }
        }
        4 => {
            let top = da - depth;
            let wall = db.abs() - width;
            let t = top.max(wall);
            let d = da.min(t);
            let mat = if t < da { b.mat } else { a.mat };
            if da <= t {
                Slot { d, mat, owner: a.owner, margin: a.margin.min((t - da) * 0.5), blend: a.blend }
            } else if top >= wall {
                let o = Owner { level: a.owner.level + depth, ..a.owner };
                Slot { d, mat, owner: o, margin: a.margin.min((da - t) * 0.5).min((top - wall) * 0.5), blend: a.blend }
            } else {
                let s = if db >= 0.0 { 1.0 } else { -1.0 };
                let o = Owner { level: b.owner.level + s * width, ..b.owner };
                Slot { d, mat, owner: o, margin: b.margin.min((da - t) * 0.5).min((wall - top) * 0.5).min(db.abs()), blend: b.blend }
            }
        }
        _ => a,
    }
}

// ---------------------------------------------------------------------------
// Primitives in double precision (mirror of program::eval_prim)
// ---------------------------------------------------------------------------

#[inline]
fn vmax(v: DVec3) -> f64 {
    v.x.max(v.y).max(v.z)
}

#[inline]
pub fn extrude(d2: f64, h: f64) -> f64 {
    let w = DVec2::new(d2, h);
    w.x.max(w.y).min(0.0) + w.max(DVec2::ZERO).length()
}

pub fn sd_polygon(pts: &[DVec2], p: DVec2) -> f64 {
    let count = pts.len();
    let mut d = (p - pts[0]).length_squared();
    let mut s = 1.0f64;
    let mut j = count - 1;
    for i in 0..count {
        let vi = pts[i];
        let vj = pts[j];
        let e = vj - vi;
        let w = p - vi;
        let b = w - e * (w.dot(e) / e.dot(e).max(1e-18)).clamp(0.0, 1.0);
        d = d.min(b.length_squared());
        let c1 = p.y >= vi.y;
        let c2 = p.y < vj.y;
        let c3 = e.x * w.y > e.y * w.x;
        if (c1 && c2 && c3) || (!c1 && !c2 && !c3) {
            s = -s;
        }
        j = i;
    }
    s * d.sqrt()
}

pub fn sd_regular_polygon(p: DVec2, r: f64, n: f64) -> f64 {
    let an = std::f64::consts::PI / n;
    let a = p.x.atan2(p.y);
    let m = 2.0 * an;
    let bn = (a - m * (a / m).floor()) - an;
    let l = p.length();
    let mut q = DVec2::new(l * bn.cos(), l * bn.sin().abs());
    q -= r * DVec2::new(an.cos(), an.sin());
    q.y += (-q.y).clamp(0.0, r * an.sin());
    q.length() * q.x.signum()
}

/// Taper warp factor at local point p (1 when no taper).
fn taper_factor(pr: &PrimDef, p: DVec3) -> f64 {
    if pr.taper.abs() > 1e-4 && pr.kind != 8 {
        let hy = pr.size.y.max(1e-3);
        let t = (p.y / (2.0 * hy) + 0.5).clamp(0.0, 1.0);
        (1.0 - pr.taper * t).max(0.05)
    } else {
        1.0
    }
}

fn lip(pr: &PrimDef, p: DVec3) -> f64 {
    if pr.taper.abs() > 1e-4 && pr.kind != 8 {
        taper_factor(pr, p).min(1.0) * 0.85
    } else {
        1.0
    }
}

/// Raw signed distance of a primitive in its local frame, before shell and lip.
pub fn prim_raw(pr: &PrimDef, p_in: DVec3) -> f64 {
    let size = pr.size;
    let round = pr.round;
    let chamfer = pr.chamfer;
    let mut p = p_in;
    let f = taper_factor(pr, p);
    if f != 1.0 {
        p.x /= f;
        p.z /= f;
    }
    match pr.kind {
        0 if pr.edges.len() == 12 => box_edges(p, size, &pr.edges),
        1 if pr.edges.len() == 2 => cyl_edges(p, size, if p.y >= 0.0 { pr.edges[0] } else { pr.edges[1] }),
        0 => {
            let r = round.min(size.min_element());
            let q = p.abs() - (size - DVec3::splat(r));
            let mut d = q.max(DVec3::ZERO).length() + vmax(q).min(0.0) - r;
            if chamfer > 0.0 {
                let q2 = p.abs() - size;
                d = d
                    .max((q2.x + q2.y + chamfer) * SQRT1_2)
                    .max((q2.x + q2.z + chamfer) * SQRT1_2)
                    .max((q2.y + q2.z + chamfer) * SQRT1_2);
            }
            d
        }
        1 => {
            let r = round.min(size.x).min(size.y);
            let dx = DVec2::new(p.x, p.z).length() - size.x;
            let dy = p.y.abs() - size.y;
            let mut d = extrude(dx + r, dy + r) - r;
            if chamfer > 0.0 {
                d = d.max((dx + dy + chamfer) * SQRT1_2);
            }
            d
        }
        2 => {
            let r = size.max(DVec3::splat(1e-3));
            let k0 = (p / r).length();
            let k1 = (p / (r * r)).length();
            if k1 < 1e-12 {
                -r.min_element()
            } else {
                k0 * (k0 - 1.0) / k1
            }
        }
        3 => {
            let mut q = p;
            q.y -= q.y.clamp(-size.y, size.y);
            q.length() - size.x
        }
        4 => {
            let q = DVec2::new(DVec2::new(p.x, p.z).length() - size.x, p.y);
            q.length() - size.z
        }
        5 => {
            let r = round.min(size.y);
            let h = size.y - r;
            let r1 = (size.x - r).max(0.0);
            let r2 = (size.z - r).max(0.0);
            let q = DVec2::new(DVec2::new(p.x, p.z).length(), p.y);
            let k1 = DVec2::new(r2, h);
            let k2 = DVec2::new(r2 - r1, 2.0 * h);
            let ca = DVec2::new(q.x - q.x.min(if q.y < 0.0 { r1 } else { r2 }), q.y.abs() - h);
            let cb = q - k1 + k2 * ((k1 - q).dot(k2) / k2.dot(k2).max(1e-18)).clamp(0.0, 1.0);
            let s = if cb.x < 0.0 && ca.y < 0.0 { -1.0 } else { 1.0 };
            s * ca.dot(ca).min(cb.dot(cb)).sqrt() - r
        }
        6 => {
            let r = round.min(size.y);
            let d2 = sd_regular_polygon(DVec2::new(p.x, p.z), size.x - r, pr.sides.max(3.0));
            extrude(d2, p.y.abs() - size.y + r) - r
        }
        7 => {
            let r = round;
            let mid = (size.x + size.z) * 0.5;
            let half = ((size.x - size.z) * 0.5).max(1e-3);
            let d2 = (DVec2::new(p.x, p.z).length() - mid).abs() - half;
            extrude(d2 + r, p.y.abs() - size.y + r) - r
        }
        8 => {
            if pr.poly.len() < 3 {
                EMPTY_DIST as f64
            } else if pr.revolve {
                let q = DVec2::new(DVec2::new(p.x, p.z).length(), p.y);
                sd_polygon(&pr.poly, q) - round
            } else {
                let d2 = sd_polygon(&pr.poly, DVec2::new(p.x, p.y));
                extrude(d2 + round, p.z.abs() - size.z + round) - round
            }
        }
        _ => EMPTY_DIST as f64,
    }
}

fn wedge(qb: f64, qc: f64, r: f64, cap: f64) -> f64 {
    if r < 0.0 {
        qb.max(qc).max((qb + qc - r) * SQRT1_2)
    } else {
        let r = r.min(cap);
        let v = DVec2::new(qb + r, qc + r);
        v.max(DVec2::ZERO).length() + v.x.max(v.y).min(0.0) - r
    }
}

/// Edge `i` of a box (12): parallel to axis i / 4, on the side given by
/// i % 4 of the two other axes: (+,+) (-,+) (-,-) (+,-).
pub fn box_edge_signs(i: usize) -> (usize, f64, f64) {
    let (sb, sc) = [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)][i % 4];
    (i / 4, sb, sc)
}

/// Box with one fillet per edge (see program::box_edges).
pub fn box_edges(p: DVec3, size: DVec3, er: &[f64]) -> f64 {
    let q = p.abs() - size;
    let k = |sb: f64, sc: f64| -> usize {
        match (sb >= 0.0, sc >= 0.0) {
            (true, true) => 0,
            (false, true) => 1,
            (false, false) => 2,
            (true, false) => 3,
        }
    };
    let r0 = er[k(p.y, p.z)];
    let r1 = er[4 + k(p.z, p.x)];
    let r2 = er[8 + k(p.x, p.y)];
    let mut d = wedge(q.y, q.z, r0, size.y.min(size.z)).max(wedge(q.z, q.x, r1, size.z.min(size.x))).max(wedge(q.x, q.y, r2, size.x.min(size.y)));
    let rm = r0.min(r1).min(r2).min(size.min_element());
    if rm > 0.0 {
        let qs = q + DVec3::splat(rm);
        d = d.max(qs.max(DVec3::ZERO).length() + vmax(qs).min(0.0) - rm);
    }
    d
}

pub fn cyl_edges(p: DVec3, size: DVec3, r: f64) -> f64 {
    let dx = DVec2::new(p.x, p.z).length() - size.x;
    let dy = p.y.abs() - size.y;
    if r < 0.0 {
        extrude(dx, dy).max((dx + dy - r) * SQRT1_2)
    } else {
        let r = r.min(size.x).min(size.y);
        extrude(dx + r, dy + r) - r
    }
}

/// Gradient of the raw distance (local frame).
pub fn prim_grad(pr: &PrimDef, p: DVec3) -> DVec3 {
    let h = 1e-6;
    let k = [DVec3::new(1.0, -1.0, -1.0), DVec3::new(-1.0, -1.0, 1.0), DVec3::new(-1.0, 1.0, -1.0), DVec3::new(1.0, 1.0, 1.0)];
    let mut g = DVec3::ZERO;
    for v in k {
        g += v * prim_raw(pr, p + v * h);
    }
    g / (4.0 * h)
}

/// Newton projection on the iso surface `raw = level` (local frame).
pub fn prim_project(pr: &PrimDef, p0: DVec3, level: f64) -> DVec3 {
    let mut p = p0;
    for _ in 0..30 {
        let f = prim_raw(pr, p) - level;
        if f.abs() < 1e-11 {
            break;
        }
        let g = prim_grad(pr, p);
        let gg = g.length_squared();
        if gg < 1e-12 {
            break;
        }
        let mut step = g * (f / gg);
        let sl = step.length();
        let cap = f.abs() * 1.5 + 1e-6;
        if sl > cap {
            step *= cap / sl;
        }
        let q = p - step;
        let fq = prim_raw(pr, q) - level;
        if fq.abs() > f.abs() {
            p -= step * 0.5;
        } else {
            p = q;
        }
    }
    p
}
