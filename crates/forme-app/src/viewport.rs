//! The 3D viewport: gestures (finger = camera, Pencil/mouse = tools) and the
//! overlays drawn on top of the raymarched image (handles, measures, profile).

use crate::icons::{self, Icon};
use crate::state::*;
use crate::theme::*;
use egui::{pos2, vec2, Align2, Color32, Id, Order, Painter, PointerButton, Pos2, Rect, Sense, Shape, Stroke};
use forme_core::{CombineMode, PrimKind, Profile};

fn p2(v: Pos2) -> [f32; 2] {
    [v.x, v.y]
}

/// Screen positions of the selection handles.
fn handle_points(st: &AppState) -> Vec<(HandleInfo, Pos2)> {
    let view = st.view();
    st.handles()
        .into_iter()
        .filter_map(|h| view.project(h.world).map(|p| (h, pos2(p[0], p[1]))))
        .collect()
}

fn handle_at(st: &AppState, pos: Pos2) -> Option<HandleInfo> {
    if st.tool != Tool::Select {
        return None;
    }
    let mut best: Option<(HandleInfo, f32)> = None;
    for (h, sp) in handle_points(st) {
        let d = sp.distance(pos);
        let reach = if matches!(h.handle, Handle::MoveX | Handle::MoveY | Handle::MoveZ) { 20.0 } else { 24.0 };
        if d < reach && best.as_ref().map(|(_, bd)| d < *bd).unwrap_or(true) {
            best = Some((h, d));
        }
    }
    best.map(|(h, _)| h)
}

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    egui::Area::new(Id::new("viewport")).order(Order::Background).fixed_pos(screen.min).show(ctx, |ui| {
        let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
        handle_input(ctx, st, &resp, rect);
        draw_overlay(ui.painter(), st);
    });
}

fn handle_input(ctx: &egui::Context, st: &mut AppState, resp: &egui::Response, rect: Rect) {
    st.hover = ctx.input(|i| i.pointer.hover_pos()).map(p2);
    st.interacting = false;

    // two fingers: pan + pinch, always the camera
    if let Some(mt) = ctx.multi_touch() {
        if mt.num_touches >= 2 {
            if matches!(st.gesture, Gesture::Place | Gesture::Handle | Gesture::Height) {
                st.cancel_gestures();
            }
            st.gesture = Gesture::Pan;
            st.camera.zoom(mt.zoom_delta);
            st.camera.pan(mt.translation_delta.x, mt.translation_delta.y, rect.height());
            st.view_changed();
            st.interacting = true;
            return;
        }
    }

    if resp.hovered() {
        let (scroll, zoom) = ctx.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        if (zoom - 1.0).abs() > 1e-4 {
            st.camera.zoom(zoom);
            st.view_changed();
        } else if scroll.y.abs() > 0.0 {
            st.camera.zoom((scroll.y * 0.0022).exp());
            st.view_changed();
        }
    }

    if resp.dragged_by(PointerButton::Secondary) {
        let d = resp.drag_delta();
        st.camera.orbit(d.x, d.y);
        st.view_changed();
        st.interacting = true;
    }
    if resp.dragged_by(PointerButton::Middle) {
        let d = resp.drag_delta();
        st.camera.pan(d.x, d.y, rect.height());
        st.view_changed();
        st.interacting = true;
    }

    let finger = st.pointer_kind == PointerKind::Finger;
    let shift = ctx.input(|i| i.modifiers.shift);

    if resp.drag_started_by(PointerButton::Primary) {
        let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
        let awaiting = st.placement.as_ref().map(|p| p.phase == PlacePhase::AwaitHeight).unwrap_or(false);
        st.gesture = if shift {
            Gesture::Pan
        } else if awaiting && !finger {
            st.begin_height(p2(origin));
            Gesture::Height
        } else if let Some(h) = handle_at(st, origin) {
            st.begin_handle(&h, p2(origin));
            Gesture::Handle
        } else {
            match st.tool {
                Tool::Place(_) if !finger => {
                    st.begin_placement(p2(origin));
                    Gesture::Place
                }
                Tool::Profile if !finger => Gesture::ProfileDrag,
                _ => Gesture::Orbit,
            }
        };
    }

    if resp.dragged_by(PointerButton::Primary) {
        let pos = resp.interact_pointer_pos().map(p2);
        st.interacting = true;
        match (st.gesture, pos) {
            (Gesture::Orbit, _) => {
                let d = resp.drag_delta();
                st.camera.orbit(d.x, d.y);
                st.view_changed();
            }
            (Gesture::Pan, _) => {
                let d = resp.drag_delta();
                st.camera.pan(d.x, d.y, rect.height());
                st.view_changed();
            }
            (Gesture::Place | Gesture::Height, Some(p)) => st.drag_placement(p),
            (Gesture::Handle, Some(p)) => st.drag_handle(p),
            _ => {}
        }
    }

    if resp.drag_stopped() {
        match st.gesture {
            Gesture::Place | Gesture::Height => {
                st.release_placement();
            }
            Gesture::Handle => st.end_handle(),
            Gesture::ProfileDrag => {
                if let Some(p) = st.hover.or(resp.interact_pointer_pos().map(p2)) {
                    st.profile_point(p, true);
                }
            }
            _ => {}
        }
        st.gesture = Gesture::None;
        st.view_changed();
    }

    if resp.double_clicked() && st.tool == Tool::Select {
        st.frame_selection();
        return;
    }

    if resp.clicked_by(PointerButton::Primary) {
        let Some(pos) = resp.interact_pointer_pos().map(p2) else { return };
        let awaiting = st.placement.as_ref().map(|p| p.phase == PlacePhase::AwaitHeight).unwrap_or(false);
        if awaiting {
            st.commit_placement();
            return;
        }
        if handle_at(st, pos2(pos[0], pos[1])).is_some() {
            return;
        }
        match st.tool {
            Tool::Select => st.tap_select(pos),
            Tool::Place(_) if finger => st.tap_select(pos),
            Tool::Place(_) => st.tap_place(pos),
            Tool::Profile if !finger => st.profile_point(pos, false),
            Tool::Kit(piece) => st.place_kit(piece, pos),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- overlays

pub fn chip(p: &Painter, at: Pos2, anchor: Align2, text: &str, border: Color32, fg: Color32) -> Rect {
    let galley = p.layout_no_wrap(text.to_string(), semibold(12.0), fg);
    let size = galley.size() + vec2(16.0, 8.0);
    let rect = anchor.anchor_size(at, size);
    p.rect(rect, 7.0, tint(Color32::from_rgb(0x14, 0x16, 0x19), 0.92), Stroke::new(1.0, border), egui::StrokeKind::Inside);
    p.galley(rect.min + vec2(8.0, 4.0), galley, fg);
    rect
}

fn arrow(p: &Painter, a: Pos2, b: Pos2, color: Color32, width: f32) {
    p.line_segment([a, b], Stroke::new(width, color));
    let d = (b - a).normalized();
    let n = vec2(-d.y, d.x);
    let tip = b + d * 9.0;
    p.add(Shape::convex_polygon(vec![tip, b + n * 5.0, b - n * 5.0], color, Stroke::NONE));
}

fn draw_overlay(p: &Painter, st: &AppState) {
    let view = st.view();

    // selection handles
    if st.placement.is_none() && st.profile.is_none() && st.tool == Tool::Select {
        let active = st.handle_drag.as_ref().map(|d| d.handle);
        let node = st.selection.and_then(|id| st.scene.node(id));
        for (h, sp) in handle_points(st) {
            let Some(o) = view.project(h.origin).map(|q| pos2(q[0], q[1])) else { continue };
            let hot = active == Some(h.handle);
            match h.handle {
                Handle::MoveX | Handle::MoveY | Handle::MoveZ => {
                    let c = match h.handle {
                        Handle::MoveX => AXIS_X,
                        Handle::MoveY => AXIS_Y,
                        _ => AXIS_Z,
                    };
                    let c = if active.is_some() && !hot { tint(c, 0.35) } else { c };
                    arrow(p, o, sp, c, if hot { 3.0 } else { 2.0 });
                }
                Handle::Round => {
                    let s = 7.0;
                    p.add(Shape::convex_polygon(
                        vec![sp + vec2(0.0, -s), sp + vec2(s, 0.0), sp + vec2(0.0, s), sp + vec2(-s, 0.0)],
                        ACCENT_LIGHT,
                        Stroke::new(1.5, ON_ACCENT),
                    ));
                }
                _ => {
                    p.line_segment([o, sp], Stroke::new(if hot { 2.5 } else { 1.75 }, ACCENT));
                    p.circle(sp, if hot { 8.5 } else { 7.0 }, ACCENT, Stroke::new(2.0, ON_ACCENT));
                }
            }
            // value chips on the size handles (bevel only while dragging it)
            if h.handle == Handle::Round && !hot {
                continue;
            }
            if let Some(sh) = node.and_then(|n| n.shape()) {
                let text = match (h.handle, sh.kind) {
                    (Handle::Radius, PrimKind::Box) => Some(format!("L {} mm", fmt_mm(sh.size.x * 2.0))),
                    (Handle::Radius, _) => Some(format!("R {} mm", fmt_mm(sh.size.x))),
                    (Handle::Height, PrimKind::Torus) => Some(format!("É {} mm", fmt_mm(sh.size.z))),
                    (Handle::Height, _) => Some(format!("H {} mm", fmt_mm(sh.half_height() * 2.0))),
                    (Handle::Depth, PrimKind::Profile) => Some(format!("Ép {} mm", fmt_mm(sh.size.z * 2.0))),
                    (Handle::Depth, _) => Some(format!("P {} mm", fmt_mm(sh.size.z * 2.0))),
                    (Handle::Round, _) => Some(format!("Bevel {} mm", fmt_mm(sh.round))),
                    _ => None,
                };
                if let Some(t) = text {
                    let dir = (sp - o).normalized();
                    let (at, anchor) = match h.handle {
                        Handle::Radius => (sp + vec2(10.0, 12.0), Align2::LEFT_TOP),
                        Handle::Height => (sp + vec2(-12.0, -12.0), Align2::RIGHT_BOTTOM),
                        Handle::Depth => (sp + vec2(-12.0, 12.0), Align2::RIGHT_TOP),
                        _ => (sp + dir * 14.0, if dir.x >= 0.0 { Align2::LEFT_CENTER } else { Align2::RIGHT_CENTER }),
                    };
                    chip(p, at, anchor, &t, ACCENT_BORDER, ACCENT_TEXT);
                }
            }
        }
    }

    // placement measures
    if let Some(pl) = &st.placement {
        let color = if pl.mode == CombineMode::Add { ACCENT } else { CARVE };
        if let Some(b) = view.project(pl.base).map(|q| pos2(q[0], q[1])) {
            p.line_segment([b - vec2(10.0, 0.0), b + vec2(10.0, 0.0)], Stroke::new(1.5, Color32::WHITE));
            p.line_segment([b - vec2(0.0, 10.0), b + vec2(0.0, 10.0)], Stroke::new(1.5, Color32::WHITE));
            if let Some(snap) = pl.snapped {
                chip(p, b + vec2(26.0, -34.0), Align2::LEFT_BOTTOM, &format!("Snap · {snap}"), Color32::from_gray(0x4A), TEXT);
            }
            let (title, value) = match pl.phase {
                PlacePhase::Base => match pl.kind {
                    PrimKind::Box => ("Base", format!("{} × {} mm", fmt_mm(pl.hx * 2.0), fmt_mm(pl.hz * 2.0))),
                    _ => ("Rayon", format!("{} mm", fmt_mm(pl.radius))),
                },
                _ => (
                    if pl.mode == CombineMode::Add { "Hauteur" } else { "Profondeur" },
                    format!("{} mm", fmt_mm(if pl.kind == PrimKind::Torus { pl.height * 0.5 } else { pl.height })),
                ),
            };
            let at = st.hover.map(|h| pos2(h[0], h[1])).unwrap_or(b) + vec2(-30.0, 26.0);
            let anchor = Align2::RIGHT_TOP;
            let g1 = p.layout_no_wrap(title.to_string(), regular(11.5), TEXT2);
            let g2 = p.layout_no_wrap(value.clone(), semibold(20.0), if pl.mode == CombineMode::Add { ACCENT_TEXT } else { Color32::from_rgb(0xFF, 0xD2, 0xAE) });
            let size = vec2(g1.size().x.max(g2.size().x) + 24.0, g1.size().y + g2.size().y + 18.0);
            let r = anchor.anchor_size(at, size);
            p.rect(r, 10.0, tint(Color32::from_rgb(0x14, 0x16, 0x19), 0.94), Stroke::new(1.5, color), egui::StrokeKind::Inside);
            let gh = g1.size().y;
            p.galley(r.min + vec2(12.0, 8.0), g1, TEXT2);
            p.galley(r.min + vec2(12.0, 10.0 + gh), g2, TEXT);
        }
    }

    // profile draft
    if let Some(d) = &st.profile {
        let to_s = |v: glam::Vec2| view.project(d.to_world(v)).map(|q| pos2(q[0], q[1]));
        if d.points.len() >= 2 {
            let prof = Profile { points: d.points.clone(), mode: d.mode };
            let poly = prof.polyline();
            let pts: Vec<Pos2> = poly.iter().filter_map(|v| to_s(*v)).collect();
            if pts.len() >= 2 {
                // closing preview, faint
                p.add(Shape::dashed_line(&[*pts.last().unwrap(), pts[0]], Stroke::new(1.5, Color32::from_gray(0x8B)), 5.0, 5.0));
                p.add(Shape::line(pts, Stroke::new(2.5, TEXT)));
            }
        }
        if let (Some(last), Some(h)) = (d.points.last().and_then(|pt| to_s(pt.pos)), st.hover) {
            if st.pointer_kind != PointerKind::Finger {
                p.line_segment([last, pos2(h[0], h[1])], Stroke::new(2.5, ACCENT));
            }
        }
        for (i, pt) in d.points.iter().enumerate() {
            let Some(s) = to_s(pt.pos) else { continue };
            if i == 0 && d.points.len() >= 3 {
                p.circle_stroke(s, 12.0, Stroke::new(1.5, ACCENT_LIGHT));
                chip(p, s + vec2(-16.0, -16.0), Align2::RIGHT_BOTTOM, "Fermer ici", ACCENT_BORDER, ACCENT_TEXT);
            }
            if pt.smooth {
                p.circle(s, 5.5, TEXT, Stroke::new(2.0, BG));
            } else {
                p.rect(Rect::from_center_size(s, vec2(11.0, 11.0)), 0.0, TEXT, Stroke::new(2.0, BG), egui::StrokeKind::Middle);
            }
        }
    }

    // axis gizmo (bottom right of the free area)
    let gc = pos2(st.principal()[0] + (st.principal()[0] - 84.0) - 40.0, st.screen[1] - 60.0);
    let (_, right, up) = view.basis();
    p.circle(gc, 27.0, Color32::from_rgb(0x1E, 0x20, 0x23), Stroke::new(1.0, Color32::from_rgb(0x2E, 0x31, 0x36)));
    for (axis, c, l) in [(glam::Vec3::X, AXIS_X, "X"), (glam::Vec3::Y, AXIS_Y, "Y"), (glam::Vec3::Z, AXIS_Z, "Z")] {
        let v = vec2(axis.dot(right), -axis.dot(up)) * 20.0;
        p.line_segment([gc, gc + v], Stroke::new(2.0, c));
        p.text(gc + v * 1.22, Align2::CENTER_CENTER, l, semibold(11.0), c);
    }
    let _ = icons::paint;
    let _ = Icon::Cube;
}
