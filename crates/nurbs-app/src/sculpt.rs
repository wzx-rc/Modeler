//! The Sculpt tab (Romain, 20 nov. 2026: "dans Sculpt Mode, on va avoir un
//! petit toolset de brush"): clay meshes made in the scene (a sphere, a
//! block, or a Poly mesh sent here) and shaped with a few brushes, ZBrush
//! / Blender style: Draw, Clay, Smooth, Grab, Inflate, Flatten, Pinch. The
//! brush acts on the clay under the pointer; Ctrl digs instead of raising,
//! Shift smooths whatever the brush, X mirrors across the mesh's own X.
//! The radius is on screen (points), as in ZBrush; [ and ] change it.
//!
//! A stroke is one undo step (`begin_live` / `end_live`); the mesh's data is
//! changed in place meanwhile (no kernel request per dab: `mesh_gen`).

use crate::meshes::{matrix, refit, to_unit, world_points};
use crate::state::{AppState, Tool};
use glam::Vec3;
use nurbs_kernel::mesh::{icosphere, quad_sphere, triangulated, MeshKind};
use nurbs_kernel::FeatureId;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brush {
    Draw,
    Clay,
    Smooth,
    Grab,
    Inflate,
    Flatten,
    Pinch,
}

impl Brush {
    pub const ALL: [Brush; 7] = [Brush::Draw, Brush::Clay, Brush::Smooth, Brush::Grab, Brush::Inflate, Brush::Flatten, Brush::Pinch];

    pub fn label(self) -> &'static str {
        match self {
            Brush::Draw => "Draw",
            Brush::Clay => "Clay",
            Brush::Smooth => "Smooth",
            Brush::Grab => "Grab",
            Brush::Inflate => "Inflate",
            Brush::Flatten => "Flatten",
            Brush::Pinch => "Pinch",
        }
    }

    /// Its key (Blender's where it has one).
    pub fn key(self) -> egui::Key {
        match self {
            Brush::Draw => egui::Key::X,
            Brush::Clay => egui::Key::C,
            Brush::Smooth => egui::Key::S,
            Brush::Grab => egui::Key::G,
            Brush::Inflate => egui::Key::I,
            Brush::Flatten => egui::Key::T,
            Brush::Pinch => egui::Key::P,
        }
    }

    pub fn key_name(self) -> &'static str {
        match self {
            Brush::Draw => "X",
            Brush::Clay => "C",
            Brush::Smooth => "S",
            Brush::Grab => "G",
            Brush::Inflate => "I",
            Brush::Flatten => "T",
            Brush::Pinch => "P",
        }
    }

    pub fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            Brush::Draw => Icon::BrushDraw,
            Brush::Clay => Icon::BrushClay,
            Brush::Smooth => Icon::BrushSmooth,
            Brush::Grab => Icon::BrushGrab,
            Brush::Inflate => Icon::BrushInflate,
            Brush::Flatten => Icon::BrushFlatten,
            Brush::Pinch => Icon::BrushPinch,
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Brush::Draw => "Draw (X): raises the clay along the surface (Ctrl: digs)",
            Brush::Clay => "Clay (C): lays flat strips of clay, builds forms up (Ctrl: carves)",
            Brush::Smooth => "Smooth (S, or Shift with any brush): evens the surface out",
            Brush::Grab => "Grab (G): pulls the clay under the brush along with the pointer",
            Brush::Inflate => "Inflate (I): swells the clay along its own normals (Ctrl: deflates)",
            Brush::Flatten => "Flatten (T): presses the clay onto its mean plane",
            Brush::Pinch => "Pinch (P): pulls the clay towards the brush's centre, sharpens creases (Ctrl: spreads)",
        }
    }
}

/// The brush's settings (kept while the app runs).
#[derive(Clone, Copy, Debug)]
pub struct SculptSettings {
    pub brush: Brush,
    /// On screen, points.
    pub radius: f32,
    /// 0 to 1.
    pub strength: f32,
    /// Mirrored across the mesh's own X (its box's middle).
    pub mirror: bool,
}

impl Default for SculptSettings {
    fn default() -> Self {
        SculptSettings { brush: Brush::Draw, radius: 60.0, strength: 0.5, mirror: true }
    }
}

/// Grab: the points taken when the stroke began, how much each follows.
#[derive(Clone, Debug)]
pub struct GrabSet {
    pub start: Vec<Vec3>,
    pub moved: Vec<(u32, f32, bool)>,
    /// The plane facing the eye through the press.
    pub at: Vec3,
    pub normal: Vec3,
}

/// A stroke under way.
#[derive(Clone, Debug)]
pub struct Stroke {
    pub id: FeatureId,
    pub brush: Brush,
    pub invert: bool,
    /// World radius (mm), fixed at the press.
    pub radius: f32,
    pub last: Vec3,
    pub grab: Option<GrabSet>,
    pub nb: Arc<Vec<Vec<u32>>>,
    /// A step was made (the mesh changed).
    pub dabs: usize,
}

/// The brush's falloff: 1 at the centre, 0 at the rim, smooth both ends.
fn falloff(x: f32) -> f32 {
    if x >= 1.0 {
        return 0.0;
    }
    let t = 1.0 - x * x;
    t * t
}

/// One dab of `brush` at `c` (world), radius `r`, on the points `p` (world).
#[allow(clippy::too_many_arguments)]
fn dab(brush: Brush, invert: bool, strength: f32, p: &mut [Vec3], n: &[Vec3], nb: &[Vec<u32>], c: Vec3, r: f32, changed: &mut [bool]) {
    let sign = if invert { -1.0 } else { 1.0 };
    let r2 = r * r;
    let near: Vec<(usize, f32)> = p.iter().enumerate().filter_map(|(i, q)| {
        let d2 = (*q - c).length_squared();
        (d2 < r2).then(|| (i, falloff(d2.sqrt() / r)))
    }).collect();
    if near.is_empty() {
        return;
    }
    // the area's normal and centre
    let mut an = Vec3::ZERO;
    let mut ac = Vec3::ZERO;
    let mut wsum = 0.0;
    for &(i, w) in &near {
        an += n[i] * w;
        ac += p[i] * w;
        wsum += w;
    }
    let an = an.normalize_or(Vec3::Y);
    let ac = ac / wsum.max(1e-6);
    let s = strength.clamp(0.0, 1.0);
    match brush {
        Brush::Draw => {
            for &(i, w) in &near {
                p[i] += an * (r * 0.06 * s * w * sign);
                changed[i] = true;
            }
        }
        Brush::Inflate => {
            for &(i, w) in &near {
                p[i] += n[i] * (r * 0.05 * s * w * sign);
                changed[i] = true;
            }
        }
        Brush::Clay => {
            // a plane a little above the area's (below when digging): the
            // clay under it comes up to it
            let plane = ac + an * (r * 0.12 * s * sign);
            for &(i, w) in &near {
                let d = (p[i] - plane).dot(an) * sign;
                if d < 0.0 {
                    p[i] -= an * (d * sign * (0.55 * s * w));
                    changed[i] = true;
                }
            }
        }
        Brush::Flatten => {
            for &(i, w) in &near {
                let d = (p[i] - ac).dot(an);
                p[i] -= an * (d * 0.45 * s * w);
                changed[i] = true;
            }
        }
        Brush::Smooth => {
            let old: Vec<Vec3> = near.iter().map(|(i, _)| p[*i]).collect();
            let avg: Vec<Vec3> = near
                .iter()
                .map(|(i, _)| {
                    let ns = &nb[*i];
                    if ns.is_empty() {
                        p[*i]
                    } else {
                        ns.iter().map(|j| p[*j as usize]).sum::<Vec3>() / ns.len() as f32
                    }
                })
                .collect();
            for (k, &(i, w)) in near.iter().enumerate() {
                p[i] = old[k].lerp(avg[k], (0.9 * s.max(0.25) * w).min(1.0));
                changed[i] = true;
            }
        }
        Brush::Pinch => {
            for &(i, w) in &near {
                let to = c - p[i];
                let tangent = to - an * to.dot(an);
                p[i] += tangent * (0.12 * s * w * sign);
                changed[i] = true;
            }
        }
        Brush::Grab => {}
    }
}

/// The point mirrored across the mesh's own X.
fn mirrored(m: glam::Mat4, inv: glam::Mat4, q: Vec3) -> Vec3 {
    let l = inv.transform_point3(q);
    m.transform_point3(Vec3::new(-l.x, l.y, l.z))
}

impl AppState {
    /// A new clay mesh, 20 mm across, its centre on the view's target: a
    /// sphere (`block` false) or a rounded block.
    pub fn add_sculpt(&mut self, block: bool) {
        let s = 20.0f32;
        let (p, f) = if block {
            let (p, q) = quad_sphere(s, 64, 0.18);
            (p, triangulated(&q))
        } else {
            icosphere(s, 6)
        };
        let at = self.camera.target.as_dvec3() - glam::DVec3::new(0.0, s as f64 * 0.5, 0.0);
        let name = if block { "Clay block" } else { "Clay" };
        let mut made = 0;
        self.edit(if block { "New clay block" } else { "New clay sphere" }, |d| {
            made = d.add_mesh(MeshKind::Sculpt, &p, f, at, glam::DQuat::IDENTITY, name);
        });
        self.selection = vec![made];
        self.mesh_gen += 1;
    }

    /// A brush picked (its key or its button): armed on the clay.
    pub fn set_brush(&mut self, b: Brush) {
        if self.tool != Tool::Sculpt {
            self.commit_gestures(self.principal());
        }
        self.sculpt.brush = b;
        self.sel_mode = crate::state::SelMode::Object;
        self.tool = Tool::Sculpt;
    }

    /// The world radius of the brush where the pointer meets the clay.
    pub fn brush_world_radius(&self, at: Vec3) -> f32 {
        let v = self.view();
        v.pixel_size_at((at - v.eye).length()) * self.sculpt.radius
    }

    /// A press with a brush: the stroke begins on the clay under the
    /// pointer (none there: nothing).
    pub fn begin_stroke(&mut self, p: [f32; 2], ctrl: bool, shift: bool) -> bool {
        let Some(h) = self.pick_mesh(p, Some(MeshKind::Sculpt)) else { return false };
        let Some(f) = self.doc.feature(h.id).cloned() else { return false };
        let Some(m) = f.mesh.as_ref() else { return false };
        // the clay's neighbours, kept while its faces stay (a stroke never
        // changes them)
        let key = h.id as usize;
        let nb = match &self.sculpt_nb {
            Some((k, n, len)) if *k == key && *len == m.faces.len() && n.len() == m.verts.len() => n.clone(),
            _ => Arc::new(m.neighbours()),
        };
        self.sculpt_nb = Some((key, nb.clone(), m.faces.len()));
        let brush = if shift { Brush::Smooth } else { self.sculpt.brush };
        let radius = self.brush_world_radius(h.point);
        self.begin_live(&format!("{} stroke", brush.label()));
        let mut grab = None;
        if brush == Brush::Grab {
            let pts = world_points(&f);
            let x = matrix(&f);
            let inv = x.inverse();
            let mut moved = vec![];
            for (i, q) in pts.iter().enumerate() {
                let d = (*q - h.point).length();
                if d < radius {
                    moved.push((i as u32, falloff(d / radius), false));
                }
            }
            if self.sculpt.mirror {
                let c2 = mirrored(x, inv, h.point);
                if (c2 - h.point).length() > radius * 0.5 {
                    for (i, q) in pts.iter().enumerate() {
                        let d = (*q - c2).length();
                        if d < radius {
                            moved.push((i as u32, falloff(d / radius), true));
                        }
                    }
                }
            }
            let fwd = self.view().basis().0;
            grab = Some(GrabSet { start: pts, moved, at: h.point, normal: fwd });
        }
        drop(f);
        self.selection = vec![h.id];
        self.stroke = Some(Stroke { id: h.id, brush, invert: ctrl, radius, last: h.point, grab, nb, dabs: 0 });
        // the press itself is a dab
        self.stroke_to(p, true);
        true
    }

    /// The pointer moved during a stroke: dabs along the way.
    pub fn stroke_to(&mut self, p: [f32; 2], first: bool) {
        let Some(mut s) = self.stroke.clone() else { return };
        let Some(f) = self.doc.feature(s.id).cloned() else { return };
        let x = matrix(&f);
        let inv = x.inverse();
        let mut pts = world_points(&f);
        let mut changed = vec![false; pts.len()];
        let strength = self.sculpt.strength;
        if let Some(g) = &s.grab {
            // the plane facing the eye through the press
            let (ro, rd) = self.view().ray(p[0], p[1]);
            let den = rd.dot(g.normal);
            if den.abs() < 1e-6 {
                return;
            }
            let t = (g.at - ro).dot(g.normal) / den;
            let now = ro + rd * t;
            let delta = now - g.at;
            // mirrored across the mesh's X (in its frame)
            let ld = inv.transform_vector3(delta);
            let mdelta = x.transform_vector3(Vec3::new(-ld.x, ld.y, ld.z));
            pts.clone_from(&g.start);
            for &(i, w, mirror) in &g.moved {
                pts[i as usize] = g.start[i as usize] + if mirror { mdelta } else { delta } * w;
                changed[i as usize] = true;
            }
        } else {
            let hit = self.pick_mesh(p, Some(MeshKind::Sculpt)).filter(|h| h.id == s.id);
            let Some(h) = hit else {
                // off the clay: the next dab starts where it comes back
                return;
            };
            let m = f.mesh.as_ref().unwrap();
            let n = m.vertex_normals(&pts);
            // dabs a fifth of the radius apart along the way
            let step = (s.radius * 0.2).max(1e-3);
            let dist = (h.point - s.last).length();
            let count = if first { 1 } else { (dist / step).floor() as usize };
            if count == 0 {
                return;
            }
            for k in 1..=count {
                let c = if first { h.point } else { s.last.lerp(h.point, k as f32 / count as f32) };
                dab(s.brush, s.invert, strength, &mut pts, &n, &s.nb, c, s.radius, &mut changed);
                if self.sculpt.mirror {
                    let c2 = mirrored(x, inv, c);
                    if (c2 - c).length() > s.radius * 0.25 {
                        dab(s.brush, s.invert, strength, &mut pts, &n, &s.nb, c2, s.radius, &mut changed);
                    }
                }
            }
            s.last = if first { h.point } else { s.last.lerp(h.point, 1.0) };
        }
        // back into the unit box, in place (the copy let go first, so the
        // mesh is not copied again for each dab)
        let unit = to_unit(&f, &pts);
        drop(f);
        if let Some(ff) = self.doc.feature_mut(s.id) {
            if let Some(mr) = ff.mesh.as_mut() {
                let md = mr.make_mut();
                for (i, c) in changed.iter().enumerate() {
                    if *c {
                        md.verts[i] = unit[i];
                    }
                }
            }
        }
        s.dabs += 1;
        self.stroke = Some(s);
        self.mesh_gen += 1;
    }

    /// The stroke ends: its box fitted to the clay, one undo step.
    pub fn end_stroke(&mut self) {
        let Some(s) = self.stroke.take() else { return };
        if let Some(f) = self.doc.feature_mut(s.id) {
            refit(f);
        }
        self.mesh_gen += 1;
        self.end_live();
    }

    /// The brush ring where the pointer is on the clay: centre, normal,
    /// world radius.
    pub fn brush_ring(&self, p: [f32; 2]) -> Option<(Vec3, Vec3, f32)> {
        let h = self.pick_mesh(p, Some(MeshKind::Sculpt))?;
        let r = self.stroke.as_ref().map(|s| s.radius).unwrap_or_else(|| self.brush_world_radius(h.point));
        Some((h.point, h.normal, r))
    }
}
