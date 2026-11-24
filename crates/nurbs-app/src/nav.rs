//! The navigation island (Romain, 22 nov. 2026, 16 h 49: "La navigation
//! doit pouvoir se faire full stylet"): at the view's top right, under the
//! view island, grips that the tip of a pen (or the mouse) drags with no key
//! and no other button, after Blender's navigation gizmo. The axis ball: a
//! drag turns the view, a tap on an axis looks along it (the same again:
//! from the other side). Zoom: a drag up or right comes closer. Pan: a drag
//! moves the view with the pen. Frame: the selection, else the whole part
//! (F). Settings › Display › Navigation buttons hides it.

use crate::camera::ViewPreset;
use crate::icons::{self, Icon};
use crate::state::AppState;
use crate::theme::*;
use crate::widgets::*;
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Margin, Order, Pos2, Rect, Response, Sense, Stroke, Ui};
use glam::Vec3;

/// The ball's diameter.
const BALL: f32 = 76.0;
/// A grip's size (a pen's target).
const GRIP: f32 = 40.0;
/// An axis dot's radius.
const DOT: f32 = 9.0;

/// Where the island's parts are this frame (the capture harness reads it).
#[derive(Clone, Copy, Debug)]
pub struct NavRects {
    pub ball: Rect,
    pub zoom: Rect,
    pub pan: Rect,
    pub frame: Rect,
}

pub fn rects(ctx: &egui::Context) -> Option<NavRects> {
    ctx.data(|d| d.get_temp::<NavRects>(Id::new("nav-rects")))
}

/// An axis dot on the ball: the axis (0 X, 1 Y, 2 Z), its sign, its place
/// on screen and how far it points towards the eye (−1 to 1).
#[derive(Clone, Copy, Debug)]
pub struct AxisDot {
    pub axis: usize,
    pub sign: f32,
    pub at: Pos2,
    pub depth: f32,
}

/// The six axis dots round the ball's centre `c`, the farthest first.
pub fn axis_dots(st: &AppState, c: Pos2) -> Vec<AxisDot> {
    let (fwd, right, up) = st.view().basis();
    let l = BALL * 0.5 - 8.0;
    let mut v = Vec::with_capacity(6);
    for axis in 0..3 {
        for sign in [1.0f32, -1.0] {
            let d = Vec3::AXES[axis] * sign;
            v.push(AxisDot { axis, sign, at: c + vec2(d.dot(right), -d.dot(up)) * l, depth: -d.dot(fwd) });
        }
    }
    v.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    v
}

/// The dot under `p`, the nearest to the eye first.
fn dot_at(dots: &[AxisDot], p: Pos2) -> Option<AxisDot> {
    dots.iter().rev().find(|d| (d.at - p).length() <= DOT + 2.0).copied()
}

/// The view looking along an axis from its `sign` side.
pub fn preset_of_axis(axis: usize, sign: f32) -> ViewPreset {
    match (axis, sign > 0.0) {
        (0, true) => ViewPreset::Right,
        (0, false) => ViewPreset::Left,
        (1, true) => ViewPreset::Top,
        (1, false) => ViewPreset::Bottom,
        (2, true) => ViewPreset::Front,
        _ => ViewPreset::Back,
    }
}

/// The camera eased to look along an axis; when it already does, from the
/// other side (Blender).
pub fn look_along(st: &mut AppState, axis: usize, sign: f32) {
    let to = st.cam_anim.map(|a| a.to).unwrap_or(st.camera);
    let mut p = preset_of_axis(axis, sign);
    if to.ortho && to.preset_of() == p {
        p = preset_of_axis(axis, -sign);
    }
    let mut c = to;
    c.preset(p);
    st.fly_to(c);
}



pub fn show(ctx: &egui::Context, st: &mut AppState) {
    if !st.settings.nav {
        ctx.data_mut(|d| d.remove::<NavRects>(Id::new("nav-rects")));
        return;
    }
    // left of the rail, level with it
    let top = st.view_rect[1] + 10.0;
    let off = panel_offset(ctx, "nav");
    egui::Area::new(Id::new("nav")).order(Order::Middle).pivot(Align2::RIGHT_TOP).fixed_pos(pos2(st.view_rect[2], top) + off).show(ctx, |ui| {
        island_frame().show(ui, |ui| {
            ui.set_width(BALL);
            grip(ui, "nav");
            ui.spacing_mut().item_spacing.y = 4.0;
            let ball = ball(ui, st);
            ui.add_space(2.0);
            let zoom = drag_grip(ui, "nav-zoom", Icon::Zoom, "Zoom: drag up (or right) to come closer, down to go back");
            if zoom.dragged() {
                let d = moved(ui, &zoom);
                st.stop_flight();
                st.camera.zoom(((d.x - d.y) * 0.005).exp());
                st.interacting = true;
            }
            let pan = drag_grip(ui, "nav-pan", Icon::Pan, "Pan: drag to move the view with the pen");
            if pan.dragged() {
                let d = moved(ui, &pan);
                st.stop_flight();
                st.camera.pan(d.x, d.y, st.screen[1]);
                st.interacting = true;
            }
            let (sep, _) = ui.allocate_exact_size(vec2(BALL, 5.0), Sense::hover());
            ui.painter().line_segment([pos2(sep.min.x + 18.0, sep.center().y), pos2(sep.max.x - 18.0, sep.center().y)], Stroke::new(1.0, DIVIDER()));
            let frame = ui.vertical_centered(|ui| icon_btn(ui, Icon::Frame, false, "Frame the selection, else the whole part (F)", GRIP)).inner;
            if frame.clicked() {
                st.frame_current();
            }
            let r = NavRects { ball, zoom: zoom.rect, pan: pan.rect, frame: frame.rect };
            ui.ctx().data_mut(|d| d.insert_temp(Id::new("nav-rects"), r));
        });
    });
}

/// A grip's drag this frame; nothing on the frame the tip comes down (a
/// pen may come down far from where it was last seen).
fn moved(ui: &Ui, r: &Response) -> egui::Vec2 {
    if ui.input(|i| i.pointer.any_pressed()) {
        egui::Vec2::ZERO
    } else {
        r.drag_delta()
    }
}

fn island_frame() -> egui::Frame {
    island().inner_margin(Margin::same(6))
}

/// The axis ball: a drag turns the view, a tap on an axis looks along it.
fn ball(ui: &mut Ui, st: &mut AppState) -> Rect {
    let (r, resp) = ui.allocate_exact_size(vec2(BALL, BALL), Sense::click_and_drag());
    let c = r.center();
    let dots = axis_dots(st, c);
    let hover = resp.hover_pos().filter(|_| !resp.dragged());
    let hot = hover.and_then(|p| dot_at(&dots, p));
    if resp.dragged() {
        let d = moved(ui, &resp);
        if d != egui::Vec2::ZERO {
            st.stop_flight();
            st.camera.orbit(d.x, d.y);
        }
        st.interacting = true;
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    } else if hot.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }
    if resp.clicked() {
        if let Some(d) = resp.interact_pointer_pos().and_then(|p| dot_at(&dots, p)) {
            look_along(st, d.axis, d.sign);
        }
    }
    let live = resp.hovered() || resp.dragged();
    sphere(ui, st, r, hot, live);
    resp.on_hover_text("Drag: turn the view · Tap an axis: look along it (again: from the other side) · In the view: Alt + drag (or the pen's button held) turns it too, with Shift pans, with Ctrl zooms");
    r
}

/// The view sphere: its great circles and the six sides' names (FRONT +Z,
/// RIGHT +X, TOP +Y…) written on its surface, bent with it (Romain 24 nov.,
/// 9 h 23, after a wireframe brick with its sides named); a tap on a name
/// looks along that axis.
fn sphere(ui: &Ui, st: &AppState, r: Rect, hot: Option<AxisDot>, live: bool) {
    let p = ui.painter();
    let c = r.center();
    let line = pick(Color32::WHITE, TEXT());
    // the sphere as before (24 nov., 6 h 18: "une sphère en white line et
    // semi-opaque remplissage"; 9 h 25: "La sphère actuelle est bien, il
    // faut juste enlever les axes de couleurs"): filled half see-through,
    // its outline a white line, the world's three great circles on it
    // (their far halves fainter); the colour axes gone, the sides' names
    // written on its surface in their place (the tap targets)
    let bg = if live { RAISED() } else { WELL() };
    let rad = BALL * 0.5 - 1.0;
    let (fwd, right, up) = st.view().basis();
    let at = |q: Vec3| c + vec2(q.dot(right), -q.dot(up)) * rad;
    let near = |q: Vec3| -q.dot(fwd);
    p.circle_filled(c, rad, bg.gamma_multiply(if live { 0.75 } else { 0.55 }));
    for axis in 0..3 {
        let (u, v) = (Vec3::AXES[(axis + 1) % 3], Vec3::AXES[(axis + 2) % 3]);
        let n = 48;
        let pt = |k: usize| {
            let t = k as f32 / n as f32 * std::f32::consts::TAU;
            u * t.cos() + v * t.sin()
        };
        for k in 0..n {
            let (a, b) = (pt(k), pt(k + 1));
            let front = (near(a) + near(b)) * 0.5 > 0.0;
            p.line_segment([at(a), at(b)], Stroke::new(1.0, line.gamma_multiply(if front { 0.32 } else { 0.1 })));
        }
    }
    p.circle_stroke(c, rad, Stroke::new(1.4, line.gamma_multiply(if live { 0.95 } else { 0.8 })));
    // the names on the surface
    let names = [(0, 1.0, "RIGHT"), (0, -1.0, "LEFT"), (1, 1.0, "TOP"), (1, -1.0, "BOTTOM"), (2, 1.0, "FRONT"), (2, -1.0, "BACK")];
    let ppp = ui.ctx().pixels_per_point();
    let tex = ui.ctx().fonts(|f| f.font_image_size());
    for (axis, sign, name) in names {
        let a = Vec3::AXES[axis] * sign;
        let facing = near(a);
        if facing < 0.05 {
            continue;
        }
        // its writing directions on the surface: along (left to right as
        // read from outside) and up
        let (t1, t2) = match axis {
            1 => (Vec3::X, Vec3::Z * -sign),
            _ => (Vec3::Y.cross(a), Vec3::Y),
        };
        let on = hot.is_some_and(|h| h.axis == axis && h.sign == sign);
        let col = if on { accent() } else { line };
        let alpha = ((facing - 0.05) / 0.35).clamp(0.0, 1.0) * if on { 1.0 } else { 0.85 };
        let mut job = egui::text::LayoutJob::default();
        job.append(name, 0.0, egui::TextFormat { font_id: medium(7.5), color: col, extra_letter_spacing: 0.8, ..Default::default() });
        let galley = p.layout_job(job);
        let size = galley.size();
        let shape = egui::epaint::TextShape::new(pos2(-size.x * 0.5, -size.y * 0.5), galley, col);
        let mut mesh = egui::epaint::Mesh::default();
        egui::epaint::Tessellator::new(ppp, Default::default(), tex, vec![]).tessellate_text(&shape, &mut mesh);
        // bent onto the sphere: x along t1, y down t2, by arc length
        let k = 0.92;
        for v in &mut mesh.vertices {
            let (u, w) = (v.pos.x / (rad * k), -v.pos.y / (rad * k));
            let q = (a * u.cos() * w.cos() + t1 * u.sin() * w.cos() + t2 * w.sin()).normalize();
            v.pos = at(q);
            v.color = v.color.gamma_multiply(alpha);
        }
        p.add(egui::Shape::mesh(mesh));
        if on {
            p.circle_stroke(at(a), 13.0, Stroke::new(1.0, accent().gamma_multiply(0.6)));
        }
    }
}

/// A round grip dragged with the tip: lit while hovered, in the accent
/// while dragged.
fn drag_grip(ui: &mut Ui, id: &str, icon: Icon, tip: &str) -> Response {
    let resp = ui
        .vertical_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(GRIP, GRIP), Sense::hover());
            let resp = ui.interact(r, Id::new(id), Sense::drag());
            let p = ui.painter();
            let (fill, stroke, col) = if resp.dragged() {
                (accent_bg(), accent_border(), accent_text())
            } else if resp.hovered() {
                (RAISED(), DIVIDER(), TEXT())
            } else {
                (Color32::TRANSPARENT, DIVIDER(), TEXT_ICON())
            };
            p.circle(r.center(), GRIP * 0.5 - 0.5, fill, Stroke::new(1.0, stroke));
            icons::paint(p, Rect::from_center_size(r.center(), vec2(19.0, 19.0)), icon, col);
            if resp.dragged() || resp.hovered() {
                ui.ctx().set_cursor_icon(match icon {
                    Icon::Zoom => CursorIcon::ResizeVertical,
                    _ if resp.dragged() => CursorIcon::Grabbing,
                    _ => CursorIcon::Grab,
                });
            }
            resp
        })
        .inner;
    resp.on_hover_text(tip)
}
