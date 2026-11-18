//! The 3D view's input: Maya navigation (Alt + left orbit, Alt + middle pan,
//! Alt + right zoom, the wheel zooms, the middle button alone pans), a click
//! picks a region, Space held shows the original. With the brush or the
//! shape tool, the left button paints.

use crate::state::*;
use crate::theme::*;
use egui::{pos2, vec2, Align2, Id, Order, PointerButton, Rect, Sense, Stroke, StrokeKind};

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    st.screen = [screen.width(), screen.height()];
    egui::Area::new(Id::new("viewport")).order(Order::Background).fixed_pos(screen.min).show(ctx, |ui| {
        let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
        handle_input(ctx, st, &resp, rect);
        if st.tool != Tool::Select && !st.bare {
            brush_cursor(ui.painter(), ctx, st, &resp);
        }
        if st.pipe.is_none() && st.job.is_none() {
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
    if alt && (resp.hovered() || resp.dragged() || l || m || r) {
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
    // a tool: the left button paints
    if st.tool != Tool::Select && !alt {
        let (pressed, released, pos) = ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.pointer.latest_pos()));
        if pressed && resp.hovered() {
            if let Some(p) = pos {
                st.stroke_begin([p.x, p.y]);
            }
        } else if st.stroke.is_some() {
            if let Some(p) = pos {
                st.stroke_to([p.x, p.y]);
            }
            if released || !l {
                st.stroke_end();
            }
        }
        if st.stroke.is_some() {
            st.interacting = true;
            return;
        }
    }
    // the middle button alone pans; the left drag alone orbits too (when no
    // tool paints with it)
    if (m || (l && st.tool == Tool::Select)) && !alt && d != egui::Vec2::ZERO && resp.dragged() {
        if m {
            st.camera.pan(d.x, d.y, rect.height());
        } else {
            st.camera.orbit(d.x, d.y);
        }
        st.interacting = true;
        return;
    }
    if resp.clicked() && st.tool == Tool::Select {
        if let Some(p) = resp.interact_pointer_pos() {
            st.click([p.x, p.y]);
        }
    }
}

/// The brush's circle at the pointer, and what it does.
fn brush_cursor(p: &egui::Painter, ctx: &egui::Context, st: &AppState, resp: &egui::Response) {
    let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else { return };
    if !resp.hovered() && st.stroke.is_none() {
        return;
    }
    let col = accent();
    p.circle_stroke(pos, st.brush, Stroke::new(1.5, col));
    p.circle_filled(pos, 1.5, col);
    let label = match st.tool {
        Tool::Brush(m) => m.label().to_string(),
        Tool::Shape(k) => k.label().to_string(),
        Tool::Select => return,
    };
    let at = pos + vec2(st.brush * 0.75 + 6.0, -st.brush * 0.75 - 6.0);
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
        return match j.step {
            Some(_) => format!("{} … {:.0} %", j.what, j.progress * 100.0),
            None => format!("{} …", j.what),
        };
    }
    let Some(p) = &st.pipe else { return "Open a mesh to rectify".into() };
    if st.compare {
        return "Original (release Space to compare)".into();
    }
    match st.tool {
        Tool::Brush(BrushMode::Merge) => return "Merge: paint over the regions to join · [ ] size · Ctrl+Z undo · Esc done".into(),
        Tool::Brush(BrushMode::Split) => return "Split: paint across a region to cut it in two · [ ] size · Ctrl+Z undo".into(),
        Tool::Brush(BrushMode::Free) => return "Free: paint over faces to take them out of their region · [ ] size".into(),
        Tool::Shape(k) => return format!("{}: paint over the surface, the shape fits it and grows · G next kind · Esc done", k.label()),
        Tool::Select => {}
    }
    if st.edited && st.shown() != ViewMode::Result {
        return "Regions edited: the result follows when you show it (3)".into();
    }
    if p.next().is_some() {
        return "Run the steps (R), or one by one from the cards on the right".into();
    }
    if st.selected.is_some() {
        return "Change the region's shape on the left, or free it · Esc to deselect".into();
    }
    "Hold Space to see the original · click a region to inspect it".into()
}
