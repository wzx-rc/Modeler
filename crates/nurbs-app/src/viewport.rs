//! The 3D view's input (Maya navigation, picking on the B-rep faces, the
//! move gizmo and the size handles, 3ds Max style creation) and what is
//! drawn over it (gizmo, handles and their values, selection rectangle,
//! the kernel's status).

use crate::state::*;
use crate::theme::*;
use crate::widgets::fmt_mm;
use egui::{pos2, vec2, Align2, Color32, Id, Order, Painter, PointerButton, Pos2, Rect, Sense, Shape, Stroke};
use glam::DVec3;
use nurbs_kernel::client::Status;
use nurbs_kernel::doc::PrimKind;
use nurbs_kernel::{Op, Prim};

fn p2(v: Pos2) -> [f32; 2] {
    [v.x, v.y]
}

/// Length of the gizmo's arrows (points).
const ARROW: f32 = 78.0;

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    st.screen = [screen.width(), screen.height()];
    egui::Area::new(Id::new("viewport")).order(Order::Background).fixed_pos(screen.min).show(ctx, |ui| {
        let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
        handle_input(ctx, st, &resp, rect);
        let p = ui.painter();
        draw_gizmo(p, st);
        draw_creation(p, st);
        draw_marquee(p, st);
        status(p, st);
    });
}

fn world(st: &AppState, w: DVec3) -> Option<Pos2> {
    st.view().project(w.as_vec3()).map(|p| pos2(p[0], p[1]))
}

/// Screen points of the gizmo: pivot, arrow tips.
fn gizmo_points(st: &AppState) -> Option<(Pos2, [Option<Pos2>; 3])> {
    let pivot = st.pivot()?;
    let c = world(st, pivot)?;
    let v = st.view();
    let dist = (pivot.as_vec3() - v.eye).length();
    let k = v.pixel_size_at(dist) as f64 * ARROW as f64;
    let tips = [0, 1, 2].map(|a| world(st, pivot + DVec3::AXES[a] * k));
    Some((c, tips))
}

/// The plane squares: between two arrows, near the pivot.
fn plane_square(c: Pos2, ta: Pos2, tb: Pos2) -> [Pos2; 4] {
    let (da, db) = ((ta - c) * 0.28, (tb - c) * 0.28);
    let (ea, eb) = ((ta - c) * 0.46, (tb - c) * 0.46);
    [c + da + db, c + ea + db, c + ea + eb, c + da + eb]
}

fn in_quad(p: Pos2, q: &[Pos2; 4]) -> bool {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let a = q[i];
        let b = q[(i + 1) % 4];
        let c = (b - a).x * (p - a).y - (b - a).y * (p - a).x;
        if c.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

fn dist_seg(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}

/// The handle under the pointer: a size handle, an arrow, a plane square.
fn handle_at(st: &AppState, p: Pos2) -> Option<Handle> {
    if st.tool != Tool::Select || st.creation.is_some() || st.sel_mode != SelMode::Object {
        return None;
    }
    if let Some(f) = st.single() {
        for h in size_handles(f) {
            if let Some(q) = size_handle_pos(f, h).and_then(|w| world(st, w)) {
                if (q - p).length() < 9.0 {
                    return Some(h);
                }
            }
        }
    }
    let (c, tips) = gizmo_points(st)?;
    let mut best: Option<(f32, Handle)> = None;
    for (a, t) in tips.iter().enumerate() {
        if let Some(t) = t {
            let d = dist_seg(p, c + (*t - c) * 0.2, *t + (*t - c).normalized() * 10.0);
            if d < 8.0 && best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, Handle::Axis(a)));
            }
        }
    }
    if let Some((_, h)) = best {
        return Some(h);
    }
    for (n, (a, b)) in [(2usize, (0usize, 1usize)), (0, (1, 2)), (1, (0, 2))] {
        if let (Some(ta), Some(tb)) = (tips[a], tips[b]) {
            if in_quad(p, &plane_square(c, ta, tb)) {
                return Some(Handle::Plane(n));
            }
        }
    }
    None
}

fn handle_input(ctx: &egui::Context, st: &mut AppState, resp: &egui::Response, rect: Rect) {
    let hover = ctx.input(|i| i.pointer.hover_pos());
    st.interacting = st.drag.is_some() || st.creation.is_some();

    // Maya navigation: Alt + left = orbit, Alt + middle = pan, Alt + right =
    // zoom, whatever the tool
    let (alt, d, l, m, r) = ctx.input(|i| {
        (
            i.modifiers.alt,
            i.pointer.delta(),
            i.pointer.button_down(PointerButton::Primary),
            i.pointer.button_down(PointerButton::Middle),
            i.pointer.button_down(PointerButton::Secondary),
        )
    });
    let in_base = st.creation.as_ref().map(|c| c.phase == Phase::Base).unwrap_or(false);
    if alt && (resp.hovered() || resp.dragged() || l || m || r) && !in_base && st.drag.is_none() {
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
    // the middle button alone pans too (no tool uses it)
    if m && !alt && d != egui::Vec2::ZERO && (resp.hovered() || resp.dragged()) {
        st.camera.pan(d.x, d.y, rect.height());
        st.interacting = true;
        return;
    }

    let (shift, ctrl) = ctx.input(|i| (i.modifiers.shift, i.modifiers.ctrl || i.modifiers.mac_cmd));

    // pre-selection: the feature under the pointer, the handle under it
    let over_ui = !resp.hovered() && !resp.dragged();
    st.hot_handle = if st.drag.is_none() && !over_ui { hover.and_then(|h| handle_at(st, h)) } else { st.drag.as_ref().map(|d| d.handle) };
    let edges = st.tool == Tool::Select && st.sel_mode == SelMode::Edge;
    if st.drag.is_none() && st.creation.is_none() && st.marquee.is_none() && !over_ui {
        st.hover_hit = hover.and_then(|h| st.pick_at(p2(h)));
        st.hover_edge = if edges { hover.and_then(|h| st.pick_edge(p2(h))) } else { None };
        st.hover_feature = if st.hot_handle.is_none() && st.tool == Tool::Select { st.hover_hit.and_then(|h| st.feature_of_face(h.face)) } else { None };
        // edges: only a fillet's faces light up (a click there selects it)
        if edges && (st.hover_edge.is_some() || st.hover_feature.and_then(|id| st.doc.feature(id)).map(|f| f.is_solid()).unwrap_or(true)) {
            st.hover_feature = None;
        }
    } else if over_ui {
        st.hover_hit = None;
        st.hover_edge = None;
        st.hover_feature = None;
    }

    match st.tool {
        Tool::Select if st.sel_mode == SelMode::Edge => {
            if resp.clicked_by(PointerButton::Primary) {
                let at = resp.interact_pointer_pos().or(hover).map(p2);
                let edge = at.and_then(|p| st.pick_edge(p));
                let round = at.and_then(|p| st.pick_at(p)).and_then(|h| st.feature_of_face(h.face)).filter(|id| st.doc.feature(*id).map(|f| !f.is_solid()).unwrap_or(false));
                match (edge, round) {
                    (None, Some(id)) => {
                        st.edge_sel.clear();
                        st.select(Some(id), false);
                    }
                    _ => st.click_edge(edge, shift || ctrl),
                }
            }
        }
        Tool::Select => {
            if resp.drag_started_by(PointerButton::Primary) {
                let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                if let Some(h) = handle_at(st, origin) {
                    st.begin_drag(h, p2(origin));
                } else {
                    st.marquee = Some((p2(origin), p2(origin)));
                }
            }
            if resp.dragged_by(PointerButton::Primary) {
                if let Some(p) = hover.or(resp.interact_pointer_pos()) {
                    if st.drag.is_some() {
                        st.update_drag(p2(p));
                    } else if let Some(mq) = &mut st.marquee {
                        mq.1 = p2(p);
                    }
                }
            }
            if resp.drag_stopped() {
                if st.drag.is_some() {
                    st.end_drag();
                } else if let Some((a, b)) = st.marquee.take() {
                    let r = Rect::from_two_pos(pos2(a[0], a[1]), pos2(b[0], b[1]));
                    let inside: Vec<_> = st.doc.features.iter().filter(|f| f.visible && f.is_solid() && world(st, f.centre()).map(|p| r.contains(p)).unwrap_or(false)).map(|f| f.id).collect();
                    if !(shift || ctrl) {
                        st.selection.clear();
                    }
                    for id in inside {
                        if !st.selection.contains(&id) {
                            st.selection.push(id);
                        }
                    }
                }
            }
            if resp.clicked_by(PointerButton::Primary) {
                let at = resp.interact_pointer_pos().or(hover);
                let id = at.and_then(|p| st.pick_at(p2(p))).and_then(|h| st.feature_of_face(h.face));
                st.select(id, shift || ctrl);
            }
        }
        Tool::Create(PrimKind::Profile) if st.creation.is_none() => {
            // the outline: a click puts a corner, a drag a curve point
            if resp.drag_started_by(PointerButton::Primary) {
                let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                st.sketch_press(p2(origin));
            }
            if resp.dragged_by(PointerButton::Primary) {
                if let Some(p) = hover.or(resp.interact_pointer_pos()) {
                    st.sketch_drag(p2(p));
                }
            }
            if resp.drag_stopped() {
                st.sketch_release();
            }
            if resp.clicked_by(PointerButton::Primary) {
                if let Some(p) = resp.interact_pointer_pos() {
                    st.sketch_press(p2(p));
                    st.sketch_release();
                }
            }
        }
        Tool::Create(kind) => {
            let phase = st.creation.as_ref().map(|c| c.phase);
            match phase {
                None => {
                    if resp.drag_started_by(PointerButton::Primary) {
                        let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                        st.begin_creation(kind, p2(origin));
                    } else if resp.clicked_by(PointerButton::Primary) {
                        // a click without a drag: a 20 mm shape there
                        if let Some(p) = resp.interact_pointer_pos() {
                            st.begin_creation(kind, p2(p));
                            st.base_released(p2(p));
                        }
                    }
                }
                Some(Phase::Base) => {
                    if let Some(p) = hover.or(resp.interact_pointer_pos()) {
                        st.update_creation(p2(p), shift);
                    }
                    if resp.drag_stopped() || !l {
                        let p = hover.or(resp.interact_pointer_pos()).unwrap_or(rect.center());
                        st.base_released(p2(p));
                    }
                }
                Some(Phase::Height) => {
                    if let Some(p) = hover {
                        st.update_creation(p2(p), shift);
                    }
                    if resp.clicked_by(PointerButton::Primary) || resp.drag_started_by(PointerButton::Primary) {
                        st.finish_creation();
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------- drawing

fn chip_bg() -> Color32 {
    pick(Color32::from_rgb(0x14, 0x16, 0x19), Color32::WHITE)
}

/// A value chip (handle values, creation sizes).
pub fn chip(p: &Painter, at: Pos2, anchor: Align2, text: &str, border: Color32, fg: Color32) -> Rect {
    let galley = p.layout_no_wrap(text.to_string(), semibold(12.0), fg);
    let size = galley.size() + vec2(16.0, 8.0);
    let rect = anchor.anchor_size(at, size);
    p.rect(rect, 7.0, tint(chip_bg(), 0.92), Stroke::new(1.0, border), egui::StrokeKind::Inside);
    p.galley(rect.min + vec2(8.0, 4.0), galley, fg);
    rect
}

fn arrow(p: &Painter, a: Pos2, b: Pos2, color: Color32, width: f32) {
    p.line_segment([a, b], Stroke::new(width, color));
    let d = (b - a).normalized();
    let n = vec2(-d.y, d.x);
    let tip = b + d * 11.0;
    p.add(Shape::convex_polygon(vec![tip, b + n * 5.5, b - n * 5.5], color, Stroke::NONE));
}

fn draw_gizmo(p: &Painter, st: &AppState) {
    if st.tool != Tool::Select || st.creation.is_some() || st.selection.is_empty() || st.sel_mode != SelMode::Object {
        return;
    }
    let hot = st.hot_handle;
    let dragging = st.drag.as_ref().map(|d| d.handle);
    let Some((c, tips)) = gizmo_points(st) else { return };
    let colors = [AXIS_X, AXIS_Y, AXIS_Z];
    // plane squares
    for (n, (a, b)) in [(2usize, (0usize, 1usize)), (0, (1, 2)), (1, (0, 2))] {
        if dragging.is_some() && dragging != Some(Handle::Plane(n)) {
            continue;
        }
        if let (Some(ta), Some(tb)) = (tips[a], tips[b]) {
            let q = plane_square(c, ta, tb);
            let on = hot == Some(Handle::Plane(n));
            let col = if on { HOT } else { colors[n] };
            p.add(Shape::convex_polygon(q.to_vec(), tint(col, if on { 0.45 } else { 0.22 }), Stroke::new(1.0, tint(col, 0.8))));
        }
    }
    for (a, t) in tips.iter().enumerate() {
        if dragging.is_some() && dragging != Some(Handle::Axis(a)) && !matches!(dragging, Some(Handle::Plane(n)) if n != a) {
            continue;
        }
        if let Some(t) = t {
            let on = hot == Some(Handle::Axis(a));
            arrow(p, c + (*t - c) * 0.18, *t, if on { HOT } else { colors[a] }, if on { 3.0 } else { 2.2 });
        }
    }
    p.circle(c, 4.0, Color32::WHITE, Stroke::new(1.5, Color32::from_rgb(0x20, 0x22, 0x26)));
    // the move under way
    if let Some(d) = &st.drag {
        if matches!(d.handle, Handle::Axis(_) | Handle::Plane(_)) {
            if let (Some(s), Some(f)) = (d.start.first(), d.start.first().and_then(|s| st.doc.feature(s.id))) {
                let dv = f.origin - s.origin;
                let txt = match d.handle {
                    Handle::Axis(a) => format!("{} {} mm", ["X", "Y", "Z"][a], fmt_mm(dv[a] as f32)),
                    _ => format!("{} × {} × {} mm", fmt_mm(dv.x as f32), fmt_mm(dv.y as f32), fmt_mm(dv.z as f32)),
                };
                chip(p, c + vec2(14.0, 14.0), Align2::LEFT_TOP, &txt, HOT, HOT);
            }
        }
    }
    // size handles of the one selected shape
    let Some(f) = st.single() else { return };
    for h in size_handles(f) {
        let Some(q) = size_handle_pos(f, h).and_then(|w| world(st, w)) else { continue };
        let on = hot == Some(h);
        if dragging.is_some() && dragging != Some(h) {
            continue;
        }
        p.circle(q, if on { 7.0 } else { 5.5 }, if on { HOT } else { Color32::WHITE }, Stroke::new(1.5, Color32::from_rgb(0x20, 0x22, 0x26)));
        if on {
            let (label, v) = match (h, f.prim) {
                (Handle::Size(0), Prim::Box { x, .. } | Prim::Profile { x, .. }) => ("W", x),
                (Handle::Size(2), Prim::Box { z, .. } | Prim::Profile { z, .. }) => ("D", z),
                (Handle::Size(3), Prim::Cylinder { radius, .. }) => ("R", radius),
                (_, pr) => ("H", pr.height()),
            };
            chip(p, q + vec2(12.0, -12.0), Align2::LEFT_BOTTOM, &format!("{label} {} mm", fmt_mm(v as f32)), HOT, HOT);
        }
    }
}

/// The outline being sketched: its pieces as the kernel will build them,
/// the pointer as the next point, the side back to the first point dimmed.
fn draw_sketch(p: &Painter, st: &AppState) {
    let Some(k) = &st.sketch else { return };
    let col = if st.mode == Op::Carve { carve() } else { accent() };
    let hover = p.ctx().input(|i| i.pointer.hover_pos());
    let closing = hover.map(|h| st.sketch_on_first(p2(h))).unwrap_or(false);
    let mut pts = k.pts.clone();
    let mut smooth = k.smooth.clone();
    // the pointer: the next point (unless a point is being dragged or the
    // outline is about to close)
    if let (None, false, Some(q)) = (k.held, closing, hover.and_then(|h| st.sketch_point(p2(h)))) {
        if pts.last().map(|l| (l[0] - q[0]).abs() + (l[1] - q[1]).abs() > 1e-9).unwrap_or(true) {
            pts.push(q);
            smooth.push(false);
        }
    }
    let n = pts.len();
    let screen = |q: [f64; 2]| world(st, k.plane.at(q));
    for piece in nurbs_kernel::doc::pieces(&pts, &smooth) {
        // the side from the last point back to the first one closes it
        let back = piece.id == 2 * (n - 1) + 1;
        if n < 3 && back {
            continue;
        }
        let line: Vec<Pos2> = piece.pts.iter().filter_map(|q| screen(*q)).collect();
        let stroke = if back && !closing { Stroke::new(1.2, tint(col, 0.35)) } else { Stroke::new(2.0, col) };
        p.add(Shape::line(line, stroke));
    }
    // the points: corners square, curve points round
    for (i, q) in k.pts.iter().enumerate() {
        let Some(s) = screen(*q) else { continue };
        let first = i == 0 && k.pts.len() >= 3;
        let r = if first && closing { 6.0 } else { 4.0 };
        let fill = if first && closing { HOT } else { chip_bg() };
        if k.smooth[i] {
            p.circle(s, r, fill, Stroke::new(1.5, col));
        } else {
            p.rect(Rect::from_center_size(s, vec2(r * 2.0, r * 2.0)), 1.0, fill, Stroke::new(1.5, col), egui::StrokeKind::Middle);
        }
    }
    if let Some(h) = hover {
        let on = if k.plane.face.is_some() { "on the face" } else { "on the ground" };
        let txt = if closing { "close".to_string() } else { format!("{} point{} · {on}", k.pts.len(), if k.pts.len() == 1 { "" } else { "s" }) };
        chip(p, h + vec2(16.0, 16.0), Align2::LEFT_TOP, &txt, tint(col, 0.6), if closing { HOT } else { TEXT2() });
    }
}

fn draw_creation(p: &Painter, st: &AppState) {
    if st.sketch.is_some() {
        draw_sketch(p, st);
        return;
    }
    let Some(c) = &st.creation else {
        // the tool armed: a small cross where the shape would start
        if let (Tool::Create(_), Some(h)) = (st.tool, p.ctx().input(|i| i.pointer.hover_pos())) {
            if let Some((plane, q)) = st.base_plane_at(p2(h)) {
                let a = plane.to_plane(q);
                let q = plane.at([st.snap(a[0]), st.snap(a[1])]);
                if let Some(s) = world(st, q) {
                    let col = if st.mode == Op::Carve { carve() } else { accent() };
                    p.line_segment([s - vec2(6.0, 0.0), s + vec2(6.0, 0.0)], Stroke::new(1.5, col));
                    p.line_segment([s - vec2(0.0, 6.0), s + vec2(0.0, 6.0)], Stroke::new(1.5, col));
                    let on = if plane.face.is_some() { "on the face" } else { "on the ground" };
                    chip(p, s + vec2(12.0, 10.0), Align2::LEFT_TOP, on, tint(col, 0.6), TEXT2());
                }
            }
        }
        return;
    };
    let Some(f) = st.doc.feature(c.id) else { return };
    let col = if f.op == Op::Carve { carve() } else { accent() };
    let at = p.ctx().input(|i| i.pointer.hover_pos()).unwrap_or(pos2(0.0, 0.0));
    let txt = match (c.phase, f.prim) {
        (Phase::Base, Prim::Box { x, z, .. } | Prim::Profile { x, z, .. }) => format!("{} × {} mm", fmt_mm(x as f32), fmt_mm(z as f32)),
        (Phase::Base, Prim::Cylinder { radius, .. }) => format!("R {} mm", fmt_mm(radius as f32)),
        (Phase::Height, _) => format!("H {} mm", fmt_mm(c.height as f32)),
    };
    chip(p, at + vec2(16.0, 16.0), Align2::LEFT_TOP, &txt, col, col);
}

fn draw_marquee(p: &Painter, st: &AppState) {
    if let Some((a, b)) = st.marquee {
        let r = Rect::from_two_pos(pos2(a[0], a[1]), pos2(b[0], b[1]));
        p.rect(r, 2.0, tint(accent(), 0.06), Stroke::new(1.0, tint(accent(), 0.7)), egui::StrokeKind::Inside);
    }
}

/// The kernel's line at the bottom left of the view.
fn status(p: &Painter, st: &AppState) {
    let n = st.doc.features.len();
    let n = n - st.doc.features.iter().filter(|f| !f.is_solid()).count();
    let mut parts = vec![format!("NURBS · OpenCascade · {n} shape{}", if n == 1 { "" } else { "s" })];
    let mut color = TEXT();
    match st.kernel.status() {
        Status::Missing => {
            parts.push("kernel not found (forme-nurbs-kernel)".into());
            color = ERROR;
        }
        Status::Broken(e) => {
            parts.push(e);
            color = ERROR;
        }
        Status::Ready => {
            let rounds = st.doc.features.iter().filter(|f| f.visible && !f.is_solid()).count();
            if rounds > 0 {
                parts.push(format!("{rounds} fillet{}", if rounds == 1 { "" } else { "s" }));
            }
            if let Some(m) = &st.model {
                parts.push(format!("{:.0} ms", m.ms));
            }
            if st.building() {
                parts.push("building…".into());
            }
        }
    }
    // just above the history strip
    let at = pos2(st.view_rect[0] + 12.0, st.view_rect[3] - 4.0);
    let r = chip(p, at, Align2::LEFT_BOTTOM, &parts.join(" · "), PANEL_BORDER(), color);
    // what could not be made
    if let Some(m) = &st.model {
        let mut y = r.min.y - 6.0;
        for e in m.errors.iter().take(4) {
            let rr = chip(p, pos2(at.x, y), Align2::LEFT_BOTTOM, e, carve(), carve_light());
            y = rr.min.y - 4.0;
        }
    }
}

/// What the hint line says.
pub fn hint(st: &AppState) -> String {
    match (&st.creation, st.tool) {
        (Some(c), _) if c.phase == Phase::Base => match c.kind {
            PrimKind::Box | PrimKind::Profile => "Drag the base · Shift: a square · release, then the height".into(),
            PrimKind::Cylinder => "Drag the radius · release, then the height".into(),
        },
        (Some(_), _) => "Move for the height (up +, down −) · click to finish · Esc cancels".into(),
        (None, Tool::Create(PrimKind::Profile)) => match &st.sketch {
            None => format!("Sketch · {}: click on a face or the ground for the first point", st.mode.label()),
            Some(k) if k.pts.len() < 3 => "Click: a corner · drag: a curve point · Backspace removes the last one · Esc drops it".into(),
            Some(_) => "Click the first point or press Enter to close · drag: a curve point · Backspace: the last one".into(),
        },
        (None, Tool::Create(k)) => format!("{} · {}: press on the ground or a face and drag · a click places 20 mm · {} again: at once", k.label(), st.mode.label(), crate::ui::prim_key(k)),
        (None, Tool::Select) if st.sel_mode == SelMode::Edge => {
            if let Some(f) = st.selected_round() {
                format!("{} · set its size on the left · Shift + click an edge to add it · Del deletes", f.kind_label())
            } else if st.edge_sel.is_empty() {
                "Edges: click an edge of the part · Shift + click adds · click a fillet to edit it · Q back to objects".into()
            } else {
                "Set the fillet or chamfer on the left · Shift + click adds an edge · Esc deselects".into()
            }
        }
        (None, Tool::Select) => {
            if st.drag.is_some() {
                "Moving · the kernel rebuilds the booleans live".into()
            } else if st.selection.is_empty() {
                if st.doc.features.is_empty() {
                    "Pick a shape on the right (B cube, C cylinder), then drag in the view".into()
                } else {
                    "Click a face to select its shape · drag for a rectangle · Alt + drag to orbit".into()
                }
            } else {
                "Arrows and squares move · dots resize · Del deletes · Esc deselects".into()
            }
        }
    }
}
