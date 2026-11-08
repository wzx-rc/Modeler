//! Compiles the shape tree into a flat bytecode that both the GPU shader and
//! the CPU interpreter run. One instruction = 4 × vec4<f32> (16 floats).
//!
//! The machine has a point register `p` (with a distance scale `sc`) and a small
//! stack of (distance, material) pairs. Each primitive resets `p` to the world
//! point, replays its chain of domain operations (repeat, mirror, inverse
//! transform) from the root, evaluates its SDF and pushes the result; COMBINE
//! pops two results and pushes their combination.

use crate::scene::*;
use glam::{Mat3, Vec2, Vec3, Vec4};

pub const OP_END: u32 = 0;
pub const OP_EMPTY: u32 = 1;
pub const OP_RESET: u32 = 2;
pub const OP_AFFINE: u32 = 3;
pub const OP_RADIAL: u32 = 4;
pub const OP_LINEAR: u32 = 5;
pub const OP_MIRROR: u32 = 6;
pub const OP_PRIM: u32 = 7;
pub const OP_COMBINE: u32 = 8;
pub const OP_BOUND: u32 = 9;

pub const INSTR_WORDS: usize = 4;
pub const STACK_SIZE: usize = 16;
pub const EMPTY_DIST: f32 = 1.0e5;

#[derive(Clone, Copy, Debug)]
enum PointOp {
    Affine { rows: [Vec4; 3], scale: f32 },
    /// `second`: evaluate the neighbouring copy instead of the nearest one
    /// (copies that overlap their sector are unioned with their neighbour).
    Radial { count: u32, axis: u32, second: bool },
    Linear { count: u32, step: Vec3, origin: Vec3, second: bool },
    Mirror { x: bool, y: bool, z: bool },
}

impl PointOp {
    fn is_fold(&self) -> bool {
        matches!(self, PointOp::Radial { count, .. } | PointOp::Linear { count, .. } if *count > 1)
    }
    fn with_second(self, s: bool) -> Self {
        match self {
            PointOp::Radial { count, axis, .. } => PointOp::Radial { count, axis, second: s },
            PointOp::Linear { count, step, origin, .. } => PointOp::Linear { count, step, origin, second: s },
            o => o,
        }
    }
}

/// Compiled scene: `code` = instructions, `aux` = profile points (2 per vec4).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Program {
    pub code: Vec<[f32; 4]>,
    pub aux: Vec<[f32; 4]>,
    pub prim_count: u32,
}

fn node_ops(scene: &Scene, n: &Node) -> Vec<PointOp> {
    let mut ops = vec![];
    match n.repeat {
        Repeat::Radial { count, axis } if count > 1 => ops.push(PointOp::Radial { count, axis: axis.index(), second: false }),
        Repeat::Linear { count, step } if count > 1 => ops.push(PointOp::Linear { count, step, origin: scene.content_center_parent(n.id), second: false }),
        _ => {}
    }
    if n.mirror.any() {
        ops.push(PointOp::Mirror { x: n.mirror.x, y: n.mirror.y, z: n.mirror.z });
    }
    if !n.transform.is_identity() {
        let t = &n.transform;
        let s = t.scale.max(1e-4);
        let r = Mat3::from_quat(t.rot.inverse()) * (1.0 / s);
        let c = -(r * t.pos);
        // rows of r
        let rows = [
            Vec4::new(r.x_axis.x, r.y_axis.x, r.z_axis.x, c.x),
            Vec4::new(r.x_axis.y, r.y_axis.y, r.z_axis.y, c.y),
            Vec4::new(r.x_axis.z, r.y_axis.z, r.z_axis.z, c.z),
        ];
        ops.push(PointOp::Affine { rows, scale: s });
    }
    ops
}

struct Builder<'a> {
    scene: &'a Scene,
    prog: Program,
}

impl<'a> Builder<'a> {
    fn instr(&mut self, w: [[f32; 4]; 4]) {
        self.prog.code.extend_from_slice(&w);
    }

    fn op(&mut self, op: u32, a: f32, b: f32, c: f32) {
        self.instr([[op as f32, a, b, c], [0.0; 4], [0.0; 4], [0.0; 4]]);
    }

    fn point_op(&mut self, o: &PointOp) {
        match *o {
            PointOp::Affine { rows, scale } => self.instr([
                [OP_AFFINE as f32, scale, 0.0, 0.0],
                rows[0].to_array(),
                rows[1].to_array(),
                rows[2].to_array(),
            ]),
            PointOp::Radial { count, axis, second } => self.op(OP_RADIAL, count as f32, axis as f32, second as u32 as f32),
            PointOp::Linear { count, step, origin, second } => self.instr([
                [OP_LINEAR as f32, count as f32, second as u32 as f32, 0.0],
                [step.x, step.y, step.z, 0.0],
                [origin.x, origin.y, origin.z, 0.0],
                [0.0; 4],
            ]),
            PointOp::Mirror { x, y, z } => self.op(OP_MIRROR, x as u32 as f32, y as u32 as f32, z as u32 as f32),
        }
    }

    fn prim(&mut self, id: NodeId, chain: &[PointOp], shape: &Shape, material: u32, combine: &Combine) {
        // copies made by folds may overlap their cell: each fold is evaluated
        // for the nearest and the neighbouring copy, and the variants unioned
        let folds: Vec<usize> = chain.iter().enumerate().filter(|(_, o)| o.is_fold()).map(|(i, _)| i).collect();
        let folds: Vec<usize> = folds.iter().rev().take(3).rev().copied().collect();
        let variants = 1usize << folds.len();
        if let Some((lo, hi)) = self.scene.shape_world_aabb(id) {
            let c = (lo + hi) * 0.5;
            let r = (hi - lo).length() * 0.5;
            let skip = variants * (2 + chain.len()) + (variants - 1);
            let pad = combine.blend.max(0.0) + combine.depth.max(0.0) + combine.width.max(0.0);
            self.instr([
                [OP_BOUND as f32, combine.mode.code() as f32, pad, skip as f32],
                [c.x, c.y, c.z, r],
                [0.0; 4],
                [0.0; 4],
            ]);
        }
        let (aux_off, aux_count, mode) = match (&shape.kind, &shape.profile) {
            (PrimKind::Profile, Some(p)) => {
                let pts = p.polyline();
                let off = self.prog.aux.len() * 2;
                for pair in pts.chunks(2) {
                    let a = pair[0];
                    let b = if pair.len() > 1 { pair[1] } else { Vec2::ZERO };
                    self.prog.aux.push([a.x, a.y, b.x, b.y]);
                }
                let mode = if p.mode == ProfileMode::Revolve { 1.0 } else { 0.0 };
                (off as f32, pts.len() as f32, mode)
            }
            _ => (0.0, 0.0, 0.0),
        };
        let s = shape.size;
        for v in 0..variants {
            self.op(OP_RESET, 0.0, 0.0, 0.0);
            for (i, o) in chain.iter().enumerate() {
                let second = folds.iter().position(|&f| f == i).map(|b| v & (1 << b) != 0).unwrap_or(false);
                self.point_op(&o.with_second(second));
            }
            self.instr([
                [OP_PRIM as f32, shape.kind.code() as f32, material as f32, v as f32],
                [s.x, s.y, s.z, shape.round],
                [shape.chamfer, shape.taper, shape.shell, shape.sides.max(3) as f32],
                [aux_off, aux_count, mode, 0.0],
            ]);
            if v > 0 {
                self.combine(&Combine { mode: CombineMode::Add, blend: 0.0, ..Default::default() });
            }
        }
        self.prog.prim_count += 1;
    }

    fn combine(&mut self, c: &Combine) {
        let kind = match c.kind {
            BlendKind::Smooth => 0.0,
            BlendKind::Chamfer => 1.0,
        };
        self.instr([
            [OP_COMBINE as f32, c.mode.code() as f32, c.blend.max(0.0), kind],
            [c.depth, c.width, 0.0, 0.0],
            [0.0; 4],
            [0.0; 4],
        ]);
    }

    /// Emits a node; `chain` = point ops of its ancestors.
    fn node(&mut self, id: NodeId, chain: &[PointOp], combine_override: Option<Combine>) {
        let Some(n) = self.scene.node(id) else { return };
        if !n.visible {
            return;
        }
        let mut chain2 = chain.to_vec();
        chain2.extend(node_ops(self.scene, n));
        let combine = combine_override.unwrap_or(n.combine);
        match &n.kind {
            NodeKind::Shape(shape) => {
                if shape.kind == PrimKind::Profile && shape.profile.as_ref().map(|p| p.points.len() < 3).unwrap_or(true) {
                    return;
                }
                self.prim(id, &chain2, shape, n.material, &combine);
                self.combine(&combine);
            }
            NodeKind::Group { children, combined } => {
                if *combined || combine_override.is_some() {
                    self.op(OP_EMPTY, 0.0, 0.0, 0.0);
                    for c in children {
                        self.node(*c, &chain2, None);
                    }
                    self.combine(&combine);
                } else {
                    for c in children {
                        self.node(*c, &chain2, None);
                    }
                }
            }
        }
    }
}

fn ancestor_chain(scene: &Scene, id: NodeId) -> Vec<PointOp> {
    let mut chain = vec![];
    for a in scene.ancestors(id) {
        chain.extend(node_ops(scene, &scene.nodes[&a]));
    }
    chain
}

/// Compiles the whole scene.
pub fn compile(scene: &Scene) -> Program {
    let mut b = Builder { scene, prog: Program::default() };
    b.op(OP_EMPTY, 0.0, 0.0, 0.0);
    if let Some(root) = scene.node(scene.root) {
        let mut chain = vec![];
        chain.extend(node_ops(scene, root));
        for c in root.children().to_vec() {
            b.node(c, &chain, None);
        }
    }
    b.op(OP_END, 0.0, 0.0, 0.0);
    b.prog
}

/// Compiles a single node (with its ancestors' placement) as a standalone
/// volume, used to highlight the selection and to show cutters as ghosts.
pub fn compile_single(scene: &Scene, id: NodeId) -> Program {
    let mut b = Builder { scene, prog: Program::default() };
    b.op(OP_EMPTY, 0.0, 0.0, 0.0);
    let mut chain = ancestor_chain(scene, id);
    if let Some(root) = scene.node(scene.root) {
        let mut c = node_ops(scene, root);
        c.extend(chain);
        chain = c;
    }
    if let Some(n) = scene.node(id) {
        if n.visible {
            let add = Combine { mode: CombineMode::Add, blend: 0.0, ..n.combine };
            b.node(id, &chain, Some(add));
        }
    }
    b.op(OP_END, 0.0, 0.0, 0.0);
    b.prog
}

// ---------------------------------------------------------------------------
// CPU interpreter (mirrors shaders/sdf.wgsl exactly)
// ---------------------------------------------------------------------------

const SQRT1_2: f32 = std::f32::consts::FRAC_1_SQRT_2;

#[inline]
fn vmax(v: Vec3) -> f32 {
    v.x.max(v.y).max(v.z)
}

#[inline]
fn extrude(d2: f32, h: f32) -> f32 {
    let w = Vec2::new(d2, h);
    w.x.max(w.y).min(0.0) + w.max(Vec2::ZERO).length()
}

fn sd_polygon(aux: &[[f32; 4]], off: usize, count: usize, p: Vec2) -> f32 {
    let pt = |i: usize| -> Vec2 {
        let k = off + i;
        let v = aux[k / 2];
        if k % 2 == 0 {
            Vec2::new(v[0], v[1])
        } else {
            Vec2::new(v[2], v[3])
        }
    };
    let v0 = pt(0);
    let mut d = (p - v0).dot(p - v0);
    let mut s = 1.0f32;
    let mut j = count - 1;
    for i in 0..count {
        let vi = pt(i);
        let vj = pt(j);
        let e = vj - vi;
        let w = p - vi;
        let b = w - e * (w.dot(e) / e.dot(e).max(1e-12)).clamp(0.0, 1.0);
        d = d.min(b.dot(b));
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

fn sd_regular_polygon(p: Vec2, r: f32, n: f32) -> f32 {
    let an = std::f32::consts::PI / n;
    let a = p.x.atan2(p.y);
    let m = 2.0 * an;
    let bn = (a - m * (a / m).floor()) - an;
    let l = p.length();
    let mut q = Vec2::new(l * bn.cos(), l * bn.sin().abs());
    q -= r * Vec2::new(an.cos(), an.sin());
    q.y += (-q.y).clamp(0.0, r * an.sin());
    q.length() * q.x.signum()
}

/// Evaluates a primitive in its local space. `w` = the 4 words of the instruction.
pub fn eval_prim(w: &[[f32; 4]], aux: &[[f32; 4]], p_in: Vec3) -> f32 {
    let kind = w[0][1] as u32;
    let size = Vec3::new(w[1][0], w[1][1], w[1][2]);
    let round = w[1][3].max(0.0);
    let chamfer = w[2][0].max(0.0);
    let taper = w[2][1];
    let shell = w[2][2];
    let sides = w[2][3];
    let mut p = p_in;
    let mut lip = 1.0f32;
    if taper.abs() > 1e-4 && kind != 8 {
        let hy = size.y.max(1e-3);
        let t = (p.y / (2.0 * hy) + 0.5).clamp(0.0, 1.0);
        let f = (1.0 - taper * t).max(0.05);
        p.x /= f;
        p.z /= f;
        lip = f.min(1.0) * 0.85;
    }
    let mut d = match kind {
        0 => {
            // box
            let r = round.min(size.min_element());
            let q = p.abs() - (size - Vec3::splat(r));
            let mut d = q.max(Vec3::ZERO).length() + vmax(q).min(0.0) - r;
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
            // cylinder (Y)
            let r = round.min(size.x).min(size.y);
            let dx = Vec2::new(p.x, p.z).length() - size.x;
            let dy = p.y.abs() - size.y;
            let mut d = extrude(dx + r, dy + r) - r;
            if chamfer > 0.0 {
                d = d.max((dx + dy + chamfer) * SQRT1_2);
            }
            d
        }
        2 => {
            // ellipsoid
            let r = size.max(Vec3::splat(1e-3));
            let k0 = (p / r).length();
            let k1 = (p / (r * r)).length();
            if k1 < 1e-6 {
                -r.min_element()
            } else {
                k0 * (k0 - 1.0) / k1
            }
        }
        3 => {
            // capsule (Y)
            let mut q = p;
            q.y -= q.y.clamp(-size.y, size.y);
            q.length() - size.x
        }
        4 => {
            // torus (XZ plane)
            let q = Vec2::new(Vec2::new(p.x, p.z).length() - size.x, p.y);
            q.length() - size.z
        }
        5 => {
            // capped cone: size.x bottom radius, size.y half height, size.z top radius
            let r = round.min(size.y);
            let h = size.y - r;
            let r1 = (size.x - r).max(0.0);
            let r2 = (size.z - r).max(0.0);
            let q = Vec2::new(Vec2::new(p.x, p.z).length(), p.y);
            let k1 = Vec2::new(r2, h);
            let k2 = Vec2::new(r2 - r1, 2.0 * h);
            let ca = Vec2::new(q.x - q.x.min(if q.y < 0.0 { r1 } else { r2 }), q.y.abs() - h);
            let cb = q - k1 + k2 * ((k1 - q).dot(k2) / k2.dot(k2).max(1e-9)).clamp(0.0, 1.0);
            let s = if cb.x < 0.0 && ca.y < 0.0 { -1.0 } else { 1.0 };
            s * ca.dot(ca).min(cb.dot(cb)).sqrt() - r
        }
        6 => {
            // regular prism (Y)
            let r = round.min(size.y);
            let d2 = sd_regular_polygon(Vec2::new(p.x, p.z), size.x - r, sides.max(3.0));
            extrude(d2, p.y.abs() - size.y + r) - r
        }
        7 => {
            // tube
            let r = round;
            let mid = (size.x + size.z) * 0.5;
            let half = ((size.x - size.z) * 0.5).max(1e-3);
            let d2 = (Vec2::new(p.x, p.z).length() - mid).abs() - half;
            extrude(d2 + r, p.y.abs() - size.y + r) - r
        }
        8 => {
            let off = w[3][0] as usize;
            let count = w[3][1] as usize;
            let mode = w[3][2];
            if count < 3 {
                EMPTY_DIST
            } else if mode > 0.5 {
                let q = Vec2::new(Vec2::new(p.x, p.z).length(), p.y);
                sd_polygon(aux, off, count, q) - round
            } else {
                let d2 = sd_polygon(aux, off, count, Vec2::new(p.x, p.y));
                extrude(d2 + round, p.z.abs() - size.z + round) - round
            }
        }
        _ => EMPTY_DIST,
    };
    if shell > 0.0 {
        d = d.abs() - shell;
    }
    d * lip
}

/// Combines accumulated `a` with new `b`. Returns (distance, material).
pub fn combine(a: (f32, f32), b: (f32, f32), w: &[[f32; 4]]) -> (f32, f32) {
    let mode = w[0][1] as u32;
    let k = w[0][2];
    let chamfer = w[0][3] > 0.5;
    let depth = w[1][0];
    let width = w[1][1];
    let (da, ma) = a;
    let (db, mb) = b;
    match mode {
        0 => {
            let m = if db < da { mb } else { ma };
            let d = if k <= 0.0 {
                da.min(db)
            } else if chamfer {
                da.min(db).min((da + db - k) * SQRT1_2)
            } else {
                let h = (k - (da - db).abs()).max(0.0) / k;
                da.min(db) - h * h * k * 0.25
            };
            (d, m)
        }
        1 | 2 => {
            let nb = if mode == 1 { -db } else { db };
            let m = if nb > da { mb } else { ma };
            let d = if k <= 0.0 {
                da.max(nb)
            } else if chamfer {
                da.max(nb).max((da + nb + k) * SQRT1_2)
            } else {
                let h = (k - (da - nb).abs()).max(0.0) / k;
                da.max(nb) + h * h * k * 0.25
            };
            (d, m)
        }
        3 => (da.max((da + depth).min(width - db.abs())), ma),
        4 => {
            let t = (da - depth).max(db.abs() - width);
            let m = if t < da { mb } else { ma };
            (da.min(t), m)
        }
        _ => (da, ma),
    }
}

impl Program {
    pub fn is_empty(&self) -> bool {
        self.prim_count == 0
    }

    /// Runs the program at world point `pw`. Returns (distance, material id).
    pub fn eval(&self, pw: Vec3) -> (f32, u32) {
        let mut stack = [(EMPTY_DIST, 0.0f32); STACK_SIZE];
        let mut sp = 0usize;
        let mut p = pw;
        let mut sc = 1.0f32;
        let mut pc = 0usize;
        let code = &self.code;
        while pc + INSTR_WORDS <= code.len() {
            let w = &code[pc..pc + INSTR_WORDS];
            let op = w[0][0] as u32;
            match op {
                OP_END => break,
                OP_EMPTY => {
                    if sp < STACK_SIZE {
                        stack[sp] = (EMPTY_DIST, 0.0);
                        sp += 1;
                    }
                }
                OP_BOUND => {
                    let c = Vec3::new(w[1][0], w[1][1], w[1][2]);
                    let bd = (pw - c).length() - w[1][3];
                    let a = if sp > 0 { stack[sp - 1].0 } else { EMPTY_DIST };
                    if bd > a.abs() + w[0][2] + 0.5 {
                        let neutral = if w[0][1] as u32 == 2 { bd } else { EMPTY_DIST };
                        if sp < STACK_SIZE {
                            stack[sp] = (neutral, 0.0);
                            sp += 1;
                        }
                        pc += (w[0][3] as usize + 1) * INSTR_WORDS;
                        continue;
                    }
                }
                OP_RESET => {
                    p = pw;
                    sc = 1.0;
                }
                OP_AFFINE => {
                    let r0 = Vec4::from_array(w[1]);
                    let r1 = Vec4::from_array(w[2]);
                    let r2 = Vec4::from_array(w[3]);
                    let p4 = p.extend(1.0);
                    p = Vec3::new(r0.dot(p4), r1.dot(p4), r2.dot(p4));
                    sc *= w[0][1];
                }
                OP_RADIAL => {
                    let n = w[0][1].max(1.0);
                    let axis = w[0][2] as u32;
                    let (u, v) = match axis {
                        0 => (p.y, p.z),
                        1 => (p.z, p.x),
                        _ => (p.x, p.y),
                    };
                    // angle measured from the first in-plane axis
                    let sector = std::f32::consts::TAU / n;
                    let a = v.atan2(u);
                    let a2 = a + sector * 0.5;
                    let mut a3 = a2 - sector * (a2 / sector).floor() - sector * 0.5;
                    if w[0][3] > 0.5 && n > 1.5 {
                        a3 -= if a3 >= 0.0 { sector } else { -sector };
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
                }
                OP_LINEAR => {
                    let n = w[0][1].max(1.0);
                    let step = Vec3::new(w[1][0], w[1][1], w[1][2]);
                    let origin = Vec3::new(w[2][0], w[2][1], w[2][2]);
                    let ss = step.dot(step);
                    if ss > 1e-9 {
                        let u = (p - origin).dot(step) / ss;
                        let mut t = u.round().clamp(0.0, n - 1.0);
                        if w[0][2] > 0.5 {
                            t = (t + if u > t { 1.0 } else { -1.0 }).clamp(0.0, n - 1.0);
                        }
                        p -= step * t;
                    }
                }
                OP_MIRROR => {
                    if w[0][1] > 0.5 {
                        p.x = p.x.abs();
                    }
                    if w[0][2] > 0.5 {
                        p.y = p.y.abs();
                    }
                    if w[0][3] > 0.5 {
                        p.z = p.z.abs();
                    }
                }
                OP_PRIM => {
                    let d = eval_prim(w, &self.aux, p) * sc;
                    if sp < STACK_SIZE {
                        stack[sp] = (d, w[0][2]);
                        sp += 1;
                    }
                }
                OP_COMBINE => {
                    if sp >= 2 {
                        let b = stack[sp - 1];
                        let a = stack[sp - 2];
                        sp -= 1;
                        stack[sp - 1] = combine(a, b, w);
                    }
                }
                _ => {}
            }
            pc += INSTR_WORDS;
        }
        if sp == 0 {
            return (EMPTY_DIST, 0);
        }
        (stack[0].0, stack[0].1 as u32)
    }

    pub fn dist(&self, p: Vec3) -> f32 {
        self.eval(p).0
    }

    pub fn normal(&self, p: Vec3, eps: f32) -> Vec3 {
        let k1 = Vec3::new(1.0, -1.0, -1.0);
        let k2 = Vec3::new(-1.0, -1.0, 1.0);
        let k3 = Vec3::new(-1.0, 1.0, -1.0);
        let k4 = Vec3::new(1.0, 1.0, 1.0);
        let n = k1 * self.dist(p + k1 * eps)
            + k2 * self.dist(p + k2 * eps)
            + k3 * self.dist(p + k3 * eps)
            + k4 * self.dist(p + k4 * eps);
        n.normalize_or(Vec3::Y)
    }

    /// Sphere-traces a ray. Returns the hit distance along `dir`.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_t: f32) -> Option<f32> {
        if self.is_empty() {
            return None;
        }
        let mut t = 0.0f32;
        for _ in 0..400 {
            let d = self.dist(origin + dir * t);
            if d.abs() < 1e-3 * (1.0 + t * 0.01) {
                return Some(t);
            }
            t += d * 0.9;
            if t > max_t {
                break;
            }
        }
        None
    }

    /// Raw words as a flat f32 slice (for GPU upload).
    pub fn code_f32(&self) -> &[f32] {
        bytemuck::cast_slice(&self.code)
    }
}
