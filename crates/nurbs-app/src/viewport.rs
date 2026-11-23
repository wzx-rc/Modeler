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
        draw_push(p, st);
        draw_shell(p, st);
        draw_mirror(p, st);
        draw_array(p, st);
        draw_knife(p, st, ctx.input(|i| i.pointer.hover_pos()));
        draw_brush(p, st, ctx.input(|i| i.pointer.latest_pos()));
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
    // the selected Shell's dot (or its line across the wall)
    if let Some(g) = st.shell_grip() {
        if let (Some(a), Some(b)) = (world(st, g.from), world(st, g.at)) {
            if (b - p).length() < 10.0 || dist_seg(p, a, b) < 5.0 {
                return Some(Handle::ShellThick);
            }
        }
    }
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
    // the selected Array's grips: its squares first, then its arrows
    let grips = st.array_grips();
    for g in grips.iter().filter(|g| g.from.is_none()) {
        if world(st, g.at).map(|q| (q - p).length() < 9.0).unwrap_or(false) {
            return Some(g.handle);
        }
    }
    for g in grips.iter() {
        if let (Some(f), Some(b)) = (g.from.and_then(|f| world(st, f)), world(st, g.at)) {
            if dist_seg(p, f + (b - f) * 0.3, b + (b - f).normalized() * 10.0) < 8.0 {
                return Some(g.handle);
            }
        }
    }
    if st.tool != Tool::Select || st.creation.is_some() {
        return None;
    }
    // Q: no gizmo; E and R: rings and scale handles (shapes only)
    match st.gizmo {
        GizmoMode::Select => return None,
        GizmoMode::Rotate if st.sel_mode == SelMode::Object => return ring_at(st, p),
        GizmoMode::Scale => return scale_at(st, p),
        _ => {}
    }
    // (the move gizmo has no size dots: Romain 20 nov., 11 h 44, "enlève
    // les 4 points de scale avec le translate"; R scales)
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

/// The scale gizmo (the classic one again: Romain 21 nov., 0 h 11, "oublie
/// la box à poignées et reviens au classique"): the pivot, the tips along
/// the shape's own axes.
fn scale_points(st: &AppState) -> Option<(Pos2, [Option<Pos2>; 3])> {
    let pivot = st.pivot()?;
    let frame = st.scale_frame()?;
    let c = world(st, pivot)?;
    let v = st.view();
    let k = v.pixel_size_at((pivot.as_vec3() - v.eye).length()) as f64 * ARROW as f64;
    Some((c, [0, 1, 2].map(|a| world(st, pivot + frame * DVec3::AXES[a] * k))))
}

fn scale_at(st: &AppState, p: Pos2) -> Option<Handle> {
    let (c, tips) = scale_points(st)?;
    if (p - c).length() < 10.0 {
        return Some(Handle::ScaleAll);
    }
    let mut best: Option<(f32, usize)> = None;
    for (a, t) in tips.iter().enumerate() {
        if let Some(t) = t {
            let d = dist_seg(p, c + (*t - c) * 0.2, *t).min((p - *t).length() - 3.0);
            if d < 8.0 && best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, a));
            }
        }
    }
    best.map(|(_, a)| Handle::Scale(a))
}

/// The scale cage's grip under the pointer (corners first, then edges,
/// then faces, when they lie as near). Not used since 21 nov. (the classic
/// gizmo is back); kept with `cage.rs` should the cage be asked again.
#[allow(dead_code)]
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
    // zoom, whatever the tool. With a pen (Romain 22 nov.: "full stylet"),
    // the tip alone: Alt + Shift + left pans, Alt + Ctrl + left zooms, and
    // the pen's barrel button held counts as Alt (`pen.rs`)
    // (not on the frame a button goes down: a pen's tip may come down far
    // from where it was last seen, the view would jump)
    let (alt, d, l, m, r, nav_shift, nav_ctrl) = ctx.input(|i| {
        (
            i.modifiers.alt || crate::pen::barrel(),
            if i.pointer.any_pressed() { egui::Vec2::ZERO } else { i.pointer.delta() },
            i.pointer.button_down(PointerButton::Primary),
            i.pointer.button_down(PointerButton::Middle),
            i.pointer.button_down(PointerButton::Secondary),
            i.modifiers.shift,
            i.modifiers.ctrl || i.modifiers.mac_cmd,
        )
    });
    let in_base = st.creation.as_ref().map(|c| c.phase == Phase::Base).unwrap_or(false);
    // (a press begun over an island is the island's)
    if alt && (resp.hovered() || resp.dragged() || resp.is_pointer_button_down_on()) && !in_base && st.drag.is_none() {
        if (l || m || r) && d != egui::Vec2::ZERO {
            st.stop_flight();
            if l && nav_shift {
                st.camera.pan(d.x, d.y, rect.height());
            } else if l && nav_ctrl {
                st.camera.zoom(((d.x + d.y) * 0.004).exp());
            } else if l {
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
    let select = st.tool == Tool::Select;
    let knife = matches!(st.tool, Tool::Knife | Tool::Point);
    let edges = (select && st.sel_mode == SelMode::Edge) || knife;
    let faces = (select && st.sel_mode == SelMode::Face) || matches!(st.tool, Tool::Push(_) | Tool::ShellPick);
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
        let by_feature = (select && matches!(st.sel_mode, SelMode::Object | SelMode::Edge)) || st.tool == Tool::Align;
        st.hover_feature = if st.hot_handle.is_none() && by_feature { hover.and_then(|h| st.pick_path(p2(h))).or_else(|| st.hover_hit.and_then(|h| st.feature_of_face(h.face))) } else { None };
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
    // a closed group lights whole under the pointer (a click takes it all)
    st.hover_also = match st.hover_feature {
        Some(id) if select && st.sel_mode == SelMode::Object => Some(st.pick_ids(id)).filter(|v| v.len() > 1).unwrap_or_default(),
        _ => vec![],
    };

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

    // the selected Shell's faces to open (`shell.rs`): a click opens one
    if st.tool == Tool::ShellPick {
        if resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos().or(hover) {
                st.shell_pick(p2(p));
            }
        } else if resp.clicked_by(PointerButton::Secondary) {
            st.select_tool();
        }
        return;
    }

    // Align (`align.rs`): the click on the target; its window open, the
    // view only turns (the window's buttons end it)
    if st.tool == Tool::Align {
        if resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos().or(hover) {
                st.align_pick(p2(p));
            }
        } else if resp.clicked_by(PointerButton::Secondary) {
            st.align_cancel();
        }
        return;
    }
    if st.align_open() {
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
            let on_gizmo = matches!(handle_at(st, origin), Some(Handle::Axis(_) | Handle::Plane(_) | Handle::Free));
            // (Shift + drag on the move gizmo with faces picked: they are
            // extruded, Maya; Romain 23 nov., 7 h 05)
            let quick = shift && !ctrl && st.gizmo != GizmoMode::Scale && if on_gizmo { st.shift_extrude_picked(p2(origin)) } else { st.begin_face_drag(p2(origin), true, false) };
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
        Tool::Align | Tool::ShellPick => {}
        Tool::Form => {
            // a brush stroke on a form (`forms.rs`)
            if st.sculpt.is_some() {
                if let Some(p) = hover.or(resp.interact_pointer_pos()) {
                    st.form_drag(p2(p));
                }
                if resp.drag_stopped() || !l {
                    st.form_release();
                }
                return;
            }
            if resp.drag_started_by(PointerButton::Primary) {
                let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                if st.form_press(p2(origin), shift, ctrl) {
                    if let Some(p) = hover {
                        st.form_drag(p2(p));
                    }
                }
            } else if resp.clicked_by(PointerButton::Primary) {
                if let Some(p) = resp.interact_pointer_pos() {
                    st.form_click(p2(p));
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
                    st.marquee_elements(a, b, shift, ctrl);
                }
            }
            if resp.clicked_by(PointerButton::Primary) {
                let at = resp.interact_pointer_pos().or(hover);
                let id = at.and_then(|p| st.pick_path(p2(p))).or_else(|| at.and_then(|p| st.pick_at(p2(p))).and_then(|h| st.feature_of_face(h.face)));
                // the element, not the operation whose face was clicked
                let id = id.map(|id| st.element_of(id));
                // (a closed group's shape: the whole group)
                st.click_shape(id, shift || ctrl);
            }
        }
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
    let hot = st.hot_handle;
    let dragging = st.drag.as_ref().map(|d| d.handle);
    if st.gizmo == GizmoMode::Select {
        return;
    }
    if (st.sel_mode == SelMode::Object && st.gizmo == GizmoMode::Rotate) || st.gizmo == GizmoMode::Scale {
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
        let Some((_, tips)) = scale_points(st) else { return };
        for (a, t) in tips.iter().enumerate() {
            if dragging.is_some() && dragging != Some(Handle::Scale(a)) && dragging != Some(Handle::ScaleAll) {
                continue;
            }
            let Some(t) = t else { continue };
            let on = hot == Some(Handle::Scale(a)) || hot == Some(Handle::ScaleAll);
            let col = if on { HOT } else { colors[a] };
            p.line_segment([c + (*t - c) * 0.18, *t], Stroke::new(if on { 3.0 } else { 2.2 }, col));
            p.rect_filled(Rect::from_center_size(*t, vec2(10.0, 10.0)), 2.0, col);
        }
        let on = hot == Some(Handle::ScaleAll);
        p.rect(Rect::from_center_size(c, vec2(14.0, 14.0)), 3.0, if on { tint(HOT, 0.5) } else { tint(Color32::WHITE, 0.25) }, Stroke::new(1.5, if on { HOT } else { Color32::WHITE }), egui::StrokeKind::Middle);
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
        draw_surface_outline(p, st, k, false);
        return;
    }
    // the Path tool's CURVE: the spline through the points as it will be
    if st.tool == Tool::Create(PrimKind::Path) && st.surface_curve {
        draw_surface_outline(p, st, k, true);
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
fn draw_surface_outline(p: &Painter, st: &AppState, k: &crate::state::Sketch, path: bool) {
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
    // the outline with its fillets, closed once it has 3 points (a path:
    // open, closed only when the pointer is on its first point)
    let n = placed + next.is_some() as usize;
    let closed = if path { closing && placed >= 3 } else { n >= 3 };
    let line = st.surface_line(k, next, closed);
    let sl: Vec<Option<Pos2>> = line.iter().map(|q| world(st, *q)).collect();
    let m = sl.len();
    for i in 0..m {
        let j = (i + 1) % m;
        if j == 0 && (n < 3 || (path && !closed)) {
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
        let txt = if closing {
            if path { "close".to_string() } else { "close: fill it".to_string() }
        } else if path && placed >= 2 {
            format!("{placed} points · Enter: an open curve")
        } else {
            format!("{} point{} · {on}", placed, if placed == 1 { "" } else { "s" })
        };
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

/// The Knife: the point the pointer catches on an edge (a diamond), the
/// line from the cut's last point to it (or to the pointer, dashed).
/// The Forms tool's brush: a ring of its radius laid on the surface under
/// the pointer (on screen off the part), and its mirror with the symmetry.
fn draw_brush(p: &Painter, st: &AppState, pointer: Option<Pos2>) {
    if st.tool != Tool::Form {
        return;
    }
    let Some(at) = pointer else { return };
    let light = Color32::from_rgba_unmultiplied(240, 242, 245, 230);
    let dark = Color32::from_rgba_unmultiplied(10, 12, 16, 150);
    let ring = |pts: Vec<Pos2>, strong: bool| {
        let (w, c) = if strong { (1.4, light) } else { (1.0, Color32::from_rgba_unmultiplied(240, 242, 245, 120)) };
        p.add(egui::Shape::closed_line(pts.clone(), Stroke::new(w + 2.0, dark)));
        p.add(egui::Shape::closed_line(pts, Stroke::new(w, c)));
    };
    // on a stroke: the ring on screen round the pointer
    let (hit, strong) = match &st.sculpt {
        Some(_) => (None, true),
        None => (st.hover_hit.and_then(|h| st.feature_of_face(h.face).map(|id| (h.point.as_dvec3(), h.normal.as_dvec3(), id))), true),
    };
    let r_pts = st.form.radius;
    match hit {
        Some((h, n, id)) => {
            let r = r_pts as f64 * st.mm_per_point(h);
            let n = n.normalize_or(DVec3::Y);
            let u = n.any_orthonormal_vector();
            let v = n.cross(u);
            let circle = |c: DVec3, u: DVec3, v: DVec3| -> Vec<Pos2> { (0..48).filter_map(|k| { let a = std::f64::consts::TAU * k as f64 / 48.0; world(st, c + (u * a.cos() + v * a.sin()) * r) }).collect() };
            let pts = circle(h, u, v);
            if pts.len() > 8 {
                ring(pts, strong);
            }
            if let Some(c) = world(st, h) {
                p.circle_filled(c, 2.0, light);
            }
            // the mirror, across the form's middle
            if st.form.mirror {
                if let Some(f) = st.doc.feature(id).filter(|f| f.form.is_some()) {
                    let m = f.matrix();
                    let (o, ax) = (m.transform_point3(DVec3::ZERO), m.transform_vector3(DVec3::X).normalize_or(DVec3::X));
                    let refl = |q: DVec3| q - ax * (2.0 * (q - o).dot(ax));
                    let refv = |q: DVec3| q - ax * (2.0 * q.dot(ax));
                    let pts = circle(refl(h), refv(u), refv(v));
                    if pts.len() > 8 {
                        ring(pts, false);
                    }
                }
            }
        }
        None => {
            let pts: Vec<Pos2> = (0..48).map(|k| { let a = std::f32::consts::TAU * k as f32 / 48.0; at + vec2(a.cos(), a.sin()) * r_pts }).collect();
            ring(pts, st.sculpt.is_some());
        }
    }
    if let Some(s) = &st.sculpt {
        chip(p, at + vec2(16.0, 16.0), Align2::LEFT_TOP, s.brush.label(), HOT, HOT);
    }
}

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

/// The selected Shell's grip: a line across the wall from the face it
/// keeps, ticked at both ends, its dot at the thickness; hovered or
/// dragged, the value by it.
fn draw_shell(p: &Painter, st: &AppState) {
    let Some(g) = st.shell_grip() else { return };
    let Some(sh) = st.selected_shell().and_then(|f| f.shell.clone()) else { return };
    let (Some(a), Some(b)) = (world(st, g.from), world(st, g.at)) else { return };
    let col = accent();
    let on = st.hot_handle == Some(Handle::ShellThick) || st.drag.as_ref().map(|d| d.handle == Handle::ShellThick).unwrap_or(false);
    // seen short on screen: the line drawn on past the dot (dimmer), so it
    // can be found and its way seen
    let d = b - a;
    let len = d.length();
    let dir = if len > 1e-3 { d / len } else { vec2(0.0, -1.0) };
    if len < 24.0 {
        p.line_segment([b, b + dir * (24.0 - len)], Stroke::new(1.0, tint(col, 0.35)));
    }
    p.line_segment([a, b], Stroke::new(if on { 2.4 } else { 1.8 }, col));
    let n = vec2(-dir.y, dir.x) * 5.0;
    p.line_segment([a - n, a + n], Stroke::new(1.4, col));
    p.circle(b, if on { 4.5 } else { 3.5 }, if on { HOT } else { chip_bg() }, Stroke::new(1.4, col));
    if on {
        let at = b + dir * 16.0;
        let anchor = if dir.x >= 0.0 { Align2::LEFT_CENTER } else { Align2::RIGHT_CENTER };
        let way = if sh.outward { " out" } else { "" };
        chip(p, at, anchor, &format!("Wall {} mm{way}", fmt_mm(sh.thickness as f32)), col, col);
    }
}

/// The selected Mirror's plane: a faint pane round the part, its border
/// dashed, its axis and the side kept.
fn draw_mirror(p: &Painter, st: &AppState) {
    let Some((m, q)) = st.mirror_plane() else { return };
    let col = match m.axis {
        0 => AXIS_X,
        1 => AXIS_Y,
        _ => AXIS_Z,
    };
    let pts: Vec<Pos2> = q.iter().filter_map(|w| world(st, *w)).collect();
    if pts.len() != 4 {
        return;
    }
    p.add(egui::Shape::convex_polygon(pts.clone(), tint(col, 0.07), Stroke::NONE));
    for i in 0..4 {
        p.add(egui::Shape::dashed_line(&[pts[i], pts[(i + 1) % 4]], Stroke::new(1.2, tint(col, 0.75)), 6.0, 4.0));
    }
    // the side kept: an arrow from the pane's middle
    let c = (q[0] + q[1] + q[2] + q[3]) * 0.25;
    let n = m.normal() * if m.flip { -1.0 } else { 1.0 };
    let reach = (q[0] - q[2]).length() * 0.08;
    if let (Some(a), Some(b)) = (world(st, c), world(st, c + n * reach)) {
        arrow(p, a, b, col, 2.0);
        chip(p, b + vec2(10.0, -4.0), Align2::LEFT_BOTTOM, &format!("Mirror {} · keeps {}{}", m.axis_name(), if m.flip { "−" } else { "+" }, m.axis_name()), col, col);
    }
}

/// The selected Array's places: a dot at each copy's middle (the
/// original's ringed), joined in order by a dashed line (round a circle,
/// an arc on its axis), with its count.
fn draw_array(p: &Painter, st: &AppState) {
    let Some((a, pts)) = st.array_marks() else { return };
    let col = accent();
    let screen: Vec<Option<Pos2>> = pts.iter().map(|w| world(st, *w)).collect();
    let dash = |p: &Painter, line: &[Pos2]| {
        if line.len() >= 2 {
            p.add(egui::Shape::dashed_line(line, Stroke::new(1.2, tint(col, 0.6)), 5.0, 4.0));
        }
    };
    match a.kind {
        nurbs_kernel::ArrayKind::Circle => {
            // the arc through the copies' middles, on the axis
            let n = a.axis_dir();
            let c0 = pts[0];
            let centre = a.centre + n * (c0 - a.centre).dot(n);
            let r0 = c0 - centre;
            let total = if a.angle.abs() >= 359.999 { 360.0 } else { a.step_angle() * (a.count.max(1) - 1) as f64 };
            let steps = ((total.abs() / 4.0).ceil() as usize).max(2);
            let arc: Vec<Pos2> = (0..=steps).filter_map(|i| world(st, centre + glam::DQuat::from_axis_angle(n, (total * i as f64 / steps as f64).to_radians()) * r0)).collect();
            dash(p, &arc);
            // the axis, a little past the arc
            let reach = r0.length().max(5.0) * 0.35;
            if let (Some(u), Some(v)) = (world(st, centre - n * reach), world(st, centre + n * reach)) {
                p.line_segment([u, v], Stroke::new(1.2, tint(col, 0.8)));
                p.circle_filled(world(st, centre).unwrap_or(u), 2.5, col);
            }
        }
        nurbs_kernel::ArrayKind::Line | nurbs_kernel::ArrayKind::Grid => {
            let n = a.count.max(1) as usize;
            let rows = if a.kind == nurbs_kernel::ArrayKind::Grid { a.count2.max(1) as usize } else { 1 };
            // the places in rows: row j holds i = 0..n (the original first)
            let at = |i: usize, j: usize| -> Option<Pos2> {
                let k = j * n + i;
                screen.get(k).copied().flatten()
            };
            for j in 0..rows {
                let row: Vec<Pos2> = (0..n).filter_map(|i| at(i, j)).collect();
                dash(p, &row);
            }
            if rows > 1 {
                let col0: Vec<Pos2> = (0..rows).filter_map(|j| at(0, j)).collect();
                dash(p, &col0);
            }
        }
    }
    for (k, s) in screen.iter().enumerate() {
        let Some(s) = s else { continue };
        if k == 0 {
            p.circle_stroke(*s, 5.0, Stroke::new(1.5, col));
        }
        p.circle_filled(*s, 2.6, col);
    }
    // its grips: squares on the first copies, arrows past the ends; the
    // one hovered or dragged lit, its value by it
    let hot = st.hot_handle;
    let dragging = st.drag.as_ref().map(|d| d.handle);
    let dark = Color32::from_rgb(0x20, 0x22, 0x26);
    let mut lit_at = None;
    let grips = st.array_grips();
    for g in &grips {
        if dragging.is_some() && dragging != Some(g.handle) {
            continue;
        }
        let on = hot == Some(g.handle) || dragging == Some(g.handle);
        let Some(q) = world(st, g.at) else { continue };
        match g.from.and_then(|f| world(st, f)) {
            Some(f) => {
                p.circle_filled(f, 2.0, col);
                arrow(p, f, q, if on { HOT } else { col }, if on { 2.8 } else { 2.0 });
            }
            None => {
                let r = if on { 5.5 } else { 4.5 };
                p.rect(Rect::from_center_size(q, vec2(r * 2.0, r * 2.0)), 1.5, if on { HOT } else { Color32::WHITE }, Stroke::new(1.0, dark), egui::StrokeKind::Middle);
            }
        }
        if on {
            lit_at = Some((g.handle, q));
        }
    }
    if let Some((h, q)) = lit_at {
        let text = match (&st.gizmo_readout, dragging) {
            (Some(t), Some(_)) => t.clone(),
            _ => grip_value(&a, h),
        };
        // over the windows (the operation window stays put: the arrow may
        // reach under it)
        let top = p.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("array-grip-chip"))).with_clip_rect(p.clip_rect());
        chip(&top, q + vec2(14.0, 12.0), Align2::LEFT_TOP, &text, HOT, HOT);
    } else if let Some(Some(last)) = screen.last() {
        let total = pts.len();
        chip(p, *last + vec2(12.0, -10.0), Align2::LEFT_BOTTOM, &format!("Array · {} · {total}", a.kind_name()), col, col);
    }
}

/// What an Array grip changes, as it is now (shown while hovered).
fn grip_value(a: &nurbs_kernel::Array, h: Handle) -> String {
    match h {
        Handle::ArrayStep(w) => {
            let s = if w == 0 { a.step } else { a.step2 };
            format!("{} {} mm · drag", if w == 0 { "Step" } else { "Row step" }, crate::widgets::fmt3(s.length()))
        }
        Handle::ArrayCount(_) if a.kind == nurbs_kernel::ArrayKind::Circle => format!("Count {} · drag round", a.count),
        Handle::ArrayCount(w) => format!("{} {} · pull: more", if w == 0 { "Count" } else { "Rows" }, if w == 0 { a.count } else { a.count2 }),
        Handle::ArrayAngle => format!("Angle {}° · drag round", crate::widgets::fmt3(if a.angle.abs() >= 359.999 { 360.0 } else { a.angle }).trim_end_matches(",0")),
        _ => String::new(),
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
    let h = hint_of(st);
    // a component mode with R: the gizmo's cubes scale what is picked
    if st.gizmo == GizmoMode::Scale && st.sel_mode != SelMode::Object {
        return h.replace("Arrows: move it · Centre square: on all three axes", "Cubes: scale it along X, Y or Z · Centre: all at once");
    }
    h
}

fn hint_of(st: &AppState) -> String {
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
        (None, Tool::Create(PrimKind::Path)) => match &st.sketch {
            None if on_scan => "Click: the first point, on a face, the scan or the ground".into(),
            None => "Click: the first point, on a face or the ground".into(),
            Some(k) if k.pts.len() < 2 => "Click or drag: a corner · Ctrl + Z or Backspace: the last point off · Esc: drop it".into(),
            Some(_) if st.surface_curve => "Enter: an open curve · First point: a closed one · Shift + click a point: sharp / smooth · Shift: 0°, 45°, 90°".into(),
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
        (None, Tool::Align) => "Click: the shape to align the selection to · Esc or right click: cancel".into(),
        (None, Tool::ShellPick) => "Click: a face of the part to open · Esc, Enter or right click: done · The window's ×: close one again".into(),
        (None, Tool::Select) if st.align_open() => "Min / Max: the box's sides · Center: its middle · Pivot: the base's centre (or the one put with D) · Apply: keep and go on · Enter: OK · Esc: cancel".into(),
        (None, Tool::Form) if st.sculpt.is_some() => "Drag: the brush pulls the surface · Release: done (one undo step) · Esc: cancel".into(),
        (None, Tool::Form) => format!(
            "Press on a form and drag: {} · Shift + drag: Smooth · Ctrl: the other way · [ ]: radius · Click on the ground: a new form · A plain Cube or Cylinder becomes a form",
            st.form.brush.label()
        ),
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
                    Handle::ArrayStep(_) => "Along the line: the step, the copies follow · Release: keep it".into(),
                    Handle::ArrayCount(_) if st.selected_array().and_then(|f| f.array.as_ref()).map(|a| a.kind == nurbs_kernel::ArrayKind::Circle).unwrap_or(false) => "Round the axis: nearer the original, more copies · Release: keep it".into(),
                    Handle::ArrayCount(_) => "Out: more copies, the same step · In: fewer · Release: keep it".into(),
                    Handle::ArrayAngle => "Round the axis: the angle, the count kept · 15° steps while Snap is on · 360°: evenly all round · Release: keep it".into(),
                    Handle::ShellThick => "Along the line: the walls' thickness, 0.1 mm steps while Snap is on · Release: keep it".into(),
                    Handle::Scale(_) => "Away from the centre: bigger · Release: keep it".into(),
                    Handle::ScaleAll => "To the right: bigger · To the left: smaller · Release: keep it".into(),
                    Handle::Cage(_) => "The side facing the grip stays · Shift: proportions kept · Alt: from the middle · Ctrl: no snapping · Release: keep it".into(),
                    _ => "Moving: the kernel rebuilds the booleans live".into(),
                }
            } else if st.selected_array().is_some() {
                "Square on the first copy: the step (round a circle: how many) · Arrow at the end: how many (round a circle: the angle) · A copy clicked is the original: an operation on it is made on all · Del: delete it".into()
            } else if st.selected_shell().is_some() {
                "Dot: the walls' thickness · Window: Inward / Outward, the faces open, Pick faces · Del: delete it".into()
            } else if st.selected_mirror().is_some() {
                "Window: the plane's axis, where it passes, the side kept · The steps after it are made on both sides · Del: delete it".into()
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
                    "Click: a shape · Drag: a rectangle · Right button: menu · Alt + drag: orbit, with Shift pan, with Ctrl zoom · F9 F10 F11: vertex, edge, face".into()
                }
            } else {
                match st.gizmo {
                    GizmoMode::Select => "W: move · E: rotate · R: scale · Right button: menu · Del: delete · Esc: deselect".into(),
                    _ if st.selected_group().is_some_and(|g| g.open) => "An open group: click one of its shapes to work on it alone · Its bar: close it again · Arrows: move the whole group · D + click: its pivot on a point · Ctrl + Shift + G: ungroup".into(),
                    _ if st.selected_group().is_some() => "A group: its shapes move, turn and scale together, about its pivot · Its bar: open it to pick its shapes one by one · D + click: its pivot on a point · Ctrl + Shift + G: ungroup · Ctrl + D: a copy".into(),
                    GizmoMode::Move if st.selection.len() > 1 => "Arrows and squares: move · Centre square: on all three axes · Ctrl + G: group them · D + click: pivot on a point · Alt + A: align · Ctrl + Shift + D: array · E: rotate · R: scale · Del: delete".into(),
                    GizmoMode::Move => "Arrows and squares: move · Centre square: on all three axes · D + click: pivot on a point · Alt + A: align · Alt + M: mirror · Ctrl + Shift + D: array · E: rotate · R: scale · Del: delete".into(),
                    GizmoMode::Rotate => "Drag a ring: turn round X, Y or Z · W: move · R: scale · Del: delete".into(),
                    GizmoMode::Scale => "Cubes: along the shape's axes · Centre: all at once · W: move · E: rotate".into(),
                }
            }
        }
    }
}
