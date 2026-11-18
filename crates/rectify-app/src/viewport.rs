//! The 3D view's input: Maya navigation (Alt + left orbit, Alt + middle pan,
//! Alt + right zoom, the wheel zooms, the middle button alone pans, the left
//! drag alone orbits when no tool uses it). The tool in hand takes the left
//! button.

use crate::state::*;
use crate::theme::*;
use egui::{pos2, vec2, Align2, Id, Order, PointerButton, Rect, Sense, Stroke, StrokeKind};

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    st.screen = [screen.width(), screen.height()];
    egui::Area::new(Id::new("viewport")).order(Order::Background).fixed_pos(screen.min).show(ctx, |ui| {
        let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
        handle_input(ctx, st, &resp, rect);
        if st.tool != Tool::Navigate && !st.bare {
            tool_cursor(ui.painter(), ctx, st, &resp);
        }
        if st.doc.is_none() && st.job.is_none() {
            empty(ui.painter(), st);
        }
    });
}

fn handle_input(ctx: &egui::Context, st: &mut AppState, resp: &egui::Response, rect: Rect) {
    st.interacting = false;
    let (alt, d, l, m, r) = ctx.input(|i| {
        (
            i.modifiers.alt,
            i.pointer.delta(),
            i.pointer.button_down(PointerButton::Primary),
            i.pointer.button_down(PointerButton::Middle),
            i.pointer.button_down(PointerButton::Secondary),
        )
    });
    if alt && (resp.hovered() || resp.dragged() || l || m || r) && st.slot_drag.is_none() && st.path_press.is_none() {
        if (l || m || r) && d != egui::Vec2::ZERO {
            if l {
                st.camera.orbit(d.x, d.y);
            } else if m {
                st.camera.pan(d.x, d.y, rect.height());
            } else {
                st.camera.zoom(((d.x + d.y) * 0.004).exp());
            }
            st.interacting = true;
        }
        if l || m || r || resp.clicked() || resp.drag_stopped() {
            return;
        }
    }
    if resp.hovered() {
        let (scroll, zoom) = ctx.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        if (zoom - 1.0).abs() > 1e-4 {
            st.camera.zoom(zoom);
        } else if scroll.y.abs() > 0.0 {
            st.camera.zoom((scroll.y * 0.0022).exp());
        }
    }
    // the Circle, Slot and Box tools: press on the rim, at one end or near
    // a corner, drag across or along, release on the other side
    if matches!(st.tool, Tool::Slot | Tool::Box | Tool::Circle) && !alt {
        let (pressed, released, pos) = ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.pointer.latest_pos()));
        if pressed && resp.hovered() {
            if let Some(p) = pos {
                st.slot_press([p.x, p.y]);
            }
        } else if st.slot_drag.is_some() {
            if let Some(p) = pos {
                st.slot_move([p.x, p.y]);
            }
            if released || !l {
                st.slot_release();
            }
        }
        if st.slot_drag.is_some() {
            st.interacting = true;
            return;
        }
    }
    // the Path tool: a click adds a point (on the first one: closes it), a
    // drag moves one; the pointer shows what would catch it (Ctrl: the
    // scan only)
    if st.tool == Tool::Path && !alt {
        let (pressed, released, pos, free) = ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.pointer.latest_pos(), i.modifiers.ctrl || i.modifiers.command));
        if let Some(p) = pos {
            let at = [p.x, p.y];
            if st.path_press.is_some() {
                st.path_drag(at, free);
                if released || !l {
                    st.path_release(at, free);
                }
                st.interacting = st.path_press.as_ref().map(|p| p.moved).unwrap_or(false);
                return;
            } else if pressed && resp.hovered() {
                st.path_press_at(at);
                if released {
                    st.path_release(at, free);
                }
                return;
            } else if resp.hovered() && !m && !r {
                st.path_hover_at(at, free);
            } else if !resp.hovered() {
                st.path_hover_off();
            }
        }
    }
    // the middle button alone pans; the left drag alone orbits when no
    // tool takes it
    if (m || (l && st.tool == Tool::Navigate)) && !alt && d != egui::Vec2::ZERO && resp.dragged() {
        if m {
            st.camera.pan(d.x, d.y, rect.height());
        } else {
            st.camera.orbit(d.x, d.y);
        }
        st.interacting = true;
    }
}

/// The Path's points, and what would catch the pointer.
fn path_marks(p: &egui::Painter, st: &AppState) -> Option<String> {
    use rectify_core::path::SnapKind;
    let v = st.view();
    let scr = st.path_screen();
    let closable = st.path.as_ref().map(|pe| pe.pts.len() >= 3 && !pe.params.closed).unwrap_or(false);
    for (i, s) in scr.iter().enumerate() {
        let c = pos2(s[0], s[1]);
        p.circle_filled(c, 4.5, egui::Color32::from_rgb(255, 118, 24));
        p.circle_stroke(c, 4.5, Stroke::new(1.5, egui::Color32::WHITE));
        if i == 0 && closable {
            p.circle_stroke(c, 8.0, Stroke::new(1.2, tint(accent(), 0.8)));
        }
    }
    let h = st.path_hover.as_ref()?;
    let s = v.project(h.p.as_vec3())?;
    let c = pos2(s[0], s[1]);
    let col = egui::Color32::from_rgb(40, 140, 255);
    match h.kind {
        SnapKind::Corner => {
            p.rect_stroke(Rect::from_center_size(c, vec2(10.0, 10.0)), 1.0, Stroke::new(1.6, col), StrokeKind::Middle);
        }
        SnapKind::Edge => {
            let d = 6.0;
            p.add(egui::Shape::closed_line(vec![c + vec2(0.0, -d), c + vec2(d, 0.0), c + vec2(0.0, d), c + vec2(-d, 0.0)], Stroke::new(1.6, col)));
        }
        SnapKind::Plane => {
            p.circle_stroke(c, 5.0, Stroke::new(1.6, col));
        }
        SnapKind::Scan => {
            p.circle_stroke(c, 4.0, Stroke::new(1.2, TEXT2()));
        }
    }
    Some(match h.kind {
        SnapKind::Corner | SnapKind::Edge => format!("{} {:.0}°", h.kind.label(), h.angle),
        k => k.label().to_string(),
    })
}

/// The tool's name at the pointer.
fn tool_cursor(p: &egui::Painter, ctx: &egui::Context, st: &AppState, resp: &egui::Response) {
    let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else { return };
    let snap = if st.tool == Tool::Path { path_marks(p, st) } else { None };
    if !resp.hovered() && st.slot_drag.is_none() && st.path_press.is_none() {
        return;
    }
    let col = accent();
    p.circle_filled(pos, 2.0, col);
    let label = match snap {
        Some(s) => format!("Path · {s}"),
        None => st.tool.label().to_string(),
    };
    let at = pos + vec2(12.0, -12.0);
    let g = p.layout_no_wrap(label, medium(11.5), on_accent());
    let r = Rect::from_min_size(at - vec2(0.0, g.size().y + 4.0), g.size() + vec2(12.0, 4.0));
    p.rect_filled(r, 5.0, tint(col, 0.92));
    p.galley(r.min + vec2(6.0, 2.0), g, on_accent());
}

/// Nothing open: how to start.
fn empty(p: &egui::Painter, st: &AppState) {
    let c = st.principal();
    let r = Rect::from_center_size(pos2(c[0], c[1]), vec2(420.0, 120.0));
    p.rect(r, 14.0, tint(PANEL(), 0.9), Stroke::new(1.0, PANEL_BORDER()), StrokeKind::Inside);
    p.text(r.center() - vec2(0.0, 22.0), Align2::CENTER_CENTER, "Drop an .obj here", semibold(17.0), TEXT());
    p.text(r.center() + vec2(0.0, 12.0), Align2::CENTER_CENTER, "or open one from Documents/Forme Rectify (top left)", regular(12.5), TEXT2());
}

/// The line under the top bar: what to do now.
pub fn hint(st: &AppState) -> String {
    if let Some(j) = &st.job {
        return format!("{} …", j.what);
    }
    if st.doc.is_none() {
        return "Open a scan (an .obj)".into();
    }
    match st.tool {
        Tool::Navigate => "C circle · S slot · B box · P path · H hide the scan · Ctrl+Z undo · drag to turn around · F frames it".into(),
        Tool::Circle => match (&st.slot_drag, &st.slot) {
            (Some(_), _) => "Across to the other side of the rim, then release".into(),
            (None, Some(se)) => format!("A NURBS surface, Ø {} mm, {} rings: {}, profile and levels on the left, live · Enter keeps it · Esc removes · H hides the scan", fmt(se.slot.width(), 2), se.params.levels + 1, if se.slot.inward { "depth" } else { "height" }),
            (None, None) => "Circle: press on the rim of a hole, a boss, a button or a rivet and drag across it to the other side".into(),
        },
        Tool::Slot => match (&st.slot_drag, &st.slot) {
            (Some(_), _) => "Along the slot to its other end, then release".into(),
            (None, Some(se)) => format!("A NURBS surface, {} × {} mm, {} rows: depth, profile and levels on the left, live · Enter keeps it · Esc removes · H hides the scan", fmt(se.slot.length(), 2), fmt(se.slot.width(), 2), se.params.levels + 1),
            (None, None) => "Slot: press at one end of a slot with round ends and drag along it to the other end".into(),
        },
        Tool::Box => match (&st.slot_drag, &st.slot) {
            (Some(_), _) => "Across the block to its far corner, then release".into(),
            (None, Some(se)) => format!("A NURBS surface, {} × {} mm, {} rows: height, profile and levels on the left, live · Enter keeps it · Esc removes · H hides the scan", fmt(se.slot.length(), 2), fmt(se.slot.width(), 2), se.params.levels + 1),
            (None, None) => "Box: press near a corner of a block with rounded corners (raised, or a pocket) and drag across it to the far corner".into(),
        },
        Tool::Path => match &st.path {
            None => "Path: click on the scan to place points · flat faces, their edges and corners catch the pointer · Ctrl: the scan only".into(),
            Some(pe) if pe.params.closed => "Closed path: drag a point to move it · Enter keeps it · Esc removes · a click elsewhere starts another".into(),
            Some(pe) => format!("{} points · click to add · on the first point closes it · drag a point to move it · Backspace removes the last · Enter keeps it · Esc removes", pe.pts.len()),
        },
    }
}
