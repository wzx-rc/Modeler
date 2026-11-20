//! Forms (Romain, 20 nov. 13 h 05: "un outil qui s'appellerait des
//! formes… le feeling… sur un Nomad ou un ZBrush… j'ai une surface tendue,
//! une sphère ou un cylindre… une brosse avec un radius… l'équivalent d'une
//! move brush… tirer la surface… via des réglages de radius, d'intensité"):
//! a form is one smooth NURBS surface closed on itself (`nurbs_kernel::Form`,
//! a bicubic BSpline the kernel makes a solid), pulled by brushes that move
//! its poles. Each stroke is computed again from the poles as they were at
//! the press (no build-up from frame to frame) and is one undo step; while
//! it lasts, and until the kernel's model of it arrives, the app draws the
//! surface itself from its poles (`form_preview`), so it follows the
//! pointer at once.

use crate::state::{basis_rotation, AppState, Tool};
use glam::{DMat3, DVec3};
use nurbs_kernel::{FeatureId, Form, FormKind, Op, Prim};

/// The brushes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brush {
    /// The surface under the pointer follows it (in the plane facing the
    /// eye), its surroundings less and less out to the radius.
    Move,
    /// Out along the surface's normals (the pointer up), in (down).
    Inflate,
    /// The bumps evened out, more the longer the drag.
    Smooth,
    /// Onto the plane tangent where the stroke began.
    Flatten,
    /// Drawn in towards the point pressed: a crease (Ctrl: spread).
    Pinch,
}

pub const BRUSHES: [Brush; 5] = [Brush::Move, Brush::Inflate, Brush::Smooth, Brush::Flatten, Brush::Pinch];

impl Brush {
    pub fn label(self) -> &'static str {
        match self {
            Brush::Move => "Move",
            Brush::Inflate => "Inflate",
            Brush::Smooth => "Smooth",
            Brush::Flatten => "Flatten",
            Brush::Pinch => "Pinch",
        }
    }
    pub fn index(self) -> usize {
        BRUSHES.iter().position(|b| *b == self).unwrap_or(0)
    }
    pub fn tip(self) -> &'static str {
        match self {
            Brush::Move => "Move: press on the form and drag; the surface under the pointer follows it, its surroundings less and less out to the radius",
            Brush::Inflate => "Inflate: press and drag up to swell the surface along its normals, down to sink it (Ctrl: the other way)",
            Brush::Smooth => "Smooth: press and drag; the longer the drag, the smoother (Shift with any brush: Smooth)",
            Brush::Flatten => "Flatten: press and drag; the surface goes onto the plane tangent where you pressed",
            Brush::Pinch => "Pinch: press and drag; the surface is drawn in towards the point pressed, a crease (Ctrl: spread)",
        }
    }
}

/// The brushes' settings: one radius on screen for all, an intensity
/// each (as Nomad keeps them per tool).
#[derive(Clone, Debug)]
pub struct FormSettings {
    pub brush: Brush,
    /// Radius in screen points.
    pub radius: f32,
    pub intensity: [f64; 5],
    /// Symmetry across the form's own middle (its local X).
    pub mirror: bool,
    /// What a click on the ground makes.
    pub new_kind: FormKind,
}

impl Default for FormSettings {
    fn default() -> Self {
        FormSettings { brush: Brush::Move, radius: 90.0, intensity: [1.0, 0.5, 0.5, 0.6, 0.5], mirror: false, new_kind: FormKind::Sphere }
    }
}

/// A stroke under way.
pub struct Sculpt {
    pub id: FeatureId,
    pub brush: Brush,
    /// The poles in the world at the press.
    pub start: Vec<DVec3>,
    pub nu: usize,
    pub nv: usize,
    /// Their normals then (outwards).
    pub normals: Vec<DVec3>,
    /// The point pressed on the surface, its normal (towards the eye),
    /// the screen point.
    pub hit: DVec3,
    pub n: DVec3,
    pub press: [f32; 2],
    /// mm per screen point at the hit.
    pub px: f64,
    /// Each pole's weight from the hit (with the intensity), and from its
    /// mirror (empty without symmetry), the gain that makes the surface
    /// under the pointer follow it.
    pub w: Vec<f64>,
    pub wm: Vec<f64>,
    pub gain: f64,
    pub gain_m: f64,
    /// The mirror: the form's middle plane at the press (point, normal).
    pub mirror: Option<(DVec3, DVec3)>,
    /// Ctrl at the press: the brush the other way.
    pub invert: bool,
}

/// The smooth falloff of a brush, 1 at its centre, 0 at its radius.
fn falloff(d: f64) -> f64 {
    if d >= 1.0 {
        0.0
    } else {
        1.0 - d * d * (3.0 - 2.0 * d)
    }
}

fn reflect(p: DVec3, m: (DVec3, DVec3)) -> DVec3 {
    p - m.1 * (2.0 * (p - m.0).dot(m.1))
}

fn reflect_v(v: DVec3, m: (DVec3, DVec3)) -> DVec3 {
    v - m.1 * (2.0 * v.dot(m.1))
}

/// The poles' normals (from their neighbours in the net), outwards.
fn pole_normals(p: &[DVec3], nu: usize, nv: usize) -> Vec<DVec3> {
    let at = |i: i64, j: usize| p[j * nu + i.rem_euclid(nu as i64) as usize];
    let c = p.iter().copied().sum::<DVec3>() / p.len() as f64;
    let mut out = vec![DVec3::ZERO; p.len()];
    for j in 0..nv {
        for i in 0..nu as i64 {
            let n = if j == 0 || j + 1 == nv {
                // a collapsed row: from the ring two rows in to the point
                let r = if j == 0 { 2.min(nv - 1) } else { nv.saturating_sub(3) };
                let ring = (0..nu as i64).map(|k| at(k, r)).sum::<DVec3>() / nu as f64;
                (at(0, j) - ring).normalize_or_zero()
            } else {
                let du = at(i + 1, j) - at(i - 1, j);
                let dv = at(i, j + 1) - at(i, j - 1);
                du.cross(dv).normalize_or_zero()
            };
            out[j * nu + i as usize] = n;
        }
    }
    // outwards: most point away from the middle
    let s: f64 = out.iter().zip(p).map(|(n, q)| n.dot(*q - c)).sum();
    if s < 0.0 {
        for n in out.iter_mut().skip(nu).take(nu * (nv - 2)) {
            *n = -*n;
        }
    }
    out
}

/// The parameters of the surface's point nearest `q` (a search on a grid
/// of 4 points per span).
fn nearest_param(p: &[DVec3], nu: usize, nv: usize, q: DVec3) -> (f64, f64) {
    let (ru, rv) = (nu as f64, (nv - 3) as f64);
    let (su, sv) = (nu * 4, (nv - 3) * 4);
    let mut best = (f64::MAX, 0.0, 0.0);
    for j in 0..=sv {
        for i in 0..su {
            let (u, v) = (ru * i as f64 / su as f64, rv * j as f64 / sv as f64);
            let d = Form::point(p, nu, nv, u, v).distance_squared(q);
            if d < best.0 {
                best = (d, u, v);
            }
        }
    }
    (best.1, best.2)
}

/// The share of the weights the surface's point near `q` gets: the gain
/// that makes it move as much as the brush asks (at most 4).
fn gain_at(p: &[DVec3], nu: usize, nv: usize, q: DVec3, w: &[f64]) -> f64 {
    let (u, v) = nearest_param(p, nu, nv, q);
    let s: f64 = Form::basis(nu, nv, u, v).iter().map(|(k, b)| b * w[*k]).sum();
    let peak = w.iter().copied().fold(0.0, f64::max);
    if s <= 1e-6 || peak <= 1e-9 {
        return 1.0;
    }
    (peak / s).clamp(1.0, 4.0)
}

/// Rows 0 and `nv − 1` stay one point each.
fn collapse_ends(p: &mut [DVec3], nu: usize, nv: usize) {
    for j in [0, nv - 1] {
        let m = p[j * nu..(j + 1) * nu].iter().copied().sum::<DVec3>() / nu as f64;
        for q in &mut p[j * nu..(j + 1) * nu] {
            *q = m;
        }
    }
}

/// One step of smoothing: each pole towards its neighbours' middle by its
/// weight times λ.
fn smooth_step(p: &mut [DVec3], nu: usize, nv: usize, w: &[f64], lambda: f64) {
    let at = |p: &[DVec3], i: i64, j: usize| p[j * nu + i.rem_euclid(nu as i64) as usize];
    let old = p.to_vec();
    for j in 0..nv {
        for i in 0..nu as i64 {
            let k = j * nu + i as usize;
            if w[k] <= 0.0 {
                continue;
            }
            let avg = if j == 0 || j + 1 == nv {
                let r = if j == 0 { 1 } else { nv - 2 };
                (0..nu as i64).map(|x| at(&old, x, r)).sum::<DVec3>() / nu as f64
            } else {
                (at(&old, i - 1, j) + at(&old, i + 1, j) + at(&old, i, j - 1) + at(&old, i, j + 1)) / 4.0
            };
            p[k] = old[k] + (avg - old[k]) * (lambda * w[k]);
        }
    }
}

impl Sculpt {
    /// The poles for the pointer at `p` (screen), the plane's hits given.
    fn poles(&self, st: &AppState, p: [f32; 2]) -> Vec<DVec3> {
        let (nu, nv) = (self.nu, self.nv);
        let mut out = self.start.clone();
        let drag = ((p[0] - self.press[0]).powi(2) + (p[1] - self.press[1]).powi(2)).sqrt() as f64;
        let sign = if self.invert { -1.0 } else { 1.0 };
        // each pole's own effect at full weight, from the hit and from its
        // mirror; blended where both reach so the middle is not pulled twice
        let blend = |k: usize, a: DVec3, b: DVec3| -> DVec3 {
            let (wa, wb) = (self.w[k] * self.gain, self.wm.get(k).copied().unwrap_or(0.0) * self.gain_m);
            if wa + wb <= 0.0 {
                return DVec3::ZERO;
            }
            (a * wa + b * wb) * (wa.max(wb) / (wa + wb))
        };
        match self.brush {
            Brush::Move => {
                // the pointer's travel on the plane through the hit facing the eye
                let eye = st.view().eye.as_dvec3();
                let nrm = (eye - self.hit).normalize_or(DVec3::Y);
                let on = |q: [f32; 2]| {
                    let (o, d) = st.ray(q);
                    let den = d.dot(nrm);
                    if den.abs() < 1e-9 {
                        return None;
                    }
                    Some(o + d * ((self.hit - o).dot(nrm) / den))
                };
                let (Some(a), Some(b)) = (on(self.press), on(p)) else { return out };
                let delta = b - a;
                let dm = self.mirror.map(|m| reflect_v(delta, m)).unwrap_or(delta);
                for k in 0..out.len() {
                    out[k] += blend(k, delta, dm);
                }
            }
            Brush::Inflate => {
                // up the screen: out
                let amount = (self.press[1] - p[1]) as f64 * self.px * sign;
                for k in 0..out.len() {
                    let n = self.normals[k];
                    out[k] += blend(k, n * amount, n * amount);
                }
            }
            Brush::Flatten => {
                let s = (drag / 120.0).min(1.0);
                let m = self.mirror.map(|m| (reflect(self.hit, m), reflect_v(self.n, m)));
                for k in 0..out.len() {
                    let q = self.start[k];
                    let a = -self.n * ((q - self.hit).dot(self.n) * s);
                    let b = m.map(|(h, n)| -n * ((q - h).dot(n) * s)).unwrap_or(a);
                    out[k] += blend(k, a, b) / self.gain.max(1.0);
                }
            }
            Brush::Pinch => {
                let s = (drag / 100.0).min(1.6) * sign;
                let m = self.mirror.map(|m| (reflect(self.hit, m), reflect_v(self.n, m)));
                let pull = |q: DVec3, h: DVec3, n: DVec3| {
                    let off = q - h;
                    -(off - n * off.dot(n)) * s
                };
                for k in 0..out.len() {
                    let q = self.start[k];
                    let a = pull(q, self.hit, self.n);
                    let b = m.map(|(h, n)| pull(q, h, n)).unwrap_or(a);
                    out[k] += blend(k, a, b) / self.gain.max(1.0);
                }
            }
            Brush::Smooth => {
                let w: Vec<f64> = (0..out.len()).map(|k| self.w[k].max(self.wm.get(k).copied().unwrap_or(0.0)).min(1.0)).collect();
                let steps = ((drag / 6.0).ceil() as usize).min(80);
                for _ in 0..steps {
                    smooth_step(&mut out, nu, nv, &w, 0.6);
                }
            }
        }
        collapse_ends(&mut out, nu, nv);
        out
    }
}

impl AppState {
    /// Arms the Forms tool (M).
    pub fn arm_form(&mut self) {
        self.abort_gestures();
        self.tool = Tool::Form;
    }

    /// The form (or a plain Cube or Cylinder that can become one) under
    /// the pointer.
    pub fn form_under(&self, p: [f32; 2]) -> Option<(FeatureId, crate::pick::Hit)> {
        let h = self.pick_at(p)?;
        let id = self.feature_of_face(h.face)?;
        Some((id, h))
    }

    /// A plain Cube or Cylinder with nothing done on it: it can become a form.
    pub fn can_be_form(&self, id: FeatureId) -> bool {
        let Some(f) = self.doc.feature(id) else { return false };
        f.form.is_none()
            && f.is_solid()
            && matches!(f.prim, Prim::Box { .. } | Prim::Cylinder { .. })
            && f.profile.is_none()
            && f.curve.is_none()
            && f.shape_bool.is_none()
            && f.sheet.is_none()
            && self.element_ops(id).is_empty()
    }

    /// Makes a plain Cube (a soft box) or Cylinder a form of the same size
    /// (in the undo step under way).
    fn make_form(&mut self, id: FeatureId) {
        self.live(|d| {
            let Some(f) = d.feature_mut(id) else { return };
            let (fm, size) = match f.prim {
                Prim::Box { x, y, z } => (Form::cylinder(24, true), (x, y, z)),
                Prim::Cylinder { radius, height } => (Form::cylinder(24, false), (radius * 2.0, height, radius * 2.0)),
                _ => return,
            };
            f.prim = Prim::Box { x: size.0, y: size.1, z: size.2 };
            f.form = Some(fm);
        });
    }

    /// A press with the Forms tool at `p`: a stroke begins on the form
    /// there (a plain Cube or Cylinder becomes one first). False: no form
    /// there.
    pub fn form_press(&mut self, p: [f32; 2], shift: bool, ctrl: bool) -> bool {
        let Some((id, h)) = self.form_under(p) else { return false };
        let is_form = self.doc.feature(id).map(|f| f.form.is_some()).unwrap_or(false);
        if !is_form {
            if !self.can_be_form(id) {
                self.toast("Forms: a form, or a plain Cube or Cylinder with nothing done on it (it becomes a form)", true);
                return false;
            }
            self.begin_live("Make it a form");
            self.make_form(id);
        }
        let brush = if shift { Brush::Smooth } else { self.form.brush };
        self.begin_live(&format!("{} brush", brush.label()));
        let Some(f) = self.doc.feature(id).cloned() else { return false };
        let Some(fm) = f.form.clone() else { return false };
        let start = f.form_poles();
        let hit = h.point.as_dvec3();
        let px = self.mm_per_point(hit);
        let r = (self.form.radius as f64 * px).max(1e-3);
        let intensity = self.form.intensity[brush.index()];
        let w: Vec<f64> = start.iter().map(|q| falloff(q.distance(hit) / r) * intensity).collect();
        let m = f.matrix();
        let mirror = self.form.mirror.then(|| (m.transform_point3(DVec3::ZERO), DMat3::from_mat4(m) * DVec3::X));
        let (wm, gain_m) = match mirror {
            Some(mi) => {
                let hm = reflect(hit, mi);
                let wm: Vec<f64> = start.iter().map(|q| falloff(q.distance(hm) / r) * intensity).collect();
                let g = gain_at(&start, fm.nu, fm.nv, hm, &wm);
                (wm, g)
            }
            None => (vec![], 1.0),
        };
        let gain = gain_at(&start, fm.nu, fm.nv, hit, &w);
        let normals = pole_normals(&start, fm.nu, fm.nv);
        self.sculpt = Some(Sculpt { id, brush, start, nu: fm.nu, nv: fm.nv, normals, hit, n: h.normal.as_dvec3().normalize_or(DVec3::Y), press: p, px, w, wm, gain, gain_m, mirror, invert: ctrl });
        self.form_shown = Some(id);
        self.selection = vec![id];
        true
    }

    /// The pointer moved during a stroke.
    pub fn form_drag(&mut self, p: [f32; 2]) {
        let Some(s) = self.sculpt.as_ref() else { return };
        let poles = s.poles(self, p);
        let id = s.id;
        self.live(|d| {
            if let Some(f) = d.feature_mut(id) {
                f.set_form_poles(&poles);
            }
        });
        self.interacting = true;
    }

    /// The stroke ends: one undo step.
    pub fn form_release(&mut self) {
        if self.sculpt.take().is_some() {
            self.end_live();
        }
    }

    /// A click with the Forms tool: on a form, selects it; elsewhere a new
    /// form (a sphere or a cylinder, 30 mm) on the face or the ground there.
    pub fn form_click(&mut self, p: [f32; 2]) {
        if let Some((id, _)) = self.form_under(p) {
            if self.doc.feature(id).map(|f| f.form.is_some()).unwrap_or(false) {
                self.selection = vec![id];
                return;
            }
        }
        let Some((plane, a, _)) = self.start_plane(p) else { return };
        let origin = plane.at(a);
        let rot = basis_rotation(plane.u, plane.n);
        let kind = self.form.new_kind;
        let op = self.mode;
        self.begin_live("New form");
        let mut id = 0;
        self.live(|d| {
            let name = d.fresh_name("Form");
            id = d.add(op, Prim::Box { x: 30.0, y: 30.0, z: 30.0 }, origin, rot);
            if let Some(f) = d.feature_mut(id) {
                f.name = name;
                f.form = Some(match kind {
                    FormKind::Sphere => Form::sphere(24, 13),
                    FormKind::Cylinder => Form::cylinder(24, false),
                });
            }
        });
        self.end_live();
        self.selection = vec![id];
    }

    /// Twice the poles round and up (the same surface): finer brushes.
    pub fn form_detail(&mut self, id: FeatureId) {
        let Some(f) = self.doc.feature(id).cloned() else { return };
        let Some(fm) = f.form.clone() else { return };
        if fm.nu >= 96 {
            self.toast("Forms: as fine as it goes (96 poles round)", false);
            return;
        }
        let r = fm.refined();
        let mut g = f.clone();
        g.form = Some(r.clone());
        // the box fitted to the new poles (inside the old ones)
        let world: Vec<DVec3> = {
            let mut h = f.clone();
            h.form = Some(r);
            h.form_poles()
        };
        g.set_form_poles(&world);
        self.begin_live("More detail");
        self.live(|d| {
            if let Some(x) = d.feature_mut(id) {
                *x = g;
            }
        });
        self.end_live();
    }

    /// The form being pulled, drawn by the app from its poles while the
    /// stroke lasts and until the kernel's model of it arrives: its
    /// triangles (positions, normals). Not for a Carve (its surface is a
    /// hole in the part).
    pub fn form_preview(&self) -> Option<(FeatureId, Vec<([f32; 3], [f32; 3])>)> {
        let id = self.form_shown?;
        let caught_up = self.model.as_ref().map(|m| m.serial / 4 == self.version).unwrap_or(false);
        if self.sculpt.is_none() && caught_up {
            return None;
        }
        let f = self.doc.feature(id).filter(|f| f.op == Op::Add && f.visible)?;
        let fm = f.form.as_ref()?;
        Some((id, tessellate(&f.form_poles(), fm.nu, fm.nv)))
    }
}

/// The surface's triangles (4 per span each way, at most 192 × 160).
pub fn tessellate(p: &[DVec3], nu: usize, nv: usize) -> Vec<([f32; 3], [f32; 3])> {
    let su = (nu * 4).min(192);
    let sv = ((nv - 3) * 4).min(160);
    let (ru, rv) = (nu as f64, (nv - 3) as f64);
    let mut pts = Vec::with_capacity(su * (sv + 1));
    for j in 0..=sv {
        for i in 0..su {
            pts.push(Form::point(p, nu, nv, ru * i as f64 / su as f64, rv * j as f64 / sv as f64));
        }
    }
    let idx = |i: usize, j: usize| j * su + i % su;
    let mut nrm = vec![DVec3::ZERO; pts.len()];
    let mut tris = Vec::with_capacity(su * sv * 2);
    for j in 0..sv {
        for i in 0..su {
            let (a, b, c, d) = (idx(i, j), idx(i + 1, j), idx(i + 1, j + 1), idx(i, j + 1));
            for t in [[a, b, c], [a, c, d]] {
                let n = (pts[t[1]] - pts[t[0]]).cross(pts[t[2]] - pts[t[0]]);
                if n.length_squared() < 1e-18 {
                    continue;
                }
                for v in t {
                    nrm[v] += n;
                }
                tris.push(t);
            }
        }
    }
    // the collapsed rows: one normal each
    for j in [0, sv] {
        let n = (0..su).map(|i| nrm[idx(i, j)]).sum::<DVec3>();
        for i in 0..su {
            nrm[idx(i, j)] = n;
        }
    }
    let c = pts.iter().copied().sum::<DVec3>() / pts.len() as f64;
    let out_sign = if nrm.iter().zip(&pts).map(|(n, q)| n.dot(*q - c)).sum::<f64>() < 0.0 { -1.0 } else { 1.0 };
    let mut out = Vec::with_capacity(tris.len() * 3);
    for t in tris {
        for v in t {
            out.push((pts[v].as_vec3().to_array(), (nrm[v].normalize_or_zero() * out_sign).as_vec3().to_array()));
        }
    }
    out
}
