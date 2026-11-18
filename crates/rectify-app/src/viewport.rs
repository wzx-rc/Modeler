//! The 3D view's input: Maya navigation (Alt + left orbit, Alt + middle pan,
//! Alt + right zoom, the wheel zooms, the middle button alone pans, the left
//! drag alone orbits when no tool uses it), Space held shows the mesh as it
//! came. The tool in hand takes the left button.

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
    if alt && (resp.hovered() || resp.dragged() || l || m || r) && st.line_drag.is_none() {
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
    // the Line tool: press on an edge, drag along it, release
    if st.tool == Tool::Line && !alt {
        let (pressed, released, pos) = ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.pointer.latest_pos()));
        if pressed && resp.hovered() {
            if let Some(p) = pos {
                st.line_press([p.x, p.y]);
            }
        } else if st.line_drag.is_some() {
            if let Some(p) = pos {
                st.line_move([p.x, p.y]);
            }
            if released || !l {
                st.line_release();
            }
        }
        if st.line_drag.is_some() {
            st.interacting = true;
            return;
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

/// The tool's name at the pointer.
fn tool_cursor(p: &egui::Painter, ctx: &egui::Context, st: &AppState, resp: &egui::Response) {
    let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else { return };
    if !resp.hovered() && st.line_drag.is_none() {
        return;
    }
    let col = accent();
    p.circle_filled(pos, 2.0, col);
    let label = st.tool.label().to_string();
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
        return "Open a mesh to put right".into();
    }
    if st.compare {
        return "The mesh as it came (release Space)".into();
    }
    match st.tool {
        Tool::Line => match &st.line_drag {
            Some(ld) => match &ld.fit {
                Some(Ok(f)) => format!("Release to fix: an edge of {:.0}° · Esc cancels", f.angle),
                Some(Err(e)) => e.clone(),
                None => "Drag along the edge".into(),
            },
            None => "Line: press on an edge and drag along it, the edge becomes straight and sharp · Esc done".into(),
        },
        Tool::Mirror => match st.sym.found {
            Some(f) if f.matched < 0.5 => format!("Mirror {}: only {:.0} % of the mesh has a partner, try another axis", st.sym.axis.label(), f.matched * 100.0),
            Some(_) => format!("Mirror {}: the plane in orange · Enter applies · Esc done", st.sym.axis.label()),
            None => "Mirror: finding the plane".into(),
        },
        Tool::Navigate => "L line · M mirror · Ctrl+Z undo · Space the mesh as it came · drag to turn around".into(),
    }
}
