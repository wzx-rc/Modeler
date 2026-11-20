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
use nurbs_kernel::doc::{CurveKind, PrimKind};
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
        draw_poly(p, st);
        draw_patch(p, st);
        draw_brush(p, st, ctx.input(|i| i.pointer.hover_pos()), resp.hovered() || resp.dragged());
        draw_push(p, st);
        draw_knife(p, st, ctx.input(|i| i.pointer.hover_pos()));
        draw_poly_cut(p, st, ctx.input(|i| i.pointer.hover_pos()));
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
    crate::marking::draw(ctx, st);
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
    // Q: no gizmo; E and R: rings and scale handles (shapes only)
    match st.gizmo {
        GizmoMode::Select => return None,
        GizmoMode::Rotate if st.sel_mode == SelMode::Object || st.poly_comp() => return ring_at(st, p),
        GizmoMode::Scale => return cage_at(st, p),
        _ => {}
    }
    // (the move gizmo has no size dots: Romain 20 nov., 11 h 44, "enlève
    // les 4 points de scale avec le translate"; R's cage sizes)
    let (c, tips) = gizmo_points(st)?;
    // the centre (Maya): a move on all three axes
    if (p - c).length() < 9.0 {
        return Some(Handle::Free);
    }
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

/// The rotate gizmo's rings: per world axis, its points on screen and
/// whether each faces the eye.
fn rings(st: &AppState) -> Option<[Vec<(Pos2, bool)>; 3]> {
    let pivot = st.pivot()?;
    let v = st.view();
    let k = v.pixel_size_at((pivot.as_vec3() - v.eye).length()) as f64 * ARROW as f64 * 0.9;
    let eye = v.eye.as_dvec3() - pivot;
    Some([0usize, 1, 2].map(|a| {
        let n = DVec3::AXES[a];
        let (u, w) = (DVec3::AXES[(a + 1) % 3], DVec3::AXES[(a + 2) % 3]);
        let _ = n;
        (0..=72)
            .filter_map(|i| {
                let t = i as f64 / 72.0 * std::f64::consts::TAU;
                let q = u * t.cos() + w * t.sin();
                world(st, pivot + q * k).map(|s| (s, q.dot(eye) >= 0.0))
            })
            .collect()
    }))
}

fn ring_at(st: &AppState, p: Pos2) -> Option<Handle> {
    let rs = rings(st)?;
    let mut best: Option<(f32, usize)> = None;
    for (a, pts) in rs.iter().enumerate() {
        for w in pts.windows(2) {
            // the front half first (the back is dimmed)
            let d = dist_seg(p, w[0].0, w[1].0) + if w[0].1 { 0.0 } else { 3.0 };
            if d < 8.0 && best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, a));
            }
        }
    }
    best.map(|(_, a)| Handle::Rot(a))
}

/// The scale cage's grip under the pointer (corners first, then edges,
/// then faces, when they lie as near).
fn cage_at(st: &AppState, p: Pos2) -> Option<Handle> {
    let c = st.cage()?;
    let mut best: Option<(f32, [i8; 3])> = None;
    for s in c.grips() {
        let Some(q) = world(st, c.point(s)) else { continue };
        let n = s.iter().filter(|v| **v != 0).count() as f32;
        let d = (q - p).length() - n * 0.5;
        if d < 9.0 && best.map(|b| d < b.0).unwrap_or(true) {
            best = Some((d, s));
        }
    }
    best.map(|(_, s)| Handle::Cage(s))
}

fn handle_input(ctx: &egui::Context, st: &mut AppState, resp: &egui::Response, rect: Rect) {
    let hover = ctx.input(|i| i.pointer.hover_pos());
    st.interacting = st.drag.is_some() || st.creation.is_some() || st.scan_round.is_some() || st.scan_slot.is_some() || st.slot_draw.is_some();

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
            st.stop_flight();
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
    // the marking menu (right button) takes the pointer while it is open
    if crate::marking::input(ctx, st, rect) {
        return;
    }
    if resp.hovered() {
        let (scroll, zoom) = ctx.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        if (zoom - 1.0).abs() > 1e-4 {
            st.stop_flight();
            st.camera.zoom(zoom);
        } else if scroll.y.abs() > 0.0 {
            st.stop_flight();
            st.camera.zoom((scroll.y * 0.0022).exp());
        }
    }
    // the middle button alone pans too (no tool uses it)
    if m && !alt && d != egui::Vec2::ZERO && (resp.hovered() || resp.dragged()) {
        st.stop_flight();
        st.camera.pan(d.x, d.y, rect.height());
        st.interacting = true;
        return;
    }

    let (shift, ctrl) = ctx.input(|i| (i.modifiers.shift, i.modifiers.ctrl || i.modifiers.mac_cmd));
    // Ctrl held: no snapping to the part's points
    st.snap_off = ctrl;
    st.shift_held = shift;
    st.alt_held = ctx.input(|i| i.modifiers.alt);
    // D held (Maya): a click or a drag puts the gizmo's pivot on the point
    // under the pointer; the moves then carry that point onto the points
    // of interest
    let d_held = !ctrl && !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_down(egui::Key::D));
    let editing = d_held && st.tool == Tool::Select && st.drag.is_none() && st.push.is_none() && st.pivot().is_some();
    if st.pivot_edit && !editing {
        st.snap_shown = None;
    }
    st.pivot_edit = editing;
    if editing {
        let at = hover.or(resp.interact_pointer_pos()).map(p2);
        st.snap_shown = at.and_then(|p| st.pivot_target(p));
        if resp.clicked_by(PointerButton::Primary) || resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = at {
                st.put_pivot(p);
            }
        }
        st.hot_handle = None;
        return;
    }
    // the points of interest the pointer passes over: the guides line up
    // with them
    let placing = matches!(st.tool, Tool::Create(_)) || st.drag.as_ref().map(|d| matches!(d.handle, Handle::Axis(_) | Handle::Plane(_) | Handle::Free | Handle::Size(_))).unwrap_or(false);
    if let (true, Some(h), true) = (placing, hover, resp.hovered() || resp.dragged()) {
        let mut skip: Vec<nurbs_kernel::FeatureId> = st.drag.as_ref().map(|d| d.start.iter().map(|f| f.id).collect()).unwrap_or_default();
        skip.extend(st.creation.as_ref().map(|c| c.id));
        st.acquire(p2(h), &skip);
    }

    // pre-selection: the feature under the pointer, the handle under it
    let over_ui = !resp.hovered() && !resp.dragged();
    st.hot_handle = if st.drag.is_none() && !over_ui { hover.and_then(|h| handle_at(st, h)) } else { st.drag.as_ref().map(|d| d.handle) };
    let select = st.tool == Tool::Select && !st.poly_comp();
    let knife = matches!(st.tool, Tool::Knife | Tool::Point);
    let edges = (select && st.sel_mode == SelMode::Edge) || knife;
    let faces = (select && st.sel_mode == SelMode::Face) || matches!(st.tool, Tool::Push(_));
    let verts = select && st.sel_mode == SelMode::Vertex;
    if st.drag.is_none() && st.round_drag.is_none() && st.face_drag.is_none() && st.creation.is_none() && st.marquee.is_none() && st.push.is_none() && !over_ui {
        st.hover_hit = hover.and_then(|h| st.pick_at(p2(h)));
        st.hover_edge = if edges { hover.and_then(|h| st.pick_edge(p2(h))) } else { None };
        st.hover_vertex = if verts { hover.and_then(|h| st.pick_vertex(p2(h))) } else { None };
        st.hover_face = if faces { st.hover_hit.map(|h| h.face) } else { None };
        // Ctrl: a cut across the face from the edge under the pointer (no point catches)
        let kp = match (knife, st.tool == Tool::Knife && ctrl) {
            (false, _) => None,
            (true, false) => hover.and_then(|h| st.knife_pick(p2(h))),
            (true, true) => hover.and_then(|h| st.knife_pick_edge(p2(h))).map(|(e, q, f)| (e, q, f, false)),
        };
        st.knife_hover = kp.map(|k| k.1);
        st.knife_hover_point = kp.map(|k| k.3).unwrap_or(false);
        st.knife_preview = if st.tool == Tool::Knife && ctrl {
            hover.and_then(|h| st.knife_loop_at(p2(h), shift)).map(|(v, closed)| (v.iter().map(|q| q.at).collect(), closed))
        } else {
            None
        };
        let by_feature = select && matches!(st.sel_mode, SelMode::Object | SelMode::Edge);
        st.hover_feature = if st.hot_handle.is_none() && by_feature {
            if st.sel_mode == SelMode::Object {
                // a mesh in front of the part counts too
                hover.and_then(|h| st.pick_object(p2(h)))
            } else {
                hover.and_then(|h| st.pick_path(p2(h))).or_else(|| st.hover_hit.and_then(|h| st.feature_of_face(h.face)))
            }
        } else {
            None
        };
        // Object mode (Maya): the whole element, its operations' faces too
        if st.sel_mode == SelMode::Object {
            st.hover_feature = st.hover_feature.map(|id| st.element_of(id));
        }
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

    // a curve pulled into a solid (Ctrl + E on a curve, or its window's
    // Extrude): the pointer sets its height, a click keeps it
    if st.curve_pull.is_some() {
        if let Some(p) = hover.or(resp.interact_pointer_pos()).map(p2) {
            st.update_curve_pull(p);
            st.interacting = true;
        }
        if resp.clicked_by(PointerButton::Primary) || resp.drag_stopped() {
            st.finish_curve_pull();
        } else if resp.clicked_by(PointerButton::Secondary) {
            st.abort_gestures();
        }
        return;
    }

    // the Sculpt tab's brush: a press on the clay sculpts it (Ctrl digs,
    // Shift smooths); a click off the clay picks what is there
    if st.tool == Tool::Sculpt {
        if st.stroke.is_some() {
            if let Some(p) = hover.or(resp.interact_pointer_pos()) {
                st.stroke_to(p2(p), false);
            }
            st.interacting = true;
            if resp.drag_stopped() || !l {
                st.end_stroke();
            }
            return;
        }
        if resp.drag_started_by(PointerButton::Primary) {
            let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
            if st.begin_stroke(p2(origin), ctrl, shift) {
                if let Some(p) = hover {
                    st.stroke_to(p2(p), false);
                }
            }
        } else if resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos().or(hover).map(p2) {
                // a click on the clay: one dab; elsewhere: a pick
                if st.begin_stroke(p, ctrl, shift) {
                    st.end_stroke();
                } else {
                    let id = st.pick_object(p).map(|id| st.element_of(id));
                    st.select(id, shift || ctrl);
                }
            }
        }
        return;
    }
    // the Poly tab: Extrude / Inset following the pointer until a click
    if st.poly_pull.is_some() {
        if let Some(p) = hover.or(resp.interact_pointer_pos()) {
            st.update_poly_pull(p2(p));
            st.interacting = true;
        }
        if resp.clicked_by(PointerButton::Primary) || resp.drag_stopped() {
            st.finish_poly_pull();
        } else if resp.clicked_by(PointerButton::Secondary) {
            st.cancel_poly_pull();
        }
        return;
    }
    // the Bevel following the pointer: its travel sets the width, a click
    // keeps it, a right click drops it
    if st.poly_bevel.as_ref().map(|b| b.live).unwrap_or(false) {
        if let Some(p) = hover.or(resp.interact_pointer_pos()) {
            st.update_poly_bevel(p2(p));
            st.interacting = true;
        }
        if resp.clicked_by(PointerButton::Primary) || resp.drag_stopped() {
            st.finish_poly_bevel();
        } else if resp.clicked_by(PointerButton::Secondary) {
            st.cancel_poly_bevel();
        }
        return;
    }
    // the Cut tool (Maya's Multi-Cut): the point a click would put, the
    // line from the cut's last point; Ctrl: the loop it would insert
    if st.tool == Tool::PolyCut {
        st.poly_cut_hover = if over_ui { None } else { hover.and_then(|h| st.poly_cut_at(p2(h))) };
        st.poly_loop = match (ctrl, st.poly_cut_hover) {
            (true, Some(at)) => st.poly_cut_loop_preview(at),
            _ => vec![],
        };
        if resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos().or(hover) {
                st.poly_cut_click(p2(p), ctrl);
                st.poly_cut_hover = st.poly_cut_at(p2(p));
                st.poly_loop.clear();
            }
        } else if resp.clicked_by(PointerButton::Secondary) {
            if st.poly_cut.is_some() {
                st.end_poly_cut();
            } else {
                st.select_tool();
            }
        }
        return;
    }
    // Insert Edge Loop: the loop under the pointer, a click cuts it
    if st.tool == Tool::PolyLoop {
        st.poly_loop = if over_ui { vec![] } else { hover.map(|h| st.poly_loop_preview(p2(h))).unwrap_or_default() };
        if resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos().or(hover) {
                st.poly_loop_at(p2(p));
                st.poly_loop.clear();
            }
        } else if resp.clicked_by(PointerButton::Secondary) {
            st.select_tool();
        }
        return;
    }
    // a linked surface in its NURBS state, Vertex mode: its contour's
    // points (clicks, rectangles, the gizmo on them; a drag on a point moves
    // it at once, in the plane facing the eye)
    if st.patch_comp() {
        if st.patch_drag.is_none() && st.marquee.is_none() {
            st.patch_hover = if over_ui { None } else { hover.and_then(|h| st.patch_hover_at(p2(h))) };
        }
        if resp.drag_started_by(PointerButton::Primary) {
            let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
            if let Some(h) = handle_at(st, origin) {
                st.begin_drag(h, p2(origin));
            } else if let Some(k) = st.patch_hover_at(p2(origin)) {
                if !st.patch_picked().contains(&k) {
                    if let Some(id) = st.linked_target() {
                        st.patch_sel = Some(crate::linked::PatchSel { id, cvs: vec![k] });
                    }
                }
                st.begin_drag(Handle::Free, p2(origin));
            } else {
                st.marquee = Some((p2(origin), p2(origin)));
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = hover.or(resp.interact_pointer_pos()) {
                if st.patch_drag.is_some() {
                    st.update_patch_drag(p2(p));
                    st.interacting = true;
                } else if let Some(mq) = &mut st.marquee {
                    mq.1 = p2(p);
                }
            }
        }
        if resp.drag_stopped() {
            if st.patch_drag.is_some() {
                st.end_patch_drag();
            } else if let Some((a, b)) = st.marquee.take() {
                st.patch_marquee(a, b, shift, ctrl);
            }
        }
        if resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos().or(hover) {
                st.patch_click(p2(p), shift, ctrl);
            }
        }
        return;
    }
    // the Poly tab's Vertex, Edge and Face modes: a Poly mesh's components
    // (clicks, rectangles, the gizmo on them)
    if st.poly_comp() && st.tool == Tool::Select {
        if st.poly_drag.is_none() && st.marquee.is_none() {
            st.poly_hover = if over_ui { None } else { hover.and_then(|h| st.poly_hover_at(p2(h))) };
        }
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
                if st.poly_drag.is_some() {
                    st.update_poly_drag(p2(p));
                    st.interacting = true;
                } else if let Some(mq) = &mut st.marquee {
                    mq.1 = p2(p);
                }
            }
        }
        if resp.drag_stopped() {
            if st.poly_drag.is_some() {
                st.end_poly_drag();
            } else if let Some((a, b)) = st.marquee.take() {
                st.poly_marquee(a, b, shift, ctrl);
            }
        }
        if resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos().or(hover) {
                st.poly_click(p2(p), shift, ctrl);
            }
        }
        return;
    }

    // Vertex, Edge and Face modes: the selected inset's dot and arrow, the
    // gizmo of the picked component (a drag moves it, Plasticity style)
    if st.tool == Tool::Select && st.sel_mode != SelMode::Object {
        if resp.drag_started_by(PointerButton::Primary) {
            let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
            // quick extrude (Romain 20 nov., 11 h 54: "Une Edge ou une Face >
            // Shift + Drag"): Shift on a picked face or edge wins over the
            // move gizmo sitting on it (not over the scale cage, whose Shift
            // keeps the proportions)
            let quick = shift && !ctrl && st.gizmo != GizmoMode::Scale && st.begin_face_drag(p2(origin), true, false);
            if quick {
                if st.push.is_some() {
                    return;
                }
            } else if let Some(h) = handle_at(st, origin) {
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
            // a double-click: the edges linked to it (Maya's edge loop)
            if resp.double_clicked_by(PointerButton::Primary) {
                if let Some(e) = resp.interact_pointer_pos().or(hover).map(p2).and_then(|p| st.pick_edge(p)) {
                    st.pick_linked_edges(e, shift || ctrl);
                }
            }
        }
        Tool::Select if st.sel_mode == SelMode::Face => {
            if resp.clicked_by(PointerButton::Primary) {
                let at = resp.interact_pointer_pos().or(hover).map(p2);
                st.click_face(at, shift || ctrl);
            }
            // a double-click: the faces linked to it (Maya's shell)
            if resp.double_clicked_by(PointerButton::Primary) {
                if let Some(p) = resp.interact_pointer_pos().or(hover).map(p2) {
                    st.pick_linked_faces(p, shift || ctrl);
                }
            }
        }
        Tool::Select if st.sel_mode == SelMode::Vertex => {
            if resp.clicked_by(PointerButton::Primary) {
                let at = resp.interact_pointer_pos().or(hover).map(p2);
                let v = at.and_then(|p| st.pick_vertex(p));
                st.click_vertex(v, shift || ctrl);
            }
            if resp.double_clicked_by(PointerButton::Primary) {
                if let Some(v) = resp.interact_pointer_pos().or(hover).map(p2).and_then(|p| st.pick_vertex(p)) {
                    st.pick_linked_vertices(v, shift || ctrl);
                }
            }
        }
        Tool::Slot => {
            // a drag along a slot of the reference (`scan.rs`): read there
            // when released
            if st.scan_slot.is_some() {
                if let Some(p) = hover.or(resp.interact_pointer_pos()) {
                    st.scan_slot_move(p2(p));
                }
                if resp.drag_stopped() || !l {
                    st.scan_slot_release();
                }
                return;
            }
            // by hand (the default): its length dragged, its width moved,
            // a click, then its height as any shape
            if !st.slot_on_scan {
                if st.creation.is_some() {
                    if let Some(p) = hover {
                        st.update_creation(p2(p), shift);
                    }
                    if resp.clicked_by(PointerButton::Primary) || resp.drag_started_by(PointerButton::Primary) {
                        st.finish_creation();
                    }
                    return;
                }
                let width = st.slot_draw.as_ref().map(|s| s.width);
                match width {
                    Some(false) => {
                        if let Some(p) = hover.or(resp.interact_pointer_pos()) {
                            st.slot_move(p2(p));
                        }
                        if resp.drag_stopped() || !l {
                            let p = hover.or(resp.interact_pointer_pos()).unwrap_or(rect.center());
                            st.slot_release(p2(p));
                        }
                    }
                    Some(true) => {
                        if let Some(p) = hover {
                            st.slot_move(p2(p));
                        }
                        if resp.clicked_by(PointerButton::Primary) || resp.drag_started_by(PointerButton::Primary) {
                            let p = resp.interact_pointer_pos().or(hover).unwrap_or(rect.center());
                            st.slot_press(p2(p));
                        }
                    }
                    None => {
                        if resp.drag_started_by(PointerButton::Primary) {
                            let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                            st.slot_press(p2(origin));
                            if let Some(p) = hover {
                                st.slot_move(p2(p));
                            }
                        } else if resp.clicked_by(PointerButton::Primary) {
                            st.toast("Slot: drag from one end of the slot to the other, then set its width", false);
                        }
                    }
                }
                return;
            }
            if resp.drag_started_by(PointerButton::Primary) {
                let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                if st.scan_slot_press(p2(origin)) {
                    if let Some(p) = hover {
                        st.scan_slot_move(p2(p));
                    }
                }
            } else if resp.clicked_by(PointerButton::Primary) {
                st.toast("Slot on the scan: drag along a slot of the scan, from one end to the other", false);
            }
        }
        Tool::Knife | Tool::Point => {
            if resp.clicked_by(PointerButton::Primary) {
                if let Some(p) = resp.interact_pointer_pos().or(hover) {
                    st.knife_click_with(p2(p), shift, ctrl);
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
                let id = at.and_then(|p| st.pick_object(p2(p)));
                // the element, not the operation whose face was clicked
                let id = id.map(|id| st.element_of(id));
                st.select(id, shift || ctrl);
            }
        }
        Tool::Sculpt | Tool::PolyLoop | Tool::PolyCut => {}
        Tool::Create(PrimKind::Profile | PrimKind::Path | PrimKind::Surface) if st.creation.is_none() => {
            // the outline: a click or a drag puts a corner;
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
                    // Surface in Curve mode: Shift + click on a point placed
                    // breaks the curve there (or no longer)
                    if !(shift && st.sketch_toggle_sharp(p2(p))) {
                        st.sketch_press(p2(p));
                        st.sketch_release();
                    }
                }
            }
        }
        Tool::Curve(kind) => match st.creation.as_ref().map(|c| c.phase) {
            None => {
                if resp.drag_started_by(PointerButton::Primary) {
                    let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                    st.begin_curve(kind, p2(origin));
                } else if resp.clicked_by(PointerButton::Primary) {
                    // a click without a drag: 20 mm across, there
                    if let Some(p) = resp.interact_pointer_pos() {
                        st.begin_curve(kind, p2(p));
                        st.curve_released();
                    }
                }
            }
            Some(_) => {
                if let Some(p) = hover.or(resp.interact_pointer_pos()) {
                    st.update_creation(p2(p), shift);
                }
                if resp.drag_stopped() || !l {
                    st.curve_released();
                }
            }
        },
        Tool::Create(kind) => {
            // the Cylinder dragged across a round part of the reference
            // (`scan.rs`): read there when released
            if st.scan_round.is_some() {
                if let Some(p) = hover.or(resp.interact_pointer_pos()) {
                    st.scan_round_move(p2(p));
                }
                if resp.drag_stopped() || !l {
                    st.scan_round_release();
                }
                return;
            }
            let phase = st.creation.as_ref().map(|c| c.phase);
            match phase {
                None => {
                    if resp.drag_started_by(PointerButton::Primary) {
                        let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                        if kind == PrimKind::Cylinder && st.scan_round_press(p2(origin)) {
                            if let Some(p) = hover {
                                st.scan_round_move(p2(p));
                            }
                            return;
                        }
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
    // a bevel being sized: the pointer sets its width, no handles
    if st.poly_bevel.as_ref().map(|b| b.live).unwrap_or(false) {
        if let (Some(t), Some(h)) = (&st.gizmo_readout, p.ctx().input(|i| i.pointer.hover_pos())) {
            chip(p, h + vec2(16.0, 16.0), Align2::LEFT_TOP, t, HOT, HOT);
        }
        return;
    }
    let hot = st.hot_handle;
    let dragging = st.drag.as_ref().map(|d| d.handle).or(st.poly_drag.as_ref().map(|d| d.handle)).or(st.patch_drag.as_ref().map(|d| d.handle));
    if st.gizmo == GizmoMode::Select {
        return;
    }
    if ((st.sel_mode == SelMode::Object || st.poly_comp()) && st.gizmo == GizmoMode::Rotate) || st.gizmo == GizmoMode::Scale {
        draw_rot_scale(p, st, hot, dragging);
        return;
    }
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
    // the centre: a square (Maya's), a move on all three axes
    {
        let on = hot == Some(Handle::Free) || dragging == Some(Handle::Free);
        let sq = Rect::from_center_size(c, vec2(12.0, 12.0));
        if on {
            p.rect_filled(sq, 1.5, tint(HOT, 0.35));
        }
        p.rect_stroke(sq, 1.5, Stroke::new(if on { 1.6 } else { 1.1 }, if on { HOT } else { Color32::from_gray(0xd8) }), egui::StrokeKind::Middle);
    }
    p.circle(c, 2.5, Color32::WHITE, Stroke::new(1.0, Color32::from_rgb(0x20, 0x22, 0x26)));
    // a pivot put by hand (D): ringed
    if st.custom_pivot_on() {
        p.circle_stroke(c, 10.5, Stroke::new(1.3, HOT));
    }
    // the move under way
    if let Some(d) = &st.drag {
        if matches!(d.handle, Handle::Axis(_) | Handle::Plane(_) | Handle::Free) {
            if let (Some(s), Some(f)) = (d.start.first(), d.start.first().and_then(|s| st.doc.feature(s.id))) {
                // a component move: its step's delta (its origin stays)
                let dv = match (&f.tweak, &s.tweak) {
                    (Some(t), Some(t0)) => t.delta - t0.delta,
                    _ => f.origin - s.origin,
                };
                let txt = match d.handle {
                    Handle::Axis(a) => format!("{} {} mm", ["X", "Y", "Z"][a], fmt_mm(dv[a] as f32)),
                    _ => format!("{} × {} × {} mm", fmt_mm(dv.x as f32), fmt_mm(dv.y as f32), fmt_mm(dv.z as f32)),
                };
                chip(p, c + vec2(14.0, 14.0), Align2::LEFT_TOP, &txt, HOT, HOT);
            }
        }
    }
}

/// E: three rings round the world axes; R: three axes with cubes and a
/// square in the middle (all at once). The value under way by the pivot.
fn draw_rot_scale(p: &Painter, st: &AppState, hot: Option<Handle>, dragging: Option<Handle>) {
    let colors = [AXIS_X, AXIS_Y, AXIS_Z];
    let Some(c) = st.pivot().and_then(|q| world(st, q)) else { return };
    if st.gizmo == GizmoMode::Rotate {
        let Some(rs) = rings(st) else { return };
        for (a, pts) in rs.iter().enumerate() {
            if dragging.is_some() && dragging != Some(Handle::Rot(a)) {
                continue;
            }
            let on = hot == Some(Handle::Rot(a));
            let col = if on { HOT } else { colors[a] };
            for w in pts.windows(2) {
                let front = w[0].1 && w[1].1;
                p.line_segment([w[0].0, w[1].0], Stroke::new(if front { if on { 3.0 } else { 2.2 } } else { 1.2 }, if front { col } else { tint(col, 0.35) }));
            }
        }
    } else {
        // the cage: its edges thin, a grip per face, edge and corner; the
        // grip dragged alone in the accent, the value beside it
        let Some(cg) = st.cage() else { return };
        for (a, b) in cg.edges() {
            if let (Some(a), Some(b)) = (world(st, a), world(st, b)) {
                p.line_segment([a, b], Stroke::new(1.0, Color32::from_white_alpha(150)));
            }
        }
        let dark = Color32::from_rgb(0x20, 0x22, 0x26);
        let mut at_grip = None;
        for s in cg.grips() {
            let h = Handle::Cage(s);
            if dragging.is_some() && dragging != Some(h) {
                continue;
            }
            let Some(q) = world(st, cg.point(s)) else { continue };
            let on = hot == Some(h) || dragging == Some(h);
            let n = s.iter().filter(|v| **v != 0).count();
            let r = match n {
                1 => 4.5,
                2 => 3.0,
                _ => 3.5,
            } + if on { 1.5 } else { 0.0 };
            let fill = if on { HOT } else { Color32::WHITE };
            if n == 2 {
                p.circle(q, r, fill, Stroke::new(1.0, dark));
            } else {
                p.rect(Rect::from_center_size(q, vec2(r * 2.0, r * 2.0)), 1.0, fill, Stroke::new(1.0, dark), egui::StrokeKind::Middle);
            }
            if on {
                at_grip = Some(q);
            }
        }
        if let (Some(q), Some(t)) = (at_grip, &st.gizmo_readout) {
            chip(p, q + vec2(14.0, 14.0), Align2::LEFT_TOP, t, HOT, HOT);
        }
        return;
    }
    p.circle(c, 2.25, Color32::WHITE, Stroke::new(1.0, Color32::from_rgb(0x20, 0x22, 0x26)));
    if let Some(t) = &st.gizmo_readout {
        chip(p, c + vec2(14.0, 14.0), Align2::LEFT_TOP, t, HOT, HOT);
    }
}

/// The outline being sketched: its pieces as the kernel will build them,
/// the pointer as the next point, the side back to the first point dimmed.
fn draw_sketch(p: &Painter, st: &AppState) {
    let Some(k) = &st.sketch else { return };
    if st.tool == Tool::Create(PrimKind::Surface) {
        draw_surface_outline(p, st, k);
        return;
    }
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
        let r = if first && closing { 3.5 } else { 2.5 };
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
        let on = k.plane.label();
        let txt = if closing { "close".to_string() } else if path && k.pts.len() >= 2 { format!("{} points · Enter: an open path", k.pts.len()) } else { format!("{} point{} · {on}", k.pts.len(), if k.pts.len() == 1 { "" } else { "s" }) };
        chip(p, h + vec2(16.0, 16.0), Align2::LEFT_TOP, &txt, tint(col, 0.6), if closing { HOT } else { TEXT2() });
    }
}

/// The Surface tool's outline: straight sides through the points (on the
/// plane, or on the scan "On scan"), the pointer as the next point, the side
/// back to the first point dimmed.
fn draw_surface_outline(p: &Painter, st: &AppState, k: &crate::state::Sketch) {
    let col = PATH;
    let hover = p.ctx().input(|i| i.pointer.hover_pos());
    let closing = hover.map(|h| st.sketch_on_first(p2(h))).unwrap_or(false);
    let placed = k.pts.len();
    let ctrl = p.ctx().input(|i| i.modifiers.ctrl || i.modifiers.mac_cmd);
    let mut next = None;
    if k.held.is_none() && k.grab.is_none() && !ctrl && !closing {
        if let Some(h) = hover {
            next = if st.surface_on_scan { st.reference_at(p2(h)).map(|x| x.0) } else { st.sketch_snap(p2(h)).map(|(q, _)| k.plane.at(q)) };
        }
    }
    // the outline with its fillets, closed once it has 3 points
    let n = placed + next.is_some() as usize;
    let line = st.surface_line(k, next, n >= 3);
    let sl: Vec<Option<Pos2>> = line.iter().map(|q| world(st, *q)).collect();
    let m = sl.len();
    for i in 0..m {
        let j = (i + 1) % m;
        if j == 0 && n < 3 {
            continue;
        }
        let (Some(a), Some(b)) = (sl[i], sl[j]) else { continue };
        let back = j == 0;
        let stroke = if back && !closing { Stroke::new(1.2, tint(col, 0.35)) } else { Stroke::new(2.0, col) };
        p.line_segment([a, b], stroke);
    }
    let sp: Vec<Option<Pos2>> = (0..placed).map(|i| world(st, k.world(i))).collect();
    for (i, s) in sp.iter().enumerate().take(placed) {
        let Some(s) = s else { continue };
        let first = i == 0 && placed >= 3;
        let r = if first && closing { 3.5 } else { 2.5 };
        let fill = if first && closing { HOT } else { chip_bg() };
        // Curve mode: a point the curve goes round is a disc, a sharp one
        // (a V) a square
        let sharp = k.sharp.get(i).copied().unwrap_or(false);
        if st.surface_curve && !sharp {
            p.circle(*s, r + 0.5, fill, Stroke::new(1.5, col));
        } else {
            p.rect(Rect::from_center_size(*s, vec2(r * 2.0, r * 2.0)), 1.0, fill, Stroke::new(1.5, col), egui::StrokeKind::Middle);
        }
    }
    if let Some(h) = hover {
        let on = if st.surface_on_scan { "on the scan" } else { k.plane.label() };
        let txt = if closing { "close: fill it".to_string() } else { format!("{} point{} · {on}", placed, if placed == 1 { "" } else { "s" }) };
        chip(p, h + vec2(16.0, 16.0), Align2::LEFT_TOP, &txt, tint(col, 0.6), if closing { HOT } else { TEXT2() });
    }
}

fn draw_creation(p: &Painter, st: &AppState) {
    // a curve pulled into a solid: its height
    if let Some(cp) = &st.curve_pull {
        let col = if st.doc.feature(cp.id).map(|f| f.op == Op::Carve).unwrap_or(false) { carve() } else { accent() };
        let at = p.ctx().input(|i| i.pointer.hover_pos()).unwrap_or(pos2(0.0, 0.0));
        chip(p, at + vec2(16.0, 16.0), Align2::LEFT_TOP, &format!("H {} mm", fmt_mm(cp.h as f32)), col, col);
        return;
    }
    if st.sketch.is_some() {
        draw_sketch(p, st);
        return;
    }
    // the Slot along a slot of the reference: its length
    if let Some(ss) = &st.scan_slot {
        let at = p.ctx().input(|i| i.pointer.hover_pos()).unwrap_or(pos2(0.0, 0.0));
        chip(p, at + vec2(16.0, 16.0), Align2::LEFT_TOP, &format!("{} mm · release: read the scan", fmt_mm((ss.b - ss.a).length() as f32)), accent(), accent());
        return;
    }
    // the Slot by hand: its length, then its width
    if let Some(sd) = &st.slot_draw {
        let at = p.ctx().input(|i| i.pointer.hover_pos()).unwrap_or(pos2(0.0, 0.0));
        let txt = if sd.width { format!("W {} mm · click: its height", fmt_mm(sd.w as f32)) } else { format!("L {} mm · release: its width", fmt_mm(sd.len() as f32)) };
        chip(p, at + vec2(16.0, 16.0), Align2::LEFT_TOP, &txt, accent(), accent());
        return;
    }
    if st.tool == Tool::Slot && st.slot_on_scan && st.creation.is_none() {
        if let Some(h) = p.ctx().input(|i| i.pointer.hover_pos()) {
            let (txt, col) = if st.scan_under(p2(h)).is_some() { ("on the scan · drag along a slot, end to end", accent()) } else { ("the Slot reads the scan", TEXT2()) };
            p.circle_stroke(h, 7.0, Stroke::new(1.5, col));
            chip(p, h + vec2(12.0, 10.0), Align2::LEFT_TOP, txt, tint(col, 0.6), TEXT2());
        }
        return;
    }
    // the Cylinder across a round part of the reference: its diameter
    if let Some(sr) = &st.scan_round {
        let at = p.ctx().input(|i| i.pointer.hover_pos()).unwrap_or(pos2(0.0, 0.0));
        chip(p, at + vec2(16.0, 16.0), Align2::LEFT_TOP, &format!("Ø {} mm · release: read the scan", fmt_mm(sr.radius() as f32 * 2.0)), accent(), accent());
        return;
    }
    let Some(c) = &st.creation else {
        // the tool armed: a small cross where the shape would start
        if let (Tool::Create(k), Some(h)) = (st.tool, p.ctx().input(|i| i.pointer.hover_pos())) {
            // the Cylinder over the reference: it reads the scan
            if k == PrimKind::Cylinder && st.scan_under(p2(h)).is_some() {
                p.circle_stroke(h, 7.0, Stroke::new(1.5, accent()));
                chip(p, h + vec2(12.0, 10.0), Align2::LEFT_TOP, "on the scan · drag across a round part, rim to rim", tint(accent(), 0.6), TEXT2());
                return;
            }
            if let Some((plane, a, snapped)) = st.start_plane_hover(p2(h)) {
                let q = plane.at(a);
                if let Some(s) = &snapped {
                    draw_snap(p, st, s);
                }
                if let Some(s) = world(st, q) {
                    let col = if st.mode == Op::Carve { carve() } else { accent() };
                    if plane.scan.is_some() {
                        // the scan's plane there: a square on it, about 60
                        // points wide, and its normal, so its tilt shows
                        // (dimmer for a tangent plane)
                        let h = st.mm_per_point(q) * 30.0;
                        let c: Vec<Pos2> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].iter().filter_map(|&(i, j)| world(st, q + plane.u * (i * h) + plane.v * (j * h))).collect();
                        let dim = if plane.scan == Some(true) { 0.9 } else { 0.5 };
                        if c.len() == 4 {
                            p.add(egui::Shape::convex_polygon(c.clone(), tint(col, 0.08), Stroke::NONE));
                            p.add(egui::Shape::closed_line(c.clone(), Stroke::new(3.0, Color32::from_black_alpha(70))));
                            p.add(egui::Shape::closed_line(c, Stroke::new(1.2, tint(col, dim))));
                        }
                        if let Some(t) = world(st, q + plane.n * h) {
                            p.line_segment([s, t], Stroke::new(3.0, Color32::from_black_alpha(70)));
                            p.line_segment([s, t], Stroke::new(1.2, tint(col, dim)));
                        }
                    }
                    p.line_segment([s - vec2(6.0, 0.0), s + vec2(6.0, 0.0)], Stroke::new(1.5, col));
                    p.line_segment([s - vec2(0.0, 6.0), s + vec2(0.0, 6.0)], Stroke::new(1.5, col));
                    let on = plane.label();
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
        // a curve drawn from its centre: its radius
        (_, Prim::Path { x, z }) if f.curve.is_some_and(|k| k.kind.from_centre()) => format!("R {} mm", fmt_mm(f.curve.map(|k| k.radius(x, z)).unwrap_or(0.0) as f32)),
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
            p.circle(q, 2.0, tint(HOT, 0.25), Stroke::new(1.0, tint(HOT, 0.9)));
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
            p.circle(q, 2.0, col, Stroke::new(1.0, dark));
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
            p.circle(s, 2.75, accent(), Stroke::new(1.0, chip_bg()));
        } else if hot {
            p.circle(s, 2.75, chip_bg(), Stroke::new(1.2, accent()));
        } else {
            p.circle(s, 1.75, chip_bg(), Stroke::new(1.0, tint(TEXT(), 0.8)));
        }
    }
}

/// A linked surface in its NURBS state, Vertex mode: its contour's points,
/// a corner a small square, a tangent point a dot; picked ones filled.
fn draw_patch(p: &Painter, st: &AppState) {
    if !st.patch_comp() {
        return;
    }
    let Some((_, cv)) = st.patch_points() else { return };
    let picked = st.patch_picked();
    let a = accent();
    for k in nurbs_kernel::patch::CONTOUR {
        let Some(s) = world(st, cv[k].as_dvec3()) else { continue };
        let on = picked.contains(&k);
        let hot = st.patch_hover == Some(k) && st.patch_drag.is_none();
        let (fill, stroke) = if on {
            (a, Stroke::new(1.0, chip_bg()))
        } else if hot {
            (chip_bg(), Stroke::new(1.6, a))
        } else {
            (chip_bg(), Stroke::new(1.2, tint(TEXT(), 0.9)))
        };
        if nurbs_kernel::patch::is_corner(k) {
            let r = if on || hot { 4.5 } else { 3.6 };
            p.rect(Rect::from_center_size(s, vec2(r * 2.0, r * 2.0)), 1.0, fill, stroke, egui::StrokeKind::Middle);
        } else {
            p.circle(s, if on || hot { 3.6 } else { 2.8 }, fill, stroke);
        }
    }
}

/// The Poly tab's component modes: the target mesh's points (Vertex
/// mode), the edge under the pointer, the picked points.
fn draw_poly(p: &Painter, st: &AppState) {
    if !st.poly_comp() || st.tool != Tool::Select {
        return;
    }
    let Some(id) = st.poly_target().or(match st.poly_hover {
        Some(crate::poly::PolyHover::Vert(id, _) | crate::poly::PolyHover::Edge(id, _) | crate::poly::PolyHover::Face(id, _)) => Some(id),
        None => None,
    }) else {
        return;
    };
    let Some(f) = st.doc.feature(id) else { return };
    let Some(m) = f.mesh.as_ref() else { return };
    let pts = crate::meshes::world_points(f);
    let v = st.view();
    let a = accent();
    // the edge under the pointer
    if let Some(crate::poly::PolyHover::Edge(hid, e)) = st.poly_hover {
        if hid == id && st.poly_drag.is_none() {
            if let (Some(s0), Some(s1)) = (pts.get(e.0 as usize).and_then(|q| world(st, q.as_dvec3())), pts.get(e.1 as usize).and_then(|q| world(st, q.as_dvec3()))) {
                p.line_segment([s0, s1], Stroke::new(2.4, tint(a, 0.75)));
            }
        }
    }
    if st.sel_mode != SelMode::Vertex {
        return;
    }
    // the points facing the eye (their normal), the picked ones lit
    let normals = m.vertex_normals(&pts);
    let picked: Vec<u32> = st.poly_sel.as_ref().filter(|s| s.id == id).map(|s| s.verts.clone()).unwrap_or_default();
    let hot = match st.poly_hover {
        Some(crate::poly::PolyHover::Vert(hid, i)) if hid == id => Some(i),
        _ => None,
    };
    for (i, q) in pts.iter().enumerate() {
        let i = i as u32;
        let facing = normals[i as usize].dot(v.eye - *q) > 0.0;
        let is_picked = picked.contains(&i);
        if !facing && !is_picked {
            continue;
        }
        let Some(sp) = v.project(*q) else { continue };
        let sp = pos2(sp[0], sp[1]);
        if is_picked {
            p.circle(sp, 3.0, a, Stroke::new(1.0, chip_bg()));
        } else if hot == Some(i) {
            p.circle(sp, 3.0, chip_bg(), Stroke::new(1.4, a));
        } else {
            p.circle(sp, 1.9, chip_bg(), Stroke::new(1.0, tint(TEXT(), 0.8)));
        }
    }
}

/// The Cut tool: the point a click would put (a ring on a point, a diamond
/// on an edge), the line from the cut's last point to it (full when it
/// would cut across a face, dashed when it would start again there).
fn draw_poly_cut(p: &Painter, st: &AppState, pointer: Option<Pos2>) {
    if st.tool != Tool::PolyCut {
        return;
    }
    let col = EDGE_SEL;
    let hot = st.poly_cut_hover;
    let at = hot.and_then(|h| st.cut_point(h)).and_then(|q| world(st, q.as_dvec3()));
    if let (Some(a), Some(b)) = (st.poly_cut_last().and_then(|q| world(st, q.as_dvec3())), at.or(pointer)) {
        if hot.map(|h| st.poly_cut_joins(h)).unwrap_or(false) {
            p.line_segment([a, b], Stroke::new(1.8, col));
        } else {
            let n = ((b - a).length() / 6.0).ceil().max(1.0) as usize;
            for i in (0..n).step_by(2) {
                let (t0, t1) = (i as f32 / n as f32, ((i + 1) as f32 / n as f32).min(1.0));
                p.line_segment([a + (b - a) * t0, a + (b - a) * t1], Stroke::new(1.2, tint(col, 0.6)));
            }
        }
        p.circle(a, 3.0, col, Stroke::new(1.0, chip_bg()));
    }
    if let (Some(c), Some(h)) = (at, hot) {
        match h {
            crate::poly::CutAt::Vert(..) => {
                p.circle(c, 4.5, chip_bg(), Stroke::new(1.6, col));
            }
            crate::poly::CutAt::Edge(_, _, _, t) => {
                let r = 4.0;
                let d = [c + vec2(0.0, -r), c + vec2(r, 0.0), c + vec2(0.0, r), c + vec2(-r, 0.0)];
                p.add(egui::Shape::convex_polygon(d.to_vec(), col, Stroke::new(1.2, chip_bg())));
                if (t - 0.5).abs() < 1e-4 {
                    chip(p, c + vec2(10.0, -10.0), Align2::LEFT_BOTTOM, "middle", HOT, HOT);
                }
            }
        }
    }
}

/// The Sculpt tab's brush: a ring of its radius round the pointer, full on
/// the clay, dashed off it (ZBrush's draw size is on screen).
fn draw_brush(p: &Painter, st: &AppState, pointer: Option<Pos2>, in_view: bool) {
    if st.tool != Tool::Sculpt || !in_view {
        return;
    }
    let Some(c) = pointer else { return };
    let on = st.stroke.is_some() || st.pick_mesh(p2(c), Some(nurbs_kernel::mesh::MeshKind::Sculpt)).is_some();
    let a = accent();
    let r = st.sculpt.radius;
    // Clay Buildup: its square, along the stroke on screen
    let brush = st.stroke.as_ref().map(|s| s.brush).unwrap_or(st.sculpt.brush);
    if brush == crate::sculpt::Brush::Buildup {
        let ang = st
            .stroke
            .as_ref()
            .and_then(|s| {
                let v = st.view();
                let (q0, q1) = (v.project(s.last)?, v.project(s.last + s.dir * s.radius)?);
                Some((q1[1] - q0[1]).atan2(q1[0] - q0[0]))
            })
            .unwrap_or(0.0);
        let (cs, sn) = (ang.cos(), ang.sin());
        let k = r * 0.84;
        let sq: Vec<Pos2> = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].iter().map(|(x, y)| c + vec2(x * cs - y * sn, x * sn + y * cs) * k).collect();
        let col = if on { a } else { tint(a, 0.6) };
        p.add(egui::Shape::closed_line(sq, Stroke::new(if on { 1.6 } else { 1.2 }, col)));
        if on {
            p.circle_stroke(c, r * (0.15 + 0.5 * st.sculpt.strength), Stroke::new(1.0, tint(a, 0.55)));
        }
        p.circle_filled(c, 1.6, a);
        return;
    }
    if on {
        p.circle_stroke(c, r, Stroke::new(1.6, a));
        // the strength, as an inner ring
        p.circle_stroke(c, r * (0.15 + 0.5 * st.sculpt.strength), Stroke::new(1.0, tint(a, 0.55)));
    } else {
        let n = 48;
        for i in (0..n).step_by(2) {
            let t0 = i as f32 / n as f32 * std::f32::consts::TAU;
            let t1 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;
            p.line_segment([c + vec2(t0.cos(), t0.sin()) * r, c + vec2(t1.cos(), t1.sin()) * r], Stroke::new(1.2, tint(a, 0.6)));
        }
    }
    p.circle_filled(c, 1.6, a);
}

/// The Knife: the point the pointer catches on an edge (a diamond), the
/// line from the cut's last point to it (or to the pointer, dashed).
fn draw_knife(p: &Painter, st: &AppState, pointer: Option<Pos2>) {
    if !matches!(st.tool, Tool::Knife | Tool::Point) {
        return;
    }
    let col = EDGE_SEL;
    let hot = st.knife_hover.and_then(|q| world(st, q));
    // the loop cut a click would make
    if let Some((pts, closed)) = &st.knife_preview {
        let mut v: Vec<Pos2> = pts.iter().filter_map(|q| world(st, *q)).collect();
        if *closed && v.len() > 2 {
            v.push(v[0]);
        }
        if v.len() >= 2 {
            p.add(egui::Shape::line(v.clone(), Stroke::new(1.6, col)));
            for c in &v {
                p.circle(*c, 1.6, col, Stroke::NONE);
            }
        }
        if let Some(c) = hot {
            let r = 3.5;
            let d = [c + vec2(0.0, -r), c + vec2(r, 0.0), c + vec2(0.0, r), c + vec2(-r, 0.0)];
            p.add(egui::Shape::convex_polygon(d.to_vec(), col, Stroke::new(1.2, chip_bg())));
        }
        return;
    }
    // the cut goes on from its last point (Romain 20 nov., 12 h 09: the
    // faint dashed line read as no preview): a plain line to the pointer,
    // full when it would cut there
    if st.tool == Tool::Knife {
        if let (Some(a), Some(b)) = (st.knife_last().and_then(|q| world(st, q)), hot.or(pointer)) {
            if hot.is_some() {
                p.line_segment([a, b], Stroke::new(1.8, col));
            } else {
                p.line_segment([a, b], Stroke::new(1.5, tint(col, 0.9)));
            }
        }
    }
    if let Some(a) = st.knife_last().and_then(|q| world(st, q)) {
        p.circle(a, 3.0, col, Stroke::new(1.0, chip_bg()));
    }
    // a vertex or an edge's middle caught: a ring round the diamond
    if let (Some(c), true) = (hot, st.knife_hover_point) {
        p.circle_stroke(c, 7.0, Stroke::new(1.4, col));
    }
    if let Some(c) = hot {
        let r = 3.5;
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
            p.circle(a, 2.0, chip_bg(), Stroke::new(1.0, col));
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
            p.circle(a, 1.5, col, Stroke::NONE);
            p.circle(b, if on { 4.0 } else { 3.0 }, if on { HOT } else { chip_bg() }, Stroke::new(1.2, col));
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
        p.circle(a, 2.0, chip_bg(), Stroke::new(1.0, col));
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
    // Claude drives the app through its link (`remote.rs`)
    if st.link.served > 0 {
        parts.push(format!("Claude · {} request{}", st.link.served, if st.link.served == 1 { "" } else { "s" }));
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
    if st.mat_drag.is_some() {
        return if st.sel_mode == SelMode::Face {
            "Release on a face: that face takes it · Off the part: nothing".into()
        } else {
            "Release on a shape: it takes it (its faces' own go) · Face mode (3): one face · Off the part: nothing".into()
        };
    }
    if let Some(m) = &st.marking.menu {
        return match m.ctx {
            crate::marking::Ctx::Shape(_) => "Release on an item: run it · Up: the materials · In the middle: nothing · Esc: close".into(),
            _ => "Release on an item: run it · Quick flick: at once · In the middle: nothing · Esc: close".into(),
        };
    }
    if let Some(g) = &st.push {
        return if g.held { "Drag: out adds, in carves · Release: done · Esc: cancel".into() } else { "Move: out adds, in carves · Click: done · Esc: cancel".into() };
    }
    if let Some(h) = sculpt_poly_hint(st) {
        return h;
    }
    // shapes start on the scan's faces too
    let on_scan = st.reference.is_some() && st.ref_shown;
    match (&st.creation, st.tool) {
        (Some(c), _) if c.curve.is_some() => match c.curve {
            Some(CurveKind::Rectangle) => "Drag: corner to corner · Shift: a square · Release: done".into(),
            Some(CurveKind::Ellipse) => "Drag: corner to corner · Shift: a circle · Release: done".into(),
            _ => "Drag: the radius · Release: done".into(),
        },
        (Some(c), _) if c.phase == Phase::Base => match c.kind {
            PrimKind::Box | PrimKind::Profile | PrimKind::Path | PrimKind::Surface => "Drag: the base · Shift: a square · Release: then the height".into(),
            PrimKind::Cylinder => "Drag: the radius · Release: then the height".into(),
        },
        (Some(_), _) => "Move: the height (up +, down −) · Click: done · Esc: cancel".into(),
        (None, Tool::Sculpt | Tool::PolyLoop | Tool::PolyCut) => String::new(),
        (None, Tool::Create(PrimKind::Path)) => match &st.sketch {
            None if on_scan => "Click: the first point, on a face, the scan or the ground".into(),
            None => "Click: the first point, on a face or the ground".into(),
            Some(k) if k.pts.len() < 2 => "Click or drag: a corner · Ctrl + Z or Backspace: the last point off · Esc: drop it".into(),
            Some(_) => "Enter: an open path · First point: a closed one · Shift: 0°, 45°, 90° · Ctrl + drag a point: its fillet".into(),
        },
        (None, Tool::Create(PrimKind::Surface)) => match &st.sketch {
            None if st.surface_on_scan => "On scan: click the first point on the scan · Tab: On plane / On scan".into(),
            None => "Click: the first point, on a face, the scan's face or the ground (its plane holds the others) · Tab: On plane / On scan".into(),
            Some(k) if k.pts.len() < 3 => "Click or drag: a point · Ctrl + Z or Backspace: the last point off · Esc: drop it".into(),
            Some(_) if st.surface_curve => "First point or Enter: close it, filled · Shift + click a point: sharp / smooth · Ctrl + Z: the last point off · Tab: the next points on the plane / on the scan".into(),
            Some(_) => "First point or Enter: close it, filled with a surface with no thickness · Ctrl + drag a point: its fillet · Ctrl + Z: the last point off · Tab: the next points on the plane / on the scan".into(),
        },
        (None, Tool::Create(PrimKind::Profile)) => match &st.sketch {
            None if on_scan => "Click: the first point, on a face, the scan or the ground".into(),
            None => "Click: the first point, on a face or the ground".into(),
            Some(k) if k.pts.len() < 3 => "Click or drag: a corner · Ctrl + Z or Backspace: the last point off · Esc: drop it".into(),
            Some(_) => "First point or Enter: close it · Shift: 0°, 45°, 90° · Ctrl + drag a point: its fillet".into(),
        },
        (None, Tool::Slot) if !st.slot_on_scan => match &st.slot_draw {
            Some(s) if s.width => "Move: its width · Click: on to its height · Esc: cancel".into(),
            Some(_) => "To its other end · Shift: 0°, 45°, 90° · Release: then its width".into(),
            None => "Drag from one end of the slot to the other on a face or the ground, then set its width and its height · ON SCAN (options): read a slot of the scan".into(),
        },
        (None, Tool::Slot) if st.scan_slot.is_some() => "Along the slot, to its other end · Release: the scan read there, a sketch made as read · Esc: cancel".into(),
        (None, Tool::Slot) => "Drag along a slot of the scan (or a raised pill), from one end to the other · Release: read there, a Carve sketch (cut in) or an Add (raised) with round ends".into(),
        (None, Tool::Create(PrimKind::Cylinder)) if st.scan_round.is_some() => "Across the round part, to the other side of its rim · Release: the scan read there, a cylinder made as read · Esc: cancel".into(),
        (None, Tool::Create(PrimKind::Cylinder)) if st.reference.is_some() && st.ref_shown => {
            format!("Drag on the scan, rim to rim: a hole or a boss read there · Drag off the scan: the base, on the ground or a face · Click: 20 mm, on the scan too · {}: one at once · Top bar: Add or Carve", crate::ui::prim_key(PrimKind::Cylinder))
        }
        (None, Tool::Create(PrimKind::Box)) if on_scan => "Drag: the base, on a face, the scan or the ground · On the scan, corner to corner across a block or a pocket: read when let go (else on to the height) · Click: 20 mm · B: one at once · Top bar: Add or Carve".into(),
        (None, Tool::Create(k)) if on_scan => format!("Drag: the base, on a face, the scan's face under the pointer or the ground · Click: 20 mm · {}: one at once · Top bar: Add or Carve", crate::ui::prim_key(k)),
        (None, Tool::Create(k)) => format!("Drag: the base, on the ground or a face · Click: 20 mm · {}: one at once · Top bar: Add or Carve", crate::ui::prim_key(k)),
        (None, Tool::Curve(k)) => {
            let how = if k.from_centre() { "from its centre out" } else { "corner to corner" };
            format!("Drag: a {} {how}, on a face or the ground · Click: 20 mm · Then its settings in its window · G: another", k.label().to_lowercase())
        }
        (None, Tool::Push(k)) => match k {
            PushKind::Inset => "Click a flat face: 2 mm, then drag its dot · Press and drag: sized at once".into(),
            PushKind::Extrude => "Press on a face and drag: out adds, in carves · A round face goes along its normals · On a closed curve: pulled into a solid".into(),
        },
        (None, Tool::Select) if st.curve_pull.is_some() => "Move: its height along its normal (the other way goes into the face) · Click: keep it · Esc: drop it · Top bar: Add or Carve".into(),
        (None, Tool::Select) if st.pivot_edit => "D held · Click: the pivot jumps onto the nearest corner, edge middle or circle centre (the moves then put it on the points) · Far from any: back to its own".into(),
        (None, Tool::Select) if st.sel_mode != SelMode::Object && st.selected_tweak().is_some() => "Arrows: move it · Centre square: on all three axes · D + click: pivot on a point · Window: X, Y, Z · Del: delete it".into(),
        (None, Tool::Knife) => {
            if st.knife.is_some() {
                "Click the next edge: a line across their face (a round one: along it), the cut goes on · Corners and middles catch first · Esc, right click: end the cut · Enter, Q, W: done, back to Select".into()
            } else {
                "Click an edge: a point, then the next edges · Corners and middles catch first · Ctrl + click: a cut across the face · Ctrl + Shift + click: a loop round the part".into()
            }
        }
        (None, Tool::Point) => "Click an edge: a point cut into it (its middle catches) · Esc: Select".into(),
        (None, Tool::Select) if st.face_drag.is_some() => "Away from the press: bigger · Release: keep it".into(),
        (None, Tool::Select) if st.round_drag.is_some() => "Away from the press: bigger · Ctrl held: chamfer · Shift held: tension (up fuller, down flatter)".into(),
        (None, Tool::Select) if st.sel_mode == SelMode::Vertex => {
            if st.vert_sel.is_empty() {
                "Click: a vertex · Double-click: those of its piece · Drag on empty: a rectangle · Drag a sketch corner: its fillet · Shift + click: add".into()
            } else if !st.picked_profile_corners().is_empty() {
                "Drag the corner: its fillet · Ctrl held: chamfer · Shift held: tension · Arrows: move it · Esc: deselect".into()
            } else {
                "Arrows: move it · Centre square: on all three axes · D + click: pivot on a point · Shift + click or rectangle: add · Ctrl + rectangle: take away · Esc: deselect".into()
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
                "Click: a face · Double-click: its linked faces · Drag on empty: a rectangle · Shift + click: add · I: inset · Ctrl + E: extrude".into()
            } else {
                "Double-click: its linked faces · Shift + drag the face: extrude · Ctrl + drag: inset · Arrows: move it · Centre square: on all three axes · D + click: pivot on a point · I: inset · Ctrl + E: extrude · Esc: deselect".into()
            }
        }
        (None, Tool::Select) if st.sel_mode == SelMode::Edge => {
            if st.selected_round().is_some() {
                "Drag on it: its size · Shift held: tension · Shift + click an edge: add it · Del: delete it".into()
            } else if st.edge_sel.is_empty() {
                "Click: an edge · Double-click: its loop · Drag on one: a fillet · Ctrl held: chamfer · Shift held: tension · Drag on empty: a rectangle · Ctrl + B: chamfer".into()
            } else {
                "Drag an edge: a fillet · Shift + drag a picked edge: pull it · Arrows: move it · Centre square: on all three axes · D + click: pivot on a point · Ctrl + B: chamfer · Esc: deselect".into()
            }
        }
        (None, Tool::Select) => {
            if let Some(d) = &st.drag {
                match d.handle {
                    Handle::Rot(_) => "Round the ring: turn · 15° steps while Snap is on · Release: keep it".into(),
                    Handle::Scale(_) => "Away from the centre: bigger · Release: keep it".into(),
                    Handle::ScaleAll => "To the right: bigger · To the left: smaller · Release: keep it".into(),
                    Handle::Cage(_) => "The side facing the grip stays · Shift: proportions kept · Alt: from the middle · Ctrl: no snapping · Release: keep it".into(),
                    _ => "Moving: the kernel rebuilds the booleans live".into(),
                }
            } else if st.sel_mode == SelMode::Object && st.selection.len() >= 2 && st.selection.iter().all(|id| st.doc.is_closed_curve(*id)) {
                "Window: Union, Subtract (the first picked less the others), Intersect: one curve, kept live · Shift + click: add a curve".into()
            } else if st.single().is_some_and(|f| f.is_extruded_curve()) {
                "Ctrl + E: its height again · Window: its curve's settings, Amount, Flat · Arrow: its height · Del: delete it".into()
            } else if st.single().is_some_and(|f| f.shape_bool.is_some()) {
                "Ctrl + E: extrude it · Click one of its curves again: that curve (its settings, moved alone; the result follows) · Window: the operation and its curves · Del: it and its curves".into()
            } else if st.pullable().is_some() && st.single().is_some_and(|f| st.curve_inside(f.id).is_none()) {
                "Ctrl + E: extrude it (the pointer sets its height) · Window: its settings · Shift + click: add a curve (Union, Subtract, Intersect)".into()
            } else if st.single().is_some_and(|f| st.curve_inside(f.id).is_some()) {
                "A curve of a 2D boolean: moved or set, the result follows · Window: back to the boolean · Click on empty: deselect".into()
            } else if st.selection.is_empty() {
                if st.doc.features.is_empty() {
                    "B: a cube · C: a cylinder · L: a sketch · Then: drag in the view".into()
                } else {
                    "Click: a shape · Drag: a rectangle · Right button: menu · Alt + drag: orbit · F9 F10 F11: vertex, edge, face".into()
                }
            } else {
                match st.gizmo {
                    GizmoMode::Select => "W: move · E: rotate · R: scale · Right button: menu · Del: delete · Esc: deselect".into(),
                    GizmoMode::Move => "Arrows and squares: move · Centre square: on all three axes · D + click: pivot on a point · E: rotate · R: scale · Del: delete".into(),
                    GizmoMode::Rotate => "Drag a ring: turn round X, Y or Z · W: move · R: scale · Del: delete".into(),
                    GizmoMode::Scale => "Cage: a face's grip one side, an edge's two, a corner all three · Shift: proportions kept · Alt: from the middle · W: move · E: rotate".into(),
                }
            }
        }
    }
}

/// The help box's lines in the Sculpt and Poly tabs (None: as in Forme).
fn sculpt_poly_hint(st: &AppState) -> Option<String> {
    use crate::workspace::Workspace;
    // a linked surface in NURBS: its contour's points
    if st.patch_comp() {
        return Some(if st.patch_drag.is_some() {
            "Release: the contour kept (one undo step), the Poly layer follows".into()
        } else {
            "Click: a contour point (a square: a corner, it takes its tangents) · Shift: add · Drag a point: move it · W: the gizmo on the picked ones · 4: Object mode".into()
        });
    }
    if st.tool == Tool::Sculpt {
        let b = st.sculpt.brush;
        let what = if st.stroke.is_some() {
            "Release: the stroke kept (one undo step)".to_string()
        } else if st.doc.features.iter().any(|f| f.mesh.as_ref().map(|m| m.kind == nurbs_kernel::mesh::MeshKind::Sculpt).unwrap_or(false)) {
            format!("Drag on the clay: {} · Ctrl: the other way · Shift: smooth", b.label())
        } else {
            "O: a clay sphere · B: a clay block · Or a Poly mesh: Send to Sculpt".to_string()
        };
        return Some(format!("{what} · X C V S G I T P: brushes · [ ]: size · M: mirror {} · W E R: move, turn, scale", if st.sculpt.mirror { "off" } else { "on" }));
    }
    if let Some(b) = st.poly_bevel.as_ref().filter(|b| b.live) {
        let kind = if b.chamfer { "chamfer" } else { "bevel" };
        return Some(format!("Move left / right: the width · [ ]: segments ({}) · C: {} (now) or the other · Click: keep it · Right click or Esc: drop it", b.segments, kind));
    }
    if st.tool == Tool::PolyCut {
        return Some(if st.poly_cut.is_some() {
            "Click an edge or a point: on across the face · Ctrl + click: an edge loop there · Right click or a click off the mesh: this cut done · Enter or Esc: done".into()
        } else {
            "Click an edge or a point of a Poly mesh: the cut starts (a point catches at 12 points, an edge's middle at 6) · Ctrl + click an edge: an edge loop there".into()
        });
    }
    if st.tool == Tool::PolyLoop {
        return Some("Click an edge: a loop cut across its ring (shown under the pointer) · Esc or Q: done".into());
    }
    if let Some(g) = &st.poly_pull {
        return Some(match g.kind {
            crate::poly::PullKind::Extrude => "Move: out or in along the faces' normal · Click: keep it · Right click or Esc: drop it".into(),
            crate::poly::PullKind::Inset => "Move: towards the faces' centre, deeper · Click: keep it · Right click or Esc: drop it".into(),
        });
    }
    if st.poly_drag.is_some() {
        return Some("Release: keep it · Ctrl: no snapping".into());
    }
    if st.poly_comp() && st.tool == Tool::Select {
        if st.poly_target().is_none() {
            return Some("Click a Poly mesh's component, or select one in Object mode (4) · B C O P: a new cube, cylinder, sphere, plane".into());
        }
        return Some(match st.sel_mode {
            SelMode::Vertex => "Click: a point · Shift: add · Ctrl: take away · Drag on empty: a rectangle · W E R: move, turn, scale · Del: its faces".into(),
            SelMode::Edge => "Click: an edge · Shift: add · Drag on empty: a rectangle · Ctrl + B: bevel · Ctrl + R: insert edge loop · K: cut · W E R: move, turn, scale".into(),
            SelMode::Face => "Click: a face · Shift: add · Ctrl + E: extrude · I: inset · W E R: move, turn, scale · Del: delete".into(),
            SelMode::Object => String::new(),
        });
    }
    if st.workspace == Workspace::Poly && st.tool == Tool::Select && st.drag.is_none() && st.selection.is_empty() {
        return Some("B: a cube · C: a cylinder · O: a sphere · P: a plane · Then 1 2 3: points, edges, faces · Alt + drag: orbit".into());
    }
    if st.workspace == Workspace::Sculpt && st.tool == Tool::Select && st.drag.is_none() {
        return Some("X C V S G I T P or a brush card: sculpt again · W E R: move, turn, scale the selection · O / B: new clay".into());
    }
    None
}
