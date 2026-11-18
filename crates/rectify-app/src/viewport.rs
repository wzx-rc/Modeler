//! The 3D view's input: Maya navigation (Alt + left orbit, Alt + middle pan,
//! Alt + right zoom, the wheel zooms, the middle button alone pans), a click
//! picks a region, Space held shows the original.

use crate::state::*;
use crate::theme::*;
use egui::{pos2, vec2, Align2, Id, Order, PointerButton, Rect, Sense, Stroke, StrokeKind};

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    st.screen = [screen.width(), screen.height()];
    egui::Area::new(Id::new("viewport")).order(Order::Background).fixed_pos(screen.min).show(ctx, |ui| {
        let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
        handle_input(ctx, st, &resp, rect);
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
    // the middle button alone pans; the left drag alone orbits too (nothing
    // else uses it here)
    if (m || l) && !alt && d != egui::Vec2::ZERO && resp.dragged() {
        if m {
            st.camera.pan(d.x, d.y, rect.height());
        } else {
            st.camera.orbit(d.x, d.y);
        }
        st.interacting = true;
        return;
    }
    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            st.click([p.x, p.y]);
        }
    }
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
    if p.next().is_some() {
        return "Run the steps (R), or one by one from the cards on the right".into();
    }
    if st.selected.is_some() {
        return "Change the region's shape on the left, or free it · Esc to deselect".into();
    }
    "Hold Space to see the original · click a region to inspect it".into()
}
