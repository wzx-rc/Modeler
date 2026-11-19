//! The 3D view's input: Maya navigation (Alt + left orbit, Alt + middle pan,
//! Alt + right zoom, the wheel zooms, the middle button alone pans, a left
//! drag that no tool takes orbits). The tool in hand takes the left button:
//! the Select tool picks by mode and a drag pulls what is under it, the
//! Pen clicks points, the Circle, Slot and Box drag across the scan.

use crate::state::*;
use crate::theme::*;
use egui::{pos2, vec2, Align2, Color32, Id, Order, PointerButton, Rect, Sense, Stroke, StrokeKind};

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    st.screen = [screen.width(), screen.height()];
    egui::Area::new(Id::new("viewport")).order(Order::Background).fixed_pos(screen.min).show(ctx, |ui| {
        let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
        handle_input(ctx, st, &resp, rect);
        if !st.bare {
            marks(ui.painter(), ctx, st, &resp);
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
    // a drag built coarse: fine again once the buttons are up
    if !ctx.input(|i| i.pointer.any_down()) {
        st.fine_again();
    }
    let busy = st.slot_drag.is_some() || st.circle.is_some() || st.gesture.is_some();
    if alt && (resp.hovered() || resp.dragged() || l || m || r) && !busy {
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
            st.press = None;
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
    let (pressed, released, pos, shift, free) = ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.pointer.latest_pos(), i.modifiers.shift, i.modifiers.ctrl || i.modifiers.command));
    let at = pos.map(|p| [p.x, p.y]);
    // the Slot and Box tools: press at one end or near a corner, drag along
    // or across, release on the other side
    if matches!(st.tool, Tool::Slot | Tool::Box) && !alt {
        if pressed && resp.hovered() {
            if let Some(a) = at {
                st.slot_press(a);
            }
        } else if st.slot_drag.is_some() {
            if let Some(a) = at {
                st.slot_move(a);
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
    // the Knife and the Point tool: clicks on the edges of the shapes
    if matches!(st.tool, Tool::Knife | Tool::Point) && !alt {
        if let Some(a) = at {
            if resp.hovered() {
                if pressed {
                    st.knife_click(a);
                } else if !l {
                    st.knife_hover_at(a);
                }
            }
        }
        if r && st.knife.is_some() {
            st.knife_end();
        }
    }
    // the Circle: drawn freely, rim to rim; released, found on the scan
    if st.tool == Tool::Circle && !alt {
        if pressed && resp.hovered() {
            if let Some(a) = at {
                st.circle_press(a);
            }
        } else if st.circle.is_some() {
            if let Some(a) = at {
                st.circle_move(a);
            }
            if released || !l {
                st.circle_release();
            }
        }
        if st.circle.is_some() {
            st.interacting = true;
            return;
        }
    }
    // the Select tool and the Pen: a click picks or places a point, a drag
    // pulls what is under the pointer (the view turns with Alt only, as in
    // Maya); with the Pen, Ctrl + drag on a point placed rounds it
    if matches!(st.tool, Tool::Select | Tool::Pen) && !alt {
        if let Some(a) = at {
            if pressed && resp.hovered() {
                if st.tool == Tool::Select {
                    st.select_press(a, shift);
                } else {
                    st.pen_press(a, shift, free);
                }
            }
            if st.press.is_some() {
                if l {
                    st.select_drag(a, free);
                }
                if released || !l {
                    if st.tool == Tool::Pen && st.gesture.is_none() {
                        if let Some(pr) = st.press.take() {
                            if !pr.moved && !pr.handle {
                                st.pen_click(pr.at, shift, false);
                            }
                        }
                        st.pen_drag_end();
                    } else {
                        st.select_release();
                    }
                }
                if st.gesture.is_some() {
                    st.interacting = true;
                }
                return;
            }
            if resp.hovered() && !m && !r {
                if st.tool == Tool::Select {
                    st.hover_at(a);
                } else {
                    st.pen_hover_at(a, shift, free);
                }
            } else if !resp.hovered() {
                st.hover_off();
            }
        }
    }
    // the middle button alone pans
    if m && !alt && d != egui::Vec2::ZERO && resp.dragged() {
        st.camera.pan(d.x, d.y, rect.height());
        st.interacting = true;
    }
}

/// The Pen's points and where the next one goes; the vertices in Vertex
/// mode; what is picked; the label at the pointer.
fn marks(p: &egui::Painter, ctx: &egui::Context, st: &AppState, resp: &egui::Response) {
    let orange = Color32::from_rgb(255, 118, 24);
    let blue = Color32::from_rgb(70, 160, 255);
    let scr = |x: glam::DVec3| st.project(x).map(|s| pos2(s[0], s[1]));
    // the outline being drawn
    if let Some(pe) = &st.pen {
        let closable = pe.pts.len() >= 3;
        for (i, q) in pe.pts.iter().enumerate() {
            let Some(c) = scr(pe.frame.to3(*q)) else { continue };
            p.circle_filled(c, 4.0, orange);
            p.circle_stroke(c, 4.0, Stroke::new(1.4, Color32::WHITE));
            if i == 0 && closable {
                p.circle_stroke(c, 8.0, Stroke::new(1.2, tint(accent(), 0.8)));
            }
        }
    }
    let mut label: Option<String> = None;
    if st.tool == Tool::Pen {
        if let Some(h) = &st.pen_hover {
            if let Some(c) = scr(h.p) {
                match h.snap {
                    PenSnap::Point => {
                        p.circle_stroke(c, 6.5, Stroke::new(1.6, blue));
                    }
                    PenSnap::Close => {
                        p.circle_stroke(c, 9.0, Stroke::new(2.0, Color32::WHITE));
                    }
                    PenSnap::Angle(_) | PenSnap::Free => {
                        p.circle_filled(c, 3.0, tint(orange, 0.9));
                    }
                }
            }
            let s = h.snap.label();
            let n = st.pen.as_ref().map(|pe| pe.pts.len()).unwrap_or(0);
            let what = if n == 0 {
                match &st.pen_plane {
                    Some(pl) if pl.flat => "Pen · on this flat face".to_string(),
                    Some(_) => "Pen · on the tangent plane".to_string(),
                    None => "Pen".to_string(),
                }
            } else if h.snap == PenSnap::Close {
                "Close: fill it".to_string()
            } else {
                format!("Pen · point {}", n + 1)
            };
            label = Some(if s.is_empty() || h.snap == PenSnap::Close { what } else { format!("{what} · {s}") });
        }
    }
    // Ctrl: the point placed under the pointer, its fillet
    if let Some(pe) = &st.pen {
        if let Some(i) = pe.drag.or(pe.grab) {
            if let Some(c) = scr(pe.frame.to3(pe.pts[i])) {
                p.circle_stroke(c, 8.0, Stroke::new(1.8, Color32::WHITE));
            }
            let r = pe.radii.get(i).copied().unwrap_or(0.0);
            label = Some(if pe.drag.is_some() || r > 0.0 { format!("Fillet {} mm", fmt(r, 2)) } else { "Ctrl + drag: fillet this point".into() });
        } else if st.tool == Tool::Pen && ctx.input(|i| i.modifiers.ctrl || i.modifiers.command) {
            label = Some("Ctrl: drag a point placed to round it".into());
        }
    }
    // the bodies' vertices in Vertex mode, the outline's corners brighter
    let vertex_mode = st.tool == Tool::Select && st.mode == SelMode::Vertex;
    if vertex_mode {
        for o in &st.objects {
            let Some(b) = o.body() else { continue };
            for v in &b.built.verts {
                if let Some(c) = scr(*v) {
                    let corner = b.corner_at(*v).is_some();
                    p.circle_filled(c, if corner { 3.2 } else { 2.4 }, if corner { orange } else { tint(TEXT2(), 0.8) });
                }
            }
            // a rounded corner shows at its arc's middle
            if let crate::body::Base::Poly { pts, .. } = &b.base {
                for i in 0..pts.len() {
                    if b.corners.get(i).copied().unwrap_or(0.0) > 0.0 {
                        if let Some(c) = b.corner_show(i).and_then(scr) {
                            p.circle_stroke(c, 4.0, Stroke::new(1.4, orange));
                        }
                    }
                }
            }
        }
    }
    // the points picked and under the pointer
    for c in &st.sel {
        for x in st.comp_points(c) {
            if let Some(s) = scr(x) {
                p.circle_filled(s, 5.0, Color32::WHITE);
                p.circle_stroke(s, 5.0, Stroke::new(1.4, orange));
            }
        }
    }
    if let Some(h) = &st.hover {
        for x in st.comp_points(h) {
            if let Some(s) = scr(x) {
                p.circle_stroke(s, 7.0, Stroke::new(1.6, Color32::from_rgb(255, 214, 170)));
            }
        }
    }
    if st.tool == Tool::Select && label.is_none() {
        label = st.hover.as_ref().map(|h| hover_label(st, h));
    }
    if matches!(st.tool, Tool::Knife | Tool::Point) {
        if let Some((obj, k, _)) = st.knife {
            if let Some(crate::body::Step::Cut { pts, .. }) = st.body(obj).and_then(|b| b.steps.get(k)) {
                for kp in pts {
                    if let Some(c) = scr(kp.at) {
                        p.circle_filled(c, 3.0, orange);
                    }
                }
            }
        }
        if let Some(c) = st.knife_hover.and_then(scr) {
            p.circle_filled(c, 3.5, Color32::WHITE);
            p.circle_stroke(c, 3.5, Stroke::new(1.2, orange));
        }
        label = Some(match (st.knife_hover, st.knife) {
            (None, _) => format!("{} · on an edge", st.tool.label()),
            (Some(_), Some(_)) if st.tool == Tool::Knife => "Knife · cut to here".into(),
            (Some(_), _) => format!("{} · a point here", st.tool.label()),
        });
    }
    if st.tool == Tool::Circle {
        label = Some(match &st.circle {
            Some(cd) => format!("Circle · Ø {} mm", fmt((cd.b - cd.a).length(), 2)),
            None => "Circle · press on the rim".into(),
        });
    }
    let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else { return };
    if !resp.hovered() && st.gesture.is_none() && st.circle.is_none() && st.slot_drag.is_none() {
        return;
    }
    let label = match (&st.gesture, label) {
        (Some(g), _) => Some(gesture_label(st, g)),
        (None, l) => l.or_else(|| (st.tool != Tool::Select).then(|| st.tool.label().to_string())),
    };
    let Some(label) = label else { return };
    let col = accent();
    p.circle_filled(pos, 2.0, col);
    let at = pos + vec2(12.0, -12.0);
    let g = p.layout_no_wrap(label, medium(11.5), on_accent());
    let r = Rect::from_min_size(at - vec2(0.0, g.size().y + 4.0), g.size() + vec2(12.0, 4.0));
    p.rect_filled(r, 5.0, tint(col, 0.92));
    p.galley(r.min + vec2(6.0, 2.0), g, on_accent());
}

/// What the pointer is over in the Select tool, and what a drag does.
fn hover_label(st: &AppState, h: &Comp) -> String {
    let b = st.body(h.obj());
    match h {
        Comp::Object(_) if b.is_some() => "Object · drag: thickness".into(),
        Comp::Object(_) => "Object".into(),
        Comp::Face { .. } => "Face · drag: thickness".into(),
        Comp::Step { .. } => "Fillet · drag: its size".into(),
        Comp::Edge { edge, .. } => {
            let open = b.and_then(|b| b.built.find_edge(edge).map(|e| b.built.edge_open(e))).unwrap_or(false);
            if open { "Open edge · drag pulls it out".into() } else if st.cut_bevel { "Edge · drag: bevel".into() } else { "Edge · drag: fillet".into() }
        }
        Comp::Corner { .. } => if st.cut_bevel { "Corner · drag: bevel".into() } else { "Corner · drag: fillet".into() },
        Comp::Vertex { .. } => "Vertex".into(),
    }
}

fn gesture_label(st: &AppState, g: &Gesture) -> String {
    match g {
        Gesture::Thick { obj, k, .. } => match st.body(*obj).and_then(|b| b.steps.get(*k)) {
            Some(crate::body::Step::Thick { t }) => format!("Thickness {} mm", fmt(*t, 2)),
            _ => "Thickness".into(),
        },
        Gesture::Extrude { obj, k, .. } => match st.body(*obj).and_then(|b| b.steps.get(*k)) {
            Some(crate::body::Step::Extrude { v, .. }) => format!("Pulled {} mm", fmt(v.length(), 2)),
            _ => "Pull".into(),
        },
        Gesture::Radius { obj, k: Some(k), .. } => match st.body(*obj).and_then(|b| b.steps.get(*k)) {
            Some(crate::body::Step::Fillet { r, .. }) => format!("Fillet {} mm", fmt(*r, 2)),
            Some(crate::body::Step::Bevel { d, .. }) => format!("Bevel {} mm", fmt(*d, 2)),
            _ => "Fillet".into(),
        },
        Gesture::Radius { obj, corners, .. } => match (st.body(*obj), corners.first()) {
            (Some(b), Some(&i)) => format!("{} {} mm", if b.bevel_at(i) { "Bevel" } else { "Fillet" }, fmt(b.corners.get(i).copied().unwrap_or(0.0), 2)),
            _ => "Corner".into(),
        },
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
        return format!("{} …", j.what);
    }
    if st.doc.is_none() {
        return "Open a scan (an .obj)".into();
    }
    match st.tool {
        Tool::Select => match st.mode {
            SelMode::Object => "Object (4): click a shape to pick it · drag on it: thickness · Alt + drag turns around".into(),
            SelMode::Face => "Face (3): click a face · drag: thickness · a fillet's face picks the fillet".into(),
            SelMode::Edge => "Edge (2): click an edge (Shift + click adds) · drag: an open edge is pulled out, another one filleted or bevelled (the option on top)".into(),
            SelMode::Vertex => "Vertex (1): click a corner (Shift + click adds) · drag on it: fillet or bevel (the option on top) · Ctrl+B bevels what is picked".into(),
        },
        Tool::Pen => match &st.pen {
            None if st.fresh.is_some() => "Made · drag on it (Select): thickness · Enter keeps it · Esc removes it · click elsewhere to draw the next one".into(),
            None => "Pen: click on the scan · the plane under the pointer is the one you draw on (blue: a flat face) · then click the corners".into(),
            Some(pe) if pe.pts.len() < 3 => format!("{} points · click the next corner · Shift: horizontal, vertical or 45° · Ctrl + drag on a point rounds it · Enter makes a line · Backspace takes one back · Esc stops", pe.pts.len()),
            Some(pe) => format!("{} points · click the first point to close and fill it · Enter makes a line · Backspace takes one back · Esc stops", pe.pts.len()),
        },
        Tool::Circle => match &st.circle {
            Some(_) => "Across to the other side of the rim, then release: it is found on the scan".into(),
            None if st.fresh.is_some() => "Made · its size and height on the left · Enter keeps it · Esc removes it".into(),
            None => "Circle: press on the rim of a hole, a boss, a button or a rivet and drag across it · released, it finds the round part and makes a cylinder".into(),
        },
        Tool::Slot => match (&st.slot_drag, &st.slot) {
            (Some(_), _) => "Along the slot to its other end, then release".into(),
            (None, Some(se)) => format!("A NURBS surface, {} × {} mm, {} rows: depth, profile and levels on the left, live · Enter keeps it · Esc removes · H hides the scan", fmt(se.slot.length(), 2), fmt(se.slot.width(), 2), se.params.levels + 1),
            (None, None) => "Slot: press at one end of a slot with round ends and drag along it to the other end".into(),
        },
        Tool::Knife => match st.knife {
            Some(_) => "Knife · click the next edge: a line across the flat face to it · Enter, Esc or right click ends the cut".into(),
            None => "Knife: click an edge to start (a point there, its middle caught), then the next ones: lines across the flat faces (Multi-Cut)".into(),
        },
        Tool::Point => "Point on edge: click an edge, a point is cut into it (its middle caught) · Enter ends".into(),
        Tool::Box => match (&st.slot_drag, &st.slot) {
            (Some(_), _) => "Across the block to its far corner, then release".into(),
            (None, Some(se)) => format!("A NURBS surface, {} × {} mm, {} rows: height, profile and levels on the left, live · Enter keeps it · Esc removes · H hides the scan", fmt(se.slot.length(), 2), fmt(se.slot.width(), 2), se.params.levels + 1),
            (None, None) => "Box: press near a corner of a block with rounded corners (raised, or a pocket) and drag across it to the far corner".into(),
        },
    }
}
