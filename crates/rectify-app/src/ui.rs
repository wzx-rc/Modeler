//! The interface (Forme NURBS's islands, in orange): the file, the tools,
//! the component modes and the view at the top, what is picked or the
//! tool's settings on the left, a summary at the bottom.

use crate::icons::{self, Icon};
use crate::state::*;
use crate::body::{Base, Step};
use rectify_core::slot::Kind;
use crate::theme::*;
use crate::widgets::*;
use egui::{pos2, vec2, Align2, Id, Key, Margin, Order, Rect, Sense, Stroke, StrokeKind, Ui};

const EDGE: f32 = 14.0;
const ISLAND_H: f32 = 40.0;
const LEFT_W: f32 = 280.0;
const BOTTOM_H: f32 = 70.0;

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    let left = if st.panel_visible { 2.0 * EDGE + LEFT_W } else { EDGE };
    st.view_rect = [left, EDGE + ISLAND_H, screen.width() - EDGE, screen.height() - BOTTOM_H];
    top_bar(ctx, st, screen);
    summary(ctx, st, screen);
    hud(ctx, st);
    if st.panel_visible {
        inspector(ctx, st, screen);
    }
    files_dialog(ctx, st, screen);
    toast(ctx, st, screen);
    shortcuts(ctx, st);
}

// ------------------------------------------------------------- top bar

fn top_bar(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    egui::Area::new(Id::new("isl-doc")).order(Order::Middle).fixed_pos(screen.min + vec2(EDGE, EDGE)).show(ctx, |ui| {
        island().show(ui, |ui| {
            ui.set_height(ISLAND_H - 10.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (r, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                paint_logo(ui, Rect::from_center_size(r.center(), vec2(26.0, 26.0)));
                let title = if st.name.is_empty() { "Forme Rectify".to_string() } else { st.name.clone() };
                let sub = match &st.doc {
                    Some(d) => format!("· {} k triangles", d.mesh.tris.len() / 1000),
                    None => "· no mesh".to_string(),
                };
                let g = ui.painter().layout_no_wrap(title, semibold(13.5), TEXT());
                let s = ui.painter().layout_no_wrap(sub, regular(12.0), TEXT2());
                let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + s.size().x + 36.0, 28.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, 7.0, RAISED());
                }
                let gw = g.size().x;
                ui.painter().galley(pos2(r.min.x + 6.0, r.center().y - g.size().y * 0.5), g, TEXT());
                ui.painter().galley(pos2(r.min.x + 12.0 + gw, r.center().y - s.size().y * 0.5), s, TEXT2());
                icons::paint(ui.painter(), Rect::from_min_size(pos2(r.max.x - 18.0, r.center().y - 6.0), vec2(12.0, 12.0)), Icon::Chevron, TEXT2());
                if resp.on_hover_text("Open an .obj (Documents/Forme Rectify), or drop one on the window").clicked() {
                    st.files_open = !st.files_open;
                }
            });
        });
    });

    // centre: the tools and the component modes
    let cx = st.principal()[0];
    egui::Area::new(Id::new("isl-tools")).order(Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(pos2(cx, screen.min.y + EDGE)).show(ctx, |ui| {
        island().show(ui, |ui| {
            ui.set_height(ISLAND_H - 10.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let tools = [
                    (Tool::Select, Icon::Select, "Select: click to pick by mode (1 to 4); Shift + drag pulls what is under the pointer: a shape's thickness, an open edge out, a fillet; a drag turns around (Q)", "Q"),
                    (Tool::Pen, Icon::Sketch, "Pen: click points; the plane found under the first one is the one you draw on; click the first point to close and fill it, Enter for a line (P)", "P"),
                    (Tool::Circle, Icon::Cylinder, "Circle: drag across a round part, rim to rim, freely; released, it is found on the scan and made a cylinder, else a flat disc as drawn (C)", "C"),
                    (Tool::Slot, Icon::Capsule, "Slot: drag along a slot with round ends, a NURBS surface is laid on its wall, every row an exact slot outline (S)", "S"),
                    (Tool::Box, Icon::Box, "Box: drag across a block with rounded corners, raised or cut in, corner to corner: a NURBS surface on its wall, every row an exact rounded rectangle (B)", "B"),
                ];
                for (t, icon, tip, key) in tools {
                    let r = icon_btn(ui, icon, st.tool == t, tip, 28.0);
                    key_hint(ui, &r, key);
                    if r.clicked() {
                        st.set_tool(t);
                    }
                }
                island_sep(ui);
                let modes: Vec<(SelMode, &str, &str)> = SelMode::ALL.iter().map(|m| (*m, m.label(), m.key())).collect();
                if let Some(m) = seg_inline(ui, "isl-mode", &modes, st.mode) {
                    st.set_mode(m);
                }
                island_sep(ui);
                if icon_btn(ui, Icon::Undo, false, "Undo (Ctrl+Z)", 28.0).clicked() {
                    st.undo();
                }
                if icon_btn(ui, Icon::Redo, false, "Redo (Ctrl+Shift+Z)", 28.0).clicked() {
                    st.redo();
                }
                island_sep(ui);
                if chip(ui, None, "Zebra", st.settings.zebra, "Zebra stripes: flat and true surfaces give straight, even stripes (Z)").clicked() {
                    st.settings.zebra = !st.settings.zebra;
                }
                if icon_btn(ui, Icon::Grid, st.settings.grid, "Ground grid", 28.0).clicked() {
                    st.settings.grid = !st.settings.grid;
                }
            });
        });
    });

    // right: the view
    egui::Area::new(Id::new("isl-view")).order(Order::Middle).pivot(Align2::RIGHT_TOP).fixed_pos(pos2(screen.max.x - EDGE, screen.min.y + EDGE)).show(ctx, |ui| {
        island().show(ui, |ui| {
            ui.set_height(ISLAND_H - 10.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                use crate::camera::ViewPreset;
                let cur = st.camera.preset_of();
                let presets = [(ViewPreset::Perspective, "3D", ""), (ViewPreset::Front, "Front", ""), (ViewPreset::Right, "Right", ""), (ViewPreset::Top, "Top", "")];
                if let Some(p) = seg_inline(ui, "isl-preset", &presets, cur) {
                    st.camera.preset(p);
                }
                island_sep(ui);
                if icon_btn(ui, Icon::Frame, false, "Frame the mesh (F)", 28.0).clicked() {
                    st.frame_all();
                }
                if icon_btn(ui, Icon::Light, st.settings.light, "Light theme", 28.0).clicked() {
                    st.settings.light = !st.settings.light;
                }
            });
        });
    });
}

// --------------------------------------------------------------- summary

fn summary(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let cx = st.principal()[0];
    egui::Area::new(Id::new("summary")).order(Order::Middle).pivot(Align2::CENTER_BOTTOM).fixed_pos(pos2(cx, screen.max.y - 14.0)).interactable(false).show(ctx, |ui| {
        panel().inner_margin(Margin::symmetric(14, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let Some(d) = &st.doc else {
                    ui.label(egui::RichText::new("No mesh").font(regular(12.5)).color(TEXT2()));
                    return;
                };
                let (lo, hi) = d.mesh.bounds();
                let s = hi - lo;
                let parts = [
                    format!("{} × {} × {} mm", fmt(s.x, 1), fmt(s.y, 1), fmt(s.z, 1)),
                    format!("{} k vertices · edges {} mm", d.mesh.pos.len() / 1000, fmt(d.edge, 2)),
                    match st.objects.len() {
                        0 => "No shape yet".to_string(),
                        1 => "1 shape".to_string(),
                        n => format!("{n} shapes"),
                    },
                ];
                for (i, t) in parts.iter().enumerate() {
                    if i > 0 {
                        let (sep, _) = ui.allocate_exact_size(vec2(1.0, 16.0), Sense::hover());
                        ui.painter().rect_filled(sep, 0.0, DIVIDER());
                    }
                    ui.label(egui::RichText::new(t).font(regular(12.5)).color(if i < 2 { TEXT2() } else { TEXT() }));
                }
            });
        });
    });
}

fn hud(ctx: &egui::Context, st: &mut AppState) {
    let cx = st.principal()[0];
    let text = crate::viewport::hint(st);
    let color = if st.busy() { accent() } else { TEXT2() };
    egui::Area::new(Id::new("hud")).order(Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(pos2(cx, EDGE + ISLAND_H + 12.0)).interactable(false).show(ctx, |ui| {
        let g = ui.painter().layout_no_wrap(text, regular(12.0), LABEL());
        let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 40.0, 26.0), Sense::hover());
        let p = ui.painter();
        p.rect(r, 13.0, tint(PANEL(), 0.88), Stroke::new(1.0, PANEL_BORDER()), StrokeKind::Inside);
        p.circle_filled(pos2(r.min.x + 18.0, r.center().y), 4.0, color);
        p.galley(pos2(r.min.x + 30.0, r.center().y - g.size().y * 0.5), g, LABEL());
    });
}

fn toast(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let Some(t) = &st.toast else { return };
    let (text, warn) = (t.text.clone(), t.warn);
    let cx = st.principal()[0];
    egui::Area::new(Id::new("toast")).order(Order::Tooltip).pivot(Align2::CENTER_BOTTOM).fixed_pos(pos2(cx, screen.max.y - 92.0)).interactable(false).show(ctx, |ui| {
        let g = ui.painter().layout_no_wrap(text, medium(13.0), TEXT());
        let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 32.0, 36.0), Sense::hover());
        ui.painter().rect(r, 18.0, pick(egui::Color32::from_rgb(0x2A, 0x2D, 0x33), egui::Color32::WHITE), Stroke::new(1.0, if warn { ERROR } else { accent_border() }), StrokeKind::Inside);
        ui.painter().galley(pos2(r.min.x + 16.0, r.center().y - g.size().y * 0.5), g, TEXT());
    });
}

// ------------------------------------------------------------- inspector

fn inspector(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let top = EDGE + ISLAND_H + 10.0;
    let inner_w = LEFT_W - 24.0;
    let off = panel_offset(ctx, "props");
    let _ = screen;
    egui::Area::new(Id::new("props")).order(Order::Middle).fixed_pos(pos2(screen.min.x + EDGE, top) + off).show(ctx, |ui| {
        island().inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(inner_w);
            grip(ui, "props");
            tool_card(ui, st);
            ui.add_space(6.0);
            divider(ui);
            ui.add_space(6.0);
            objects_card(ui, st);
        });
    });
}

fn title(ui: &mut Ui, t: &str, sub: &str) {
    ui.label(egui::RichText::new(t).font(semibold(15.0)).color(TEXT()));
    if !sub.is_empty() {
        ui.label(egui::RichText::new(sub).font(regular(12.0)).color(TEXT2()));
    }
    ui.add_space(4.0);
}

fn line(ui: &mut Ui, t: impl Into<String>) {
    ui.label(egui::RichText::new(t.into()).font(regular(12.0)).color(TEXT()));
}

fn slider_f64(ui: &mut Ui, label: &str, v: &mut f64, range: std::ops::RangeInclusive<f32>, step: f64, suffix: &str) -> bool {
    let mut f = *v as f32;
    let (changed, _) = slider_row(ui, label, &mut f, range, step, suffix);
    if changed {
        *v = f as f64;
    }
    changed
}

fn tool_card(ui: &mut Ui, st: &mut AppState) {
    match st.tool {
        Tool::Select => select_card(ui, st),
        Tool::Pen => pen_card(ui, st),
        Tool::Circle => circle_card(ui, st),
        Tool::Slot | Tool::Box => slot_card(ui, st),
    }
    ui.add_space(4.0);
    let w = ui.available_width();
    if st.doc.is_some() && secondary_btn(ui, if st.hide_scan { "Show the scan (H)" } else { "Hide the scan (H)" }, w).clicked() {
        st.hide_scan = !st.hide_scan;
    }
}

/// Round sizes to pick from (mm).
const SIZES: [f64; 6] = [0.5, 1.0, 2.0, 3.0, 5.0, 10.0];

/// A row of round sizes; the one clicked.
fn size_chips(ui: &mut Ui, cur: f64, max: f64) -> Option<f64> {
    let mut out = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for s in SIZES.iter().copied().filter(|s| *s <= max.max(0.5) * 1.0001) {
            if chip(ui, None, &fmt(s, if s < 1.0 { 1 } else { 0 }), (cur - s).abs() < 1e-6, "").clicked() {
                out = Some(s);
            }
        }
    });
    out
}

fn slot_card(ui: &mut Ui, st: &mut AppState) {
    use rectify_core::circle::ProfileKind;
    let tool = st.tool;
    match tool {
        Tool::Box => title(ui, "Box", "Press near a corner of a block with rounded corners, raised or cut in, drag across it to the far corner: a NURBS surface is laid on its wall, every row an exact rounded rectangle"),
        _ => title(ui, "Slot", "Press at one end of a slot with round ends, drag along it to the other end: a NURBS surface is laid on its wall, every row an exact slot outline"),
    }
    let mut refit = false;
    let mut apply = false;
    let mut cancel = false;
    let mut new_profile = None;
    match &mut st.slot {
        None if tool == Tool::Box => line(ui, "The face around gives the axis, its walls its turn; its sides and corners are fitted on its wall, its height found from the face around."),
        None => line(ui, "The face it is cut in gives the axis; its width, length and ends are fitted on its wall, its depth found from the floor."),
        Some(se) => {
            let sl = &se.slot;
            let (kind, way) = if sl.inward { ("cut in", "deep") } else { ("raised", "high") };
            let round = sl.kind == Kind::Round;
            let size = if round { format!("Ø {}", fmt(sl.width(), 2)) } else { format!("{} × {}", fmt(sl.length(), 2), fmt(sl.width(), 2)) };
            let (kind, way) = if round { (if sl.inward { "a hole" } else { "a boss" }, way) } else { (kind, way) };
            line(ui, format!("{size} mm · {kind} · found {} mm {way}", fmt(sl.depth, 2)));
            if sl.kind == Kind::Box {
                line(ui, format!("Corners rounded {} mm", fmt(sl.r, 2)));
            }
            let max = (sl.depth * 3.0).max(sl.r * 2.0) as f32;
            refit |= slider_f64(ui, if sl.inward { "Depth" } else { "Height" }, &mut se.params.depth, 0.1..=max, 0.05, "mm");
            let items = [(ProfileKind::Cylinder, "Straight", accent()), (ProfileKind::Cone, "Tapered", accent()), (ProfileKind::Curve, "Curve", accent())];
            if let Some(k) = segmented(ui, &items, se.params.profile, 28.0) {
                new_profile = Some(k);
            }
            let mut lv = se.params.levels as f32;
            if slider_row(ui, "Levels", &mut lv, 1.0..=24.0, 1.0, "").0 {
                se.params.levels = lv.round().max(1.0) as usize;
                refit = true;
            }
            let s = &se.laid.surface;
            // each row's length: the far ends of its two round ends
            let e = (se.slot.b - se.slot.a).normalize_or_zero();
            let along = |j: usize| (0..s.nu).map(|i| (s.pts[j * s.nu + i] - se.slot.a).dot(e)).fold((f64::INFINITY, f64::NEG_INFINITY), |m, x| (m.0.min(x), m.1.max(x)));
            let (lo, hi) = (0..s.nv).map(|j| { let (a, b) = along(j); b - a }).fold((f64::INFINITY, 0.0f64), |a, w| (a.0.min(w), a.1.max(w)));
            match se.slot.kind {
                Kind::Box => line(ui, format!("{} rows, each four lines and four exact quarter circles · {} to {} mm long", se.params.levels + 1, fmt(lo, 2), fmt(hi, 2))),
                Kind::Slot => line(ui, format!("{} rows, each two lines and two exact half circles · {} to {} mm long", se.params.levels + 1, fmt(lo, 2), fmt(hi, 2))),
                Kind::Round => line(ui, format!("{} rings, each an exact circle · Ø {} to {} mm", se.params.levels + 1, fmt(lo, 2), fmt(hi, 2))),
            }
            line(ui, format!("NURBS degree 2 × {} · {} × {} control points", s.dv, s.nu, s.nv));
            line(ui, format!("{} points of the wall · the scan is {} mm from it", se.laid.points, fmt(se.laid.off, 3)));
            ui.add_space(6.0);
            let w = ui.available_width();
            let icon = match se.slot.kind {
                Kind::Box => Icon::Box,
                Kind::Round => Icon::Cylinder,
                Kind::Slot => Icon::Capsule,
            };
            apply = primary_btn(ui, Some(icon), "Keep (Enter)", Some(w)).clicked();
            cancel = secondary_btn(ui, "Remove (Esc)", w).clicked();
        }
    }
    if let Some(k) = new_profile {
        st.slot_set_profile(k);
    } else if refit {
        st.slot_refit();
    }
    if apply {
        st.slot_apply();
    }
    if cancel {
        st.slot_cancel();
    }
}


fn pen_card(ui: &mut Ui, st: &mut AppState) {
    title(ui, "Pen", "Click points on the scan: a straight outline on the plane found under the first one. Closed on its first point, it is filled; Enter makes a line");
    let mut finish = false;
    let mut back = false;
    let mut cancel = false;
    match &st.pen {
        None => {
            match &st.pen_plane {
                Some(h) if h.flat => line(ui, "Under the pointer: a flat face (blue grid)"),
                Some(_) => line(ui, "Under the pointer: no flat face, the scan's tangent plane (grey grid)"),
                None => line(ui, "Hover the scan: the plane you would draw on shows under the pointer"),
            }
            line(ui, "The faces' edges and corners catch the points (Ctrl: nothing catches); Shift: sides turned by 15°.");
            if let Some(i) = st.fresh {
                ui.add_space(6.0);
                fresh_card(ui, st, i);
            }
        }
        Some(pe) => {
            line(ui, format!("{} points · on {}", pe.pts.len(), if pe.flat { "a flat face" } else { "the tangent plane" }));
            if let Some(h) = &st.pen_hover {
                if let Some(last) = pe.pts.last() {
                    line(ui, format!("Next side {} mm{}", fmt((h.q - *last).length(), 2), if h.snap.label().is_empty() { String::new() } else { format!(" · {}", h.snap.label()) }));
                }
            }
            ui.add_space(6.0);
            let w = ui.available_width();
            if pe.pts.len() >= 2 {
                finish = primary_btn(ui, Some(Icon::Sketch), "Make a line (Enter)", Some(w)).clicked();
            }
            back = secondary_btn(ui, "Take the last point back (Backspace)", w).clicked();
            cancel = secondary_btn(ui, "Stop (Esc)", w).clicked();
        }
    }
    if finish {
        st.pen_finish();
    }
    if back {
        st.pen_back();
    }
    if cancel {
        st.pen_cancel();
    }
}

/// The shape a tool just made: Keep (Enter), Remove (Esc).
fn fresh_card(ui: &mut Ui, st: &mut AppState, i: usize) {
    let Some(o) = st.objects.get(i) else { return };
    line(ui, format!("Made: {}", o.what));
    line(ui, "Shift + drag on it: its thickness.");
    let w = ui.available_width();
    let keep = primary_btn(ui, None, "Keep (Enter)", Some(w)).clicked();
    let remove = secondary_btn(ui, "Remove (Esc)", w).clicked();
    if keep {
        st.keep_fresh();
    }
    if remove {
        st.remove_fresh();
    }
}

fn circle_card(ui: &mut Ui, st: &mut AppState) {
    title(ui, "Circle", "Press on the rim of a round part (a hole, a boss, a button, a rivet) and drag across it, freely. Released, the round part is found on the scan and made a cylinder its size and height");
    if let Some(cd) = &st.circle {
        line(ui, format!("Drawn: Ø {} mm · released, the scan is read under it", fmt((cd.b - cd.a).length(), 2)));
        return;
    }
    let Some(i) = st.fresh else {
        line(ui, "When nothing round is under it, a flat disc as drawn.");
        return;
    };
    let Some(b) = st.body(i).cloned() else { return };
    let Base::Circle { r } = b.base else { return };
    line(ui, b.describe());
    let mut d = r * 2.0;
    let max = (d * 3.0).max(5.0) as f32;
    let mut df = d as f32;
    let (changed, start) = slider_row(ui, "Ø", &mut df, 0.05..=max, 0.01, "mm");
    if changed {
        if start {
            st.end_live();
        }
        d = df as f64;
        st.set_radius(i, d * 0.5, true);
    }
    let t = b.thick().map(|s| s.1).unwrap_or(0.0);
    let mut tf = t as f32;
    let tmax = (t.abs() * 3.0).max(d * 2.0).max(5.0) as f32;
    let (changed, start) = slider_row(ui, "Height", &mut tf, -tmax..=tmax, 0.01, "mm");
    if changed {
        if start {
            st.end_live();
        }
        st.set_thickness(i, tf as f64, true);
    }
    ui.add_space(6.0);
    fresh_card(ui, st, i);
}

fn select_card(ui: &mut Ui, st: &mut AppState) {
    let Some(first) = st.sel.first().cloned() else {
        title(ui, "Select", "Pick by mode: 1 Vertex · 2 Edge · 3 Face · 4 Object (the pill at the top)");
        line(ui, "Shift + drag pulls what is under the pointer:");
        line(ui, "· a shape or a face: its thickness");
        line(ui, "· an open edge (orange): pulled out into a new face, on the scan under the pointer (Ctrl: in the air)");
        line(ui, "· an edge between two faces: a fillet");
        line(ui, "· a corner of the outline (Vertex): rounded");
        line(ui, "Shift + click adds to what is picked.");
        return;
    };
    let obj = first.obj();
    let Some(o) = st.objects.get(obj) else { return };
    let what = o.what.clone();
    let name = format!("{} {}", o.tool.label(), obj + 1);
    match first {
        Comp::Object(_) => {
            let n = st.sel.len();
            title(ui, if n > 1 { "Objects" } else { &name }, &if n > 1 { format!("{n} picked") } else { what });
            if let Some(b) = st.body(obj).cloned() {
                let t = b.thick().map(|s| s.1).unwrap_or(0.0);
                let mut tf = t as f32;
                let tmax = (t.abs() * 3.0).max(b.built.size * 0.5).max(5.0) as f32;
                let (changed, start) = slider_row(ui, "Thickness", &mut tf, -tmax..=tmax, 0.01, "mm");
                if changed {
                    if start {
                        st.end_live();
                    }
                    st.set_thickness(obj, tf as f64, true);
                }
                steps_list(ui, st, obj);
            }
            ui.add_space(6.0);
            let w = ui.available_width();
            if secondary_btn(ui, "Delete (Del)", w).clicked() {
                st.delete_selected();
            }
        }
        Comp::Corner { .. } => {
            let Some((obj, cs)) = st.picked_corners() else { return };
            let Some(b) = st.body(obj).cloned() else { return };
            title(ui, if cs.len() == 1 { "Corner" } else { "Corners" }, &format!("{} of the outline of {name}", cs.len()));
            let cur = cs.iter().map(|&i| b.corners.get(i).copied().unwrap_or(0.0)).fold(0.0, f64::max);
            let max = cs.iter().map(|&i| b.corner_max(i)).fold(f64::INFINITY, f64::min);
            if !max.is_finite() || max <= 1e-6 {
                line(ui, "These corners cannot be rounded (an end of a line, or a straight corner).");
                return;
            }
            let mut rf = cur as f32;
            let (changed, start) = slider_row(ui, "Radius", &mut rf, 0.0..=max as f32, 0.01, "mm");
            if changed {
                if start {
                    st.end_live();
                }
                st.set_corners(rf as f64, true);
            }
            if let Some(s) = size_chips(ui, cur, max) {
                st.end_live();
                st.set_corners(s, false);
            }
            line(ui, format!("At most {} mm here (each side gives half of itself)", fmt(max, 2)));
        }
        Comp::Edge { .. } => {
            let Some((obj, edges)) = st.picked_edges() else { return };
            let open = edges.iter().filter(|e| !e.1).count();
            let closed = edges.len() - open;
            title(ui, if edges.len() == 1 { "Edge" } else { "Edges" }, &format!("{} of {name}", edges.len()));
            if open > 0 {
                line(ui, format!("{open} open (a face on one side only): Shift + drag pulls it out into a new face, on the scan under the pointer (Ctrl: in the air)."));
            }
            if closed > 0 {
                line(ui, format!("{closed} between two faces: a fillet"));
                let mut r = 0.0f32;
                let (changed, _) = slider_row(ui, "Fillet", &mut r, 0.0..=5.0, 0.01, "mm");
                if changed && r > 0.0 {
                    st.end_live();
                    st.make_fillet(r as f64);
                } else if let Some(s) = size_chips(ui, 0.0, 10.0) {
                    st.end_live();
                    st.make_fillet(s);
                    st.end_live();
                }
            }
            let _ = obj;
        }
        Comp::Step { k, .. } => {
            let Some(b) = st.body(obj).cloned() else { return };
            let Some(s) = b.steps.get(k).cloned() else { return };
            title(ui, s.label(), &format!("Step {} of {name}", k + 1));
            step_slider(ui, st, obj, k, &s, &b);
            ui.add_space(6.0);
            let w = ui.available_width();
            if secondary_btn(ui, "Remove this step (Del)", w).clicked() {
                st.remove_step(obj, k);
            }
        }
        Comp::Face { tag, .. } => {
            title(ui, "Face", &format!("of {name}"));
            let step = crate::body::step_of(tag).and_then(|k| st.body(obj).and_then(|b| b.steps.get(k)).map(|s| s.label()));
            line(ui, match step {
                Some(l) => format!("Made by: {l}"),
                None if tag == crate::body::BASE => "The outline's face".to_string(),
                None => "A face of the thickness".to_string(),
            });
            line(ui, "Shift + drag on it: the shape's thickness.");
        }
        Comp::Vertex { at, .. } => {
            title(ui, "Vertex", &format!("of {name}"));
            line(ui, format!("{} · {} · {} mm", fmt(at.x, 2), fmt(at.y, 2), fmt(at.z, 2)));
            line(ui, "Made by a step: only the outline's corners round (or fillet its edges in Edge mode).");
        }
    }
}

/// One step's value as a slider.
fn step_slider(ui: &mut Ui, st: &mut AppState, obj: usize, k: usize, s: &Step, b: &crate::body::Body) {
    let (label, v, lo, hi) = match s {
        Step::Fillet { r, .. } => ("Radius", *r, 0.0, (r * 3.0).max(5.0)),
        Step::Thick { t } => ("Thickness", *t, -(t.abs() * 3.0).max(5.0), (t.abs() * 3.0).max(5.0)),
        Step::Extrude { v, .. } => ("Length", v.length(), 0.01, (v.length() * 3.0).max(5.0)),
    };
    let mut f = v as f32;
    let (changed, start) = slider_row(ui, label, &mut f, lo as f32..=hi as f32, 0.01, "mm");
    if changed {
        if start {
            st.end_live();
        }
        st.set_step(obj, k, f as f64, true);
    }
    if let Step::Fillet { r, edges } = s {
        if let Some(c) = size_chips(ui, *r, 10.0) {
            st.end_live();
            st.set_step(obj, k, c, false);
        }
        line(ui, format!("{} edge{}", edges.len(), if edges.len() == 1 { "" } else { "s" }));
    }
    if let Some((_, lim)) = b.built.limits.iter().find(|l| l.0 == k) {
        line(ui, format!("Too big: built at {} mm, the largest that works", fmt(*lim, 2)));
    }
    if let Some((kk, e)) = &b.built.failed {
        if *kk == k {
            ui.label(egui::RichText::new(format!("Not made: {e}")).font(regular(12.0)).color(ERROR));
        }
    }
}

/// A body's steps, each with its value, to pick or remove.
fn steps_list(ui: &mut Ui, st: &mut AppState, obj: usize) {
    let Some(b) = st.body(obj).cloned() else { return };
    if b.steps.is_empty() {
        line(ui, "No steps yet: Shift + drag thickens it; Edge mode (2) pulls or fillets its edges; Vertex mode (1) rounds its corners.");
        return;
    }
    ui.add_space(4.0);
    let mut pick = None;
    let mut remove = None;
    for (k, s) in b.steps.iter().enumerate() {
        let v = match s {
            Step::Fillet { r, .. } => format!("{} mm", fmt(*r, 2)),
            Step::Thick { t } => format!("{} mm", fmt(*t, 2)),
            Step::Extrude { v, .. } => format!("{} mm", fmt(v.length(), 2)),
        };
        let failed = b.built.failed.as_ref().map(|f| f.0 <= k && f.0 != usize::MAX).unwrap_or(false);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(egui::RichText::new(format!("{}", k + 1)).font(mono(10.5)).color(TEXT2()));
            if ui.add(egui::Label::new(egui::RichText::new(s.label()).font(medium(12.0)).color(if failed { ERROR } else { accent_text() })).sense(Sense::click())).clicked() {
                pick = Some(k);
            }
            ui.label(egui::RichText::new(v).font(regular(12.0)).color(TEXT()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_btn(ui, Icon::Trash, false, "Remove this step", 20.0).clicked() {
                    remove = Some(k);
                }
            });
        });
    }
    if let Some(k) = remove {
        st.remove_step(obj, k);
    } else if let Some(k) = pick {
        st.mode = SelMode::Face;
        st.sel = vec![Comp::Step { obj, k }];
        st.objects_gen += 1;
    }
}

fn objects_card(ui: &mut Ui, st: &mut AppState) {
    title(ui, "Shapes", "");
    if st.objects.is_empty() {
        line(ui, "Nothing made yet.");
        return;
    }
    let mut pick = None;
    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
        for (i, o) in st.objects.iter().enumerate() {
            let on = st.sel.iter().any(|c| c.obj() == i);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(egui::RichText::new(format!("{}", i + 1)).font(mono(10.5)).color(TEXT2()));
                let r = ui.add(egui::Label::new(egui::RichText::new(o.tool.label()).font(medium(12.0)).color(if on { TEXT() } else { accent_text() })).sense(Sense::click()));
                let r2 = ui.add(egui::Label::new(egui::RichText::new(&o.what).font(regular(12.0)).color(if on { TEXT() } else { TEXT2() })).truncate().sense(Sense::click())).on_hover_text(&o.what);
                if r.clicked() || r2.clicked() {
                    pick = Some(i);
                }
            });
        }
    });
    if let Some(i) = pick {
        st.select_object(i);
    }
}

// ---------------------------------------------------------------- files

fn files_dialog(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    if !st.files_open {
        return;
    }
    if dim(ctx, screen, "files-dim") {
        st.files_open = false;
        return;
    }
    egui::Area::new(Id::new("files")).order(Order::Foreground).pivot(Align2::CENTER_CENTER).fixed_pos(screen.center()).show(ctx, |ui| {
        panel().inner_margin(Margin::same(16)).show(ui, |ui| {
            ui.set_width(460.0);
            title(ui, "Open a mesh", &format!("{}", crate::files::documents_dir().display()));
            let list = crate::files::list();
            if list.is_empty() {
                line(ui, "No .obj here yet: put one in this folder, or drop it on the window");
            }
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (name, path, size) in list {
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 7.0, RAISED());
                    }
                    ui.painter().text(pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, &name, medium(13.0), TEXT());
                    ui.painter().text(pos2(r.max.x - 10.0, r.center().y), Align2::RIGHT_CENTER, format!("{} Mo", size / 1_000_000), regular(12.0), TEXT2());
                    if resp.clicked() {
                        st.files_open = false;
                        st.open(path);
                    }
                }
            });
        });
    });
}


// ------------------------------------------------------------ shortcuts

fn shortcuts(ctx: &egui::Context, st: &mut AppState) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
    let key = |k: Key| ctx.input(|i| i.key_pressed(k));
    let (f, esc, z, y, enter) = (key(Key::F), key(Key::Escape), key(Key::Z), key(Key::Y), key(Key::Enter));
    let (back, del) = (key(Key::Backspace), key(Key::Delete));
    if cmd && z {
        if shift {
            st.redo();
        } else {
            st.undo();
        }
    } else if cmd && y {
        st.redo();
    } else if z {
        st.settings.zebra = !st.settings.zebra;
    }
    if !cmd {
        for (k, t) in [(Key::Q, Tool::Select), (Key::P, Tool::Pen), (Key::C, Tool::Circle), (Key::S, Tool::Slot), (Key::B, Tool::Box)] {
            if key(k) {
                st.set_tool(t);
            }
        }
        for (k, m) in [(Key::Num1, SelMode::Vertex), (Key::Num2, SelMode::Edge), (Key::Num3, SelMode::Face), (Key::Num4, SelMode::Object)] {
            if key(k) && st.pen.is_none() {
                st.set_mode(m);
            }
        }
        if key(Key::H) && st.doc.is_some() {
            st.hide_scan = !st.hide_scan;
        }
    }
    if back {
        if st.pen.is_some() {
            st.pen_back();
        } else {
            st.delete_selected();
        }
    } else if del {
        st.delete_selected();
    }
    if enter {
        if st.pen.is_some() {
            st.pen_finish();
        } else if st.slot.is_some() {
            st.slot_apply();
        } else if st.fresh.is_some() {
            st.keep_fresh();
        } else {
            st.end_live();
        }
    }
    if f {
        st.frame_all();
    }
    if esc {
        st.files_open = false;
        if st.gesture.is_some() {
            st.gesture = None;
            st.press = None;
            st.cancel_live();
        } else if st.pen.is_some() {
            st.pen_cancel();
        } else if st.circle.is_some() || st.slot_drag.is_some() {
            st.circle = None;
            st.slot_drag = None;
            st.overlay_gen += 1;
        } else if st.slot.is_some() {
            st.slot_cancel();
        } else if st.fresh.is_some() {
            st.remove_fresh();
        } else if !st.sel.is_empty() {
            st.sel.clear();
            st.objects_gen += 1;
            st.overlay_gen += 1;
        } else if st.tool != Tool::Select {
            st.set_tool(Tool::Select);
        }
    }
}
