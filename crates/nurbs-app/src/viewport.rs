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
        draw_vertices(p, st);
        draw_push(p, st);
        draw_knife(p, st, ctx.input(|i| i.pointer.hover_pos()));
        if let (Some(g), Some(h)) = (&st.round_drag, ctx.input(|i| i.pointer.latest_pos())) {
            if g.size > 1e-9 {
                let label = if g.chamfer { "Chamfer" } else { "Fillet" };
                let tension = if !g.chamfer && g.tension > 0.0 { format!(" · tension {:.2}", g.tension) } else { String::new() };
                chip(p, h + vec2(16.0, 16.0), Align2::LEFT_TOP, &format!("{label} {} mm{tension}", fmt_mm(g.size as f32)), HOT, HOT);
            }
        }
        if let (Some(g), Some(h)) = (&st.face_drag, ctx.input(|i| i.pointer.latest_pos())) {
            let txt = match g.what {
                FaceDragWhat::Inset => format!("Inset {} mm", fmt_mm(g.size as f32)),
                FaceDragWhat::Edge { .. } => format!("Pull edge {} mm", fmt_mm(g.size as f32)),
            };
            chip(p, h + vec2(16.0, 16.0), Align2::LEFT_TOP, &txt, HOT, HOT);
        }
        draw_creation(p, st);
        draw_acquired(p, st);
        if let Some(s) = &st.snap_shown {
            draw_snap(p, st, s);
        }
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
    // the selected inset or extrusion: its dot, its arrow
    if let Some(hs) = st.push_handles() {
        if let Some((_, _, k)) = hs.inset {
            if world(st, k).map(|q| (q - p).length() < 10.0).unwrap_or(false) {
                return Some(Handle::PushInset);
            }
        }
        if let (Some(a), Some(b)) = (world(st, hs.base), world(st, hs.tip)) {
            if dist_seg(p, a + (b - a) * 0.25, b + (b - a).normalized() * 10.0) < 8.0 {
                return Some(Handle::PushDepth);
            }
        }
    }
    if st.tool != Tool::Select || st.creation.is_some() {
        return None;
    }
    if let Some(f) = st.single().filter(|_| st.sel_mode == SelMode::Object) {
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
    // Ctrl held: no snapping to the part's points
    st.snap_off = ctrl;
    st.shift_held = shift;
    // the points of interest the pointer passes over: the guides line up
    // with them
    let placing = matches!(st.tool, Tool::Create(_)) || st.drag.as_ref().map(|d| matches!(d.handle, Handle::Axis(_) | Handle::Plane(_) | Handle::Size(_))).unwrap_or(false);
    if let (true, Some(h), true) = (placing, hover, resp.hovered() || resp.dragged()) {
        let mut skip: Vec<nurbs_kernel::FeatureId> = st.drag.as_ref().map(|d| d.start.iter().map(|f| f.id).collect()).unwrap_or_default();
        skip.extend(st.creation.as_ref().map(|c| c.id));
        st.acquire(p2(h), &skip);
    }

    // pre-selection: the feature under the pointer, the handle under it
    let over_ui = !resp.hovered() && !resp.dragged();
    st.hot_handle = if st.drag.is_none() && !over_ui { hover.and_then(|h| handle_at(st, h)) } else { st.drag.as_ref().map(|d| d.handle) };
    let select = st.tool == Tool::Select;
    let knife = matches!(st.tool, Tool::Knife | Tool::Point);
    let edges = (select && st.sel_mode == SelMode::Edge) || knife;
    let faces = (select && st.sel_mode == SelMode::Face) || matches!(st.tool, Tool::Push(_));
    let verts = select && st.sel_mode == SelMode::Vertex;
    if st.drag.is_none() && st.round_drag.is_none() && st.face_drag.is_none() && st.creation.is_none() && st.marquee.is_none() && st.push.is_none() && !over_ui {
        st.hover_hit = hover.and_then(|h| st.pick_at(p2(h)));
        st.hover_edge = if edges { hover.and_then(|h| st.pick_edge(p2(h))) } else { None };
        st.hover_vertex = if verts { hover.and_then(|h| st.pick_vertex(p2(h))) } else { None };
        st.hover_face = if faces { st.hover_hit.map(|h| h.face) } else { None };
        st.knife_hover = if knife { hover.and_then(|h| st.knife_pick(p2(h))).map(|k| k.1) } else { None };
        let by_feature = select && matches!(st.sel_mode, SelMode::Object | SelMode::Edge);
        st.hover_feature = if st.hot_handle.is_none() && by_feature { hover.and_then(|h| st.pick_path(p2(h))).or_else(|| st.hover_hit.and_then(|h| st.feature_of_face(h.face))) } else { None };
        // edges: only a fillet's faces light up (a click there selects it)
        if edges && (st.hover_edge.is_some() || st.hover_feature.and_then(|id| st.doc.feature(id)).map(|f| f.round.is_none()).unwrap_or(true)) {
            st.hover_feature = None;
        }
    } else if over_ui {
        st.hover_hit = None;
        st.hover_edge = None;
        st.hover_feature = None;
        st.hover_face = None;
        st.hover_vertex = None;
    }

    // an inset or extrusion under way, whatever started it
    if st.push.is_some() {
        let held = st.push.as_ref().map(|g| g.held).unwrap_or(false);
        let at = hover.or(resp.interact_pointer_pos()).map(p2);
        if let Some(p) = at {
            st.push_move(p);
        }
        let p = at.unwrap_or(p2(rect.center()));
        if held {
            if resp.drag_stopped() || !l {
                st.push_next(p);
            }
        } else if resp.clicked_by(PointerButton::Primary) || resp.drag_started_by(PointerButton::Primary) {
            st.push_next(p);
        }
        return;
    }

    // Vertex, Edge and Face modes: the selected inset's dot and arrow, the
    // gizmo of the picked component (a drag moves it, Plasticity style)
    if st.tool == Tool::Select && st.sel_mode != SelMode::Object {
        if resp.drag_started_by(PointerButton::Primary) {
            let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
            if let Some(h) = handle_at(st, origin) {
                st.begin_drag(h, p2(origin));
            } else {
                // Rectify: a drag on an edge, a fillet or a sketch corner
                // sizes its fillet or chamfer; elsewhere a rectangle picks
                // the components inside it (Maya)
                if (shift || ctrl) && st.begin_face_drag(p2(origin), shift, ctrl) {
                    // Shift + drag on a picked face extrudes it (the push
                    // gesture above takes it from the next frame), Ctrl
                    // insets it; Shift on a picked edge pulls it out
                    if st.push.is_some() {
                        return;
                    }
                } else if !st.begin_round_drag(p2(origin)) {
                    st.marquee = Some((p2(origin), p2(origin)));
                }
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = hover.or(resp.interact_pointer_pos()) {
                if st.drag.is_some() {
                    st.update_drag(p2(p));
                } else if st.round_drag.is_some() {
                    st.update_round_drag(p2(p), ctrl, shift);
                    st.interacting = true;
                } else if st.face_drag.is_some() {
                    st.update_face_drag(p2(p));
                    st.interacting = true;
                } else if let Some(mq) = &mut st.marquee {
                    mq.1 = p2(p);
                }
            }
        }
        if resp.drag_stopped() {
            if st.drag.is_some() {
                st.end_drag();
            } else if st.round_drag.is_some() {
                st.end_round_drag();
            } else if st.face_drag.is_some() {
                st.end_face_drag();
            } else if let Some((a, b)) = st.marquee.take() {
                st.marquee_components(a, b, shift, ctrl);
            }
        }
    }

    match st.tool {
        Tool::Select if st.sel_mode == SelMode::Edge => {
            if resp.clicked_by(PointerButton::Primary) {
                let at = resp.interact_pointer_pos().or(hover).map(p2);
                let edge = at.and_then(|p| st.pick_edge(p));
                let round = at.and_then(|p| st.pick_at(p)).and_then(|h| st.feature_of_face(h.face)).filter(|id| st.doc.feature(*id).map(|f| f.round.is_some()).unwrap_or(false));
                match (edge, round) {
                    (None, Some(id)) => {
                        st.edge_sel.clear();
                        st.select(Some(id), false);
                    }
                    _ => st.click_edge(edge, shift || ctrl),
                }
            }
        }
        Tool::Select if st.sel_mode == SelMode::Face => {
            if resp.clicked_by(PointerButton::Primary) {
                let at = resp.interact_pointer_pos().or(hover).map(p2);
                st.click_face(at, shift || ctrl);
            }
        }
        Tool::Select if st.sel_mode == SelMode::Vertex => {
            if resp.clicked_by(PointerButton::Primary) {
                let at = resp.interact_pointer_pos().or(hover).map(p2);
                let v = at.and_then(|p| st.pick_vertex(p));
                st.click_vertex(v, shift || ctrl);
            }
        }
        Tool::Knife | Tool::Point => {
            if resp.clicked_by(PointerButton::Primary) {
                if let Some(p) = resp.interact_pointer_pos().or(hover) {
                    st.knife_click(p2(p));
                }
            } else if resp.clicked_by(PointerButton::Secondary) {
                st.knife_end();
            }
        }
        Tool::Push(kind) => {
            if resp.drag_started_by(PointerButton::Primary) {
                let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                st.push_press(kind, p2(origin), true);
                if kind == PushKind::Inset {
                    // pressed and dragged: the new inset's dot follows
                    st.begin_drag(Handle::PushInset, p2(origin));
                }
                if let Some(p) = hover {
                    if st.drag.is_some() {
                        st.update_drag(p2(p));
                    } else {
                        st.push_move(p2(p));
                    }
                }
            } else if resp.clicked_by(PointerButton::Primary) {
                if let Some(p) = resp.interact_pointer_pos() {
                    // a click: a 2 mm inset at once; an extrusion follows the pointer
                    st.push_press(kind, p2(p), false);
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
                let id = at.and_then(|p| st.pick_path(p2(p))).or_else(|| at.and_then(|p| st.pick_at(p2(p))).and_then(|h| st.feature_of_face(h.face)));
                st.select(id, shift || ctrl);
            }
        }
        Tool::Create(PrimKind::Profile | PrimKind::Path) if st.creation.is_none() => {
            // the outline: a click puts a corner, a drag a curve point;
            // Ctrl + drag from a point placed: its fillet
            let grabbed = st.sketch.as_ref().map(|k| k.grab.is_some()).unwrap_or(false);
            if resp.drag_started_by(PointerButton::Primary) {
                let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                if !(ctrl && st.sketch_grab(p2(origin))) {
                    st.sketch_press(p2(origin));
                }
            }
            if resp.dragged_by(PointerButton::Primary) {
                if let Some(p) = hover.or(resp.interact_pointer_pos()) {
                    if st.sketch.as_ref().map(|k| k.grab.is_some()).unwrap_or(false) {
                        st.sketch_fillet_drag(p2(p));
                    } else {
                        st.sketch_drag(p2(p));
                    }
                }
            }
            if resp.drag_stopped() {
                if grabbed {
                    st.sketch_grab_end();
                } else {
                    st.sketch_release();
                }
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
    // shapes selected (Object mode), or a component picked (Vertex, Edge,
    // Face modes): `pivot` says where, if anywhere
    if st.tool != Tool::Select || st.creation.is_some() {
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
                (Handle::Size(0), Prim::Box { x, .. } | Prim::Profile { x, .. } | Prim::Path { x, .. }) => ("W", x),
                (Handle::Size(2), Prim::Box { z, .. } | Prim::Profile { z, .. } | Prim::Path { z, .. }) => ("D", z),
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
    let path = st.tool == Tool::Create(PrimKind::Path);
    let col = if path { PATH } else if st.mode == Op::Carve { carve() } else { accent() };
    let hover = p.ctx().input(|i| i.pointer.hover_pos());
    let closing = hover.map(|h| st.sketch_on_first(p2(h))).unwrap_or(false);
    let ctrl = p.ctx().input(|i| i.modifiers.ctrl || i.modifiers.mac_cmd);
    let grabbing = k.grab.is_some() || ctrl;
    let mut pts = k.pts.clone();
    let mut smooth = k.smooth.clone();
    let mut corner = k.corner.clone();
    corner.resize(pts.len(), 0.0);
    // the pointer: the next point (unless a point is being dragged or the
    // outline is about to close)
    // (Ctrl: no next point, the points placed can be grabbed)
    let next = if k.held.is_none() && !closing && !grabbing { hover.and_then(|h| st.sketch_snap(p2(h))) } else { None };
    if let Some((q, _)) = next {
        if pts.last().map(|l| (l[0] - q[0]).abs() + (l[1] - q[1]).abs() > 1e-9).unwrap_or(true) {
            pts.push(q);
            smooth.push(false);
            corner.push(0.0);
        }
    }
    let n = pts.len();
    let screen = |q: [f64; 2]| world(st, k.plane.at(q));
    let open = path && !closing;
    for piece in nurbs_kernel::doc::pieces_open(&pts, &smooth, &corner, open) {
        // the side from the last point back to the first one closes it
        let back = !open && piece.id == 2 * (n - 1) + 1;
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
    if let Some(s) = next.and_then(|n| n.1) {
        draw_snap(p, st, &s);
    }
    // Ctrl: the point that a drag would round, or the one being rounded
    let ring = k.grab.or_else(|| if ctrl { hover.and_then(|h| st.sketch_point_at(p2(h))) } else { None });
    if let Some(s) = ring.and_then(|i| screen(k.pts[i])) {
        p.circle_stroke(s, 9.0, Stroke::new(1.5, HOT));
    }
    if let (Some(i), Some(h)) = (k.grab, hover) {
        let r = k.corner.get(i).copied().unwrap_or(0.0);
        chip(p, h + vec2(16.0, 16.0), Align2::LEFT_TOP, &format!("Fillet {} mm", fmt_mm(r as f32)), HOT, HOT);
        return;
    }
    if let Some(h) = hover {
        let on = if k.plane.face.is_some() { "on the face" } else { "on the ground" };
        let txt = if closing { "close".to_string() } else if path && k.pts.len() >= 2 { format!("{} points · Enter: an open path", k.pts.len()) } else { format!("{} point{} · {on}", k.pts.len(), if k.pts.len() == 1 { "" } else { "s" }) };
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
            if let Some((plane, a, snapped)) = st.start_plane(p2(h)) {
                let q = plane.at(a);
                if let Some(s) = &snapped {
                    draw_snap(p, st, s);
                }
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
        (Phase::Base, Prim::Box { x, z, .. } | Prim::Profile { x, z, .. } | Prim::Path { x, z }) => format!("{} × {} mm", fmt_mm(x as f32), fmt_mm(z as f32)),
        (Phase::Base, Prim::Cylinder { radius, .. }) => format!("R {} mm", fmt_mm(radius as f32)),
        (Phase::Height, _) => format!("H {} mm", fmt_mm(c.height as f32)),
    };
    chip(p, at + vec2(16.0, 16.0), Align2::LEFT_TOP, &txt, col, col);
}

/// The points the guides line up with (passed over lately): small rings.
fn draw_acquired(p: &Painter, st: &AppState) {
    if !matches!(st.tool, Tool::Create(_)) && st.drag.is_none() {
        return;
    }
    for a in &st.acquired {
        if let Some(q) = world(st, a.pos) {
            p.circle(q, 3.5, tint(HOT, 0.25), Stroke::new(1.2, tint(HOT, 0.9)));
        }
    }
}

/// What the pointer snapped to: a mark shaped by its kind (a square for a
/// corner, a triangle for a middle, a ringed dot for a centre, a diamond for
/// a face centre), dotted lines to the point it came from and along the
/// guides, and a small label.
pub fn draw_snap(p: &Painter, st: &AppState, s: &crate::snap::Snapped) {
    use crate::snap::SnapKind;
    let col = HOT;
    let dark = Color32::from_rgb(0x20, 0x22, 0x26);
    let dotted = |a: DVec3, b: DVec3| {
        if let (Some(sa), Some(sb)) = (world(st, a), world(st, b)) {
            p.extend(Shape::dashed_line(&[sa, sb], Stroke::new(1.3, tint(col, 0.85)), 4.0, 3.0));
        }
    };
    for (a, b) in &s.guides {
        dotted(*a, *b);
        if let Some(q) = world(st, *a) {
            p.circle(q, 3.0, col, Stroke::new(1.0, dark));
        }
    }
    if let Some(src) = s.source {
        dotted(src, s.pos);
        if let Some(q) = world(st, src) {
            p.circle(q, 3.0, col, Stroke::new(1.0, dark));
        }
    }
    let Some(c) = world(st, s.pos) else { return };
    let st1 = Stroke::new(1.5, dark);
    match s.kind {
        SnapKind::End => {
            p.rect(Rect::from_center_size(c, vec2(10.0, 10.0)), 1.0, col, st1, egui::StrokeKind::Middle);
        }
        SnapKind::Mid => {
            p.add(Shape::convex_polygon(vec![c + vec2(0.0, -6.5), c + vec2(6.0, 4.5), c + vec2(-6.0, 4.5)], col, st1));
        }
        SnapKind::Center | SnapKind::Origin => {
            p.circle_stroke(c, 7.0, Stroke::new(2.0, col));
            p.circle(c, 2.5, col, Stroke::NONE);
        }
        SnapKind::FaceCenter => {
            p.add(Shape::convex_polygon(vec![c + vec2(0.0, -6.5), c + vec2(6.5, 0.0), c + vec2(0.0, 6.5), c + vec2(-6.5, 0.0)], col, st1));
        }
        SnapKind::OnEdge | SnapKind::Aligned => {
            p.line_segment([c - vec2(5.0, 5.0), c + vec2(5.0, 5.0)], Stroke::new(2.0, col));
            p.line_segment([c - vec2(5.0, -5.0), c + vec2(5.0, -5.0)], Stroke::new(2.0, col));
        }
    }
    let galley = p.layout_no_wrap(s.kind.label().to_string(), semibold(11.0), dark);
    let r = Align2::LEFT_BOTTOM.anchor_size(c + vec2(10.0, -10.0), galley.size() + vec2(10.0, 4.0));
    p.rect(r, 4.0, col, Stroke::NONE, egui::StrokeKind::Inside);
    p.galley(r.min + vec2(5.0, 2.0), galley, dark);
}

/// Vertex mode: the part's vertices the eye sees, the picked ones and the
/// one under the pointer bigger.
fn draw_vertices(p: &Painter, st: &AppState) {
    if st.tool != Tool::Select || st.sel_mode != SelMode::Vertex {
        return;
    }
    for (q, s) in st.visible_vertices() {
        let s = pos2(s[0], s[1]);
        let picked = st.vert_sel.iter().any(|v| v.as_vec3().distance(q) < 1e-3);
        let hot = st.hover_vertex.map(|v| v.as_vec3().distance(q) < 1e-3).unwrap_or(false);
        if picked {
            p.circle(s, 5.0, accent(), Stroke::new(1.5, chip_bg()));
        } else if hot {
            p.circle(s, 5.0, chip_bg(), Stroke::new(2.0, accent()));
        } else {
            p.circle(s, 3.0, chip_bg(), Stroke::new(1.3, tint(TEXT(), 0.8)));
        }
    }
}

/// The Knife: the point the pointer catches on an edge (a diamond), the
/// line from the cut's last point to it (or to the pointer, dashed).
fn draw_knife(p: &Painter, st: &AppState, pointer: Option<Pos2>) {
    if !matches!(st.tool, Tool::Knife | Tool::Point) {
        return;
    }
    let col = EDGE_SEL;
    let hot = st.knife_hover.and_then(|q| world(st, q));
    if st.tool == Tool::Knife {
        if let (Some(a), Some(b)) = (st.knife_last().and_then(|q| world(st, q)), hot.or(pointer)) {
            if hot.is_some() {
                p.line_segment([a, b], Stroke::new(1.6, col));
            } else {
                p.extend(egui::Shape::dashed_line(&[a, b], Stroke::new(1.2, tint(col, 0.6)), 5.0, 4.0));
            }
        }
    }
    if let Some(a) = st.knife_last().and_then(|q| world(st, q)) {
        p.circle(a, 3.5, col, Stroke::new(1.2, chip_bg()));
    }
    if let Some(c) = hot {
        let r = 5.5;
        let d = [c + vec2(0.0, -r), c + vec2(r, 0.0), c + vec2(0.0, r), c + vec2(-r, 0.0)];
        p.add(egui::Shape::convex_polygon(d.to_vec(), col, Stroke::new(1.2, chip_bg())));
    }
}

/// The extrusion under way: its value by the pointer, the normal it moves
/// along. The selected inset or extrusion: its dot and its arrow.
fn draw_push(p: &Painter, st: &AppState) {
    let col = FACE_SEL;
    if let Some(g) = &st.push {
        if let (Some(a), Some(b)) = (world(st, g.origin), world(st, g.origin + g.n * g.depth)) {
            p.line_segment([a, b], Stroke::new(1.2, tint(col, 0.7)));
            p.circle(a, 3.0, chip_bg(), Stroke::new(1.3, col));
        }
        let at = p.ctx().input(|i| i.pointer.hover_pos()).unwrap_or(pos2(0.0, 0.0));
        chip(p, at + vec2(16.0, 16.0), Align2::LEFT_TOP, &format!("Extrude {} mm", fmt_mm(g.depth as f32)), col, col);
        return;
    }
    let Some(hs) = st.push_handles() else { return };
    let Some(pu) = st.selected_push().and_then(|f| f.push.clone()) else { return };
    let hot = st.hot_handle;
    let dragging = st.drag.as_ref().map(|d| d.handle);
    let lit = |h: Handle| hot == Some(h) || dragging == Some(h);
    if let Some((q, _, k)) = hs.inset {
        if let (Some(a), Some(b)) = (world(st, q), world(st, k)) {
            let on = lit(Handle::PushInset);
            p.line_segment([a, b], Stroke::new(if on { 2.2 } else { 1.6 }, col));
            p.circle(a, 2.5, col, Stroke::NONE);
            p.circle(b, if on { 7.5 } else { 6.0 }, if on { HOT } else { chip_bg() }, Stroke::new(2.0, col));
            if on {
                // on the far side of the dot from its line, clear of the arrow
                let away = (b - a).normalized();
                let at = b + if away.x.is_finite() { away * 16.0 } else { vec2(14.0, 0.0) };
                let anchor = if away.x >= 0.0 { Align2::LEFT_CENTER } else { Align2::RIGHT_CENTER };
                chip(p, at, anchor, &format!("Inset {} mm", fmt_mm(pu.inset as f32)), col, col);
            }
        }
    }
    if let (Some(a), Some(b)) = (world(st, hs.base), world(st, hs.tip)) {
        let on = lit(Handle::PushDepth);
        p.circle(a, 3.0, chip_bg(), Stroke::new(1.3, col));
        arrow(p, a, b, if on { HOT } else { col }, if on { 3.0 } else { 2.2 });
        if on {
            let d = pu.depth;
            let txt = if d.abs() < 1e-9 { "Depth 0 (the line only)".to_string() } else { format!("Depth {}{} mm", if d > 0.0 { "+" } else { "−" }, fmt_mm(d.abs() as f32)) };
            chip(p, b + vec2(14.0, -6.0), Align2::LEFT_BOTTOM, &txt, col, col);
        }
    }
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
            let rounds = st.doc.features.iter().filter(|f| f.visible && f.round.is_some()).count();
            if rounds > 0 {
                parts.push(format!("{rounds} fillet{}", if rounds == 1 { "" } else { "s" }));
            }
            if st.remesh_on {
                if st.remeshing() {
                    parts.push("remeshing…".into());
                }
                match st.remesh.as_ref().map(|r| (&r.result, r.ms)) {
                    Some((Ok((q, rep)), ms)) => parts.push(format!("remesh {} quads · {:.1} mm · {:.1} s", q.quads.len(), rep.quad_size, ms / 1000.0)),
                    Some((Err(e), _)) => {
                        parts.push(format!("remesh failed: {e}"));
                        color = ERROR;
                    }
                    None => {}
                }
            } else if let Some(q) = st.quad_mesh() {
                parts.push(format!("{} quads", q.quads.len()));
            } else if st.quads && !st.building() {
                parts.push("no quads".into());
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
        let mut lines: Vec<String> = m.errors.clone();
        // Quads view: the faces left as triangles, and why
        if let Some(q) = st.quad_mesh() {
            for (f, why) in &q.skipped {
                let who = m.faces.get(*f as usize).and_then(|x| x.tag).and_then(|t| st.doc.feature(t.feature)).map(|x| x.name.clone()).unwrap_or_else(|| "part".into());
                lines.push(format!("{who}: a face left as triangles · {why}"));
            }
        }
        for e in lines.iter().take(4) {
            let rr = chip(p, pos2(at.x, y), Align2::LEFT_BOTTOM, e, carve(), carve_light());
            y = rr.min.y - 4.0;
        }
        if lines.len() > 4 {
            chip(p, pos2(at.x, y), Align2::LEFT_BOTTOM, &format!("and {} more", lines.len() - 4), carve(), carve_light());
        }
    }
}

/// What the help box says: "Key: what it does" parts joined by " · ".
pub fn hint(st: &AppState) -> String {
    if let Some(g) = &st.push {
        return if g.held { "Drag: out adds, in carves · Release: done · Esc: cancel".into() } else { "Move: out adds, in carves · Click: done · Esc: cancel".into() };
    }
    match (&st.creation, st.tool) {
        (Some(c), _) if c.phase == Phase::Base => match c.kind {
            PrimKind::Box | PrimKind::Profile | PrimKind::Path => "Drag: the base · Shift: a square · Release: then the height".into(),
            PrimKind::Cylinder => "Drag: the radius · Release: then the height".into(),
        },
        (Some(_), _) => "Move: the height (up +, down −) · Click: done · Esc: cancel".into(),
        (None, Tool::Create(PrimKind::Path)) => match &st.sketch {
            None => "Click: the first point, on a face or the ground".into(),
            Some(k) if k.pts.len() < 2 => "Click: a corner · Drag: a curve point · Backspace: the last point off · Esc: drop it".into(),
            Some(_) => "Enter: an open path · First point: a closed one · Drag: a curve point · Shift: 0°, 45°, 90° · Ctrl + drag a point: its fillet".into(),
        },
        (None, Tool::Create(PrimKind::Profile)) => match &st.sketch {
            None => "Click: the first point, on a face or the ground".into(),
            Some(k) if k.pts.len() < 3 => "Click: a corner · Drag: a curve point · Backspace: the last point off · Esc: drop it".into(),
            Some(_) => "First point or Enter: close it · Drag: a curve point · Shift: 0°, 45°, 90° · Ctrl + drag a point: its fillet".into(),
        },
        (None, Tool::Create(k)) => format!("Drag: the base, on the ground or a face · Click: 20 mm · {}: one at once · Top bar: Add or Carve", crate::ui::prim_key(k)),
        (None, Tool::Push(k)) => match k {
            PushKind::Inset => "Click a flat face: 2 mm, then drag its dot · Press and drag: sized at once".into(),
            PushKind::Extrude => "Press on a flat face and drag: out adds, in carves".into(),
        },
        (None, Tool::Select) if st.sel_mode != SelMode::Object && st.selected_tweak().is_some() => "Arrows: move it · Window: X, Y, Z · Del: delete it".into(),
        (None, Tool::Knife) => {
            if st.knife.is_some() {
                "Click the next edge: a line across their face · Enter, Esc, right click: end the cut".into()
            } else {
                "Click an edge: the first point (its middle catches) · Then: the next edges".into()
            }
        }
        (None, Tool::Point) => "Click an edge: a point cut into it (its middle catches) · Esc: Select".into(),
        (None, Tool::Select) if st.face_drag.is_some() => "Away from the press: bigger · Release: keep it".into(),
        (None, Tool::Select) if st.round_drag.is_some() => "Away from the press: bigger · Ctrl held: chamfer · Shift held: tension (up fuller, down flatter)".into(),
        (None, Tool::Select) if st.sel_mode == SelMode::Vertex => {
            if st.vert_sel.is_empty() {
                "Click: a vertex · Drag on empty: a rectangle · Drag a sketch corner: its fillet · Shift + click: add".into()
            } else if !st.picked_profile_corners().is_empty() {
                "Drag the corner: its fillet · Ctrl held: chamfer · Shift held: tension · Arrows: move it · Esc: deselect".into()
            } else {
                "Arrows: move it · Shift + click or rectangle: add · Ctrl + rectangle: take away · Esc: deselect".into()
            }
        }
        (None, Tool::Select) if st.sel_mode == SelMode::Face => {
            if let Some(f) = st.selected_push() {
                if f.push.as_ref().map(|p| p.inset > 0.0).unwrap_or(false) {
                    "Dot: the outline · Arrow: out raises, in sinks · Del: delete it".into()
                } else {
                    "Arrow: out adds, in carves · Del: delete it".into()
                }
            } else if st.face_sel.is_empty() {
                "Click: a face · Drag on empty: a rectangle · Shift + click: add · I: inset · E: extrude".into()
            } else {
                "Shift + drag the face: extrude · Ctrl + drag: inset · Arrows: move it · I: inset · E: extrude · Esc: deselect".into()
            }
        }
        (None, Tool::Select) if st.sel_mode == SelMode::Edge => {
            if st.selected_round().is_some() {
                "Drag on it: its size · Shift held: tension · Shift + click an edge: add it · Del: delete it".into()
            } else if st.edge_sel.is_empty() {
                "Click: an edge · Drag on one: a fillet · Ctrl held: chamfer · Shift held: tension · Drag on empty: a rectangle · Ctrl + B: chamfer".into()
            } else {
                "Drag an edge: a fillet · Shift + drag a picked edge: pull it · Arrows: move it · Ctrl + B: chamfer · Esc: deselect".into()
            }
        }
        (None, Tool::Select) => {
            if st.drag.is_some() {
                "Moving: the kernel rebuilds the booleans live".into()
            } else if st.selection.is_empty() {
                if st.doc.features.is_empty() {
                    "B: a cube · C: a cylinder · L: a sketch · Then: drag in the view".into()
                } else {
                    "Click: a shape · Drag: a rectangle · Alt + drag: orbit · F9 F10 F11: vertex, edge, face".into()
                }
            } else {
                "Arrows and squares: move · Dots: resize · Del: delete · Esc: deselect".into()
            }
        }
    }
}
