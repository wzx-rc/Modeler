//! The interface (Forme NURBS's islands, in orange): the file, the tools and
//! the view at the top, the tool's settings on the left, a summary at the
//! bottom.

use crate::icons::{self, Icon};
use crate::state::*;
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

    // centre: the tools
    let cx = st.principal()[0];
    egui::Area::new(Id::new("isl-tools")).order(Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(pos2(cx, screen.min.y + EDGE)).show(ctx, |ui| {
        island().show(ui, |ui| {
            ui.set_height(ISLAND_H - 10.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let tools = [(Tool::Navigate, Icon::Select, "Navigate: the left button turns around the scan (Esc)", "Esc"), (Tool::Circle, Icon::Cylinder, "Circle: drag across a hole or a boss, a NURBS surface is laid on its wall, every ring an exact circle (C)", "C"), (Tool::Slot, Icon::Capsule, "Slot: drag along a slot with round ends, a NURBS surface is laid on its wall, every row an exact slot outline (S)", "S"), (Tool::Box, Icon::Box, "Box: drag across a block with rounded corners, raised or cut in, corner to corner: a NURBS surface on its wall, every row an exact rounded rectangle (B)", "B"), (Tool::Path, Icon::Sketch, "Path: click points on the scan, one NURBS curve through them; flat faces, their edges and corners catch the points (P)", "P")];
                for (t, icon, tip, key) in tools {
                    let r = icon_btn(ui, icon, st.tool == t, tip, 28.0);
                    key_hint(ui, &r, key);
                    if r.clicked() {
                        st.set_tool(t);
                    }
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
                        0 => "No NURBS yet".to_string(),
                        1 => "1 NURBS object".to_string(),
                        n => format!("{n} NURBS objects"),
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
        Tool::Navigate => {
            title(ui, "Tools", "A retopology in NURBS, laid on the scan; the scan itself never changes");
            let w = ui.available_width();
            if secondary_btn(ui, "Circle (C): a round part", w).clicked() {
                st.set_tool(Tool::Circle);
            }
            if secondary_btn(ui, "Slot (S): a slot with round ends", w).clicked() {
                st.set_tool(Tool::Slot);
            }
            if secondary_btn(ui, "Box (B): a block with rounded corners", w).clicked() {
                st.set_tool(Tool::Box);
            }
            if secondary_btn(ui, "Path (P): lines and curves on the scan", w).clicked() {
                st.set_tool(Tool::Path);
            }
        }
        Tool::Path => path_card(ui, st),
        Tool::Slot | Tool::Box | Tool::Circle => slot_card(ui, st),
    }
    ui.add_space(4.0);
    let w = ui.available_width();
    if st.doc.is_some() && secondary_btn(ui, if st.hide_scan { "Show the scan (H)" } else { "Hide the scan (H)" }, w).clicked() {
        st.hide_scan = !st.hide_scan;
    }
}

fn slot_card(ui: &mut Ui, st: &mut AppState) {
    use rectify_core::circle::ProfileKind;
    let tool = st.tool;
    match tool {
        Tool::Box => title(ui, "Box", "Press near a corner of a block with rounded corners, raised or cut in, drag across it to the far corner: a NURBS surface is laid on its wall, every row an exact rounded rectangle"),
        Tool::Circle => title(ui, "Circle", "Press on the rim of a round part (a hole, a boss, a button, a rivet), drag across it to the other side: a NURBS surface is laid on its wall, every ring an exact circle"),
        _ => title(ui, "Slot", "Press at one end of a slot with round ends, drag along it to the other end: a NURBS surface is laid on its wall, every row an exact slot outline"),
    }
    let mut refit = false;
    let mut apply = false;
    let mut cancel = false;
    let mut new_profile = None;
    match &mut st.slot {
        None if tool == Tool::Box => line(ui, "The face around gives the axis, its walls its turn; its sides and corners are fitted on its wall, its height found from the face around."),
        None if tool == Tool::Circle => line(ui, "The face around gives the axis; the centre and the radius are fitted on the wall, its height or depth found from the face around and the top or the floor."),
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

fn path_card(ui: &mut Ui, st: &mut AppState) {
    use rectify_core::path::{PieceKind, SnapKind};
    title(ui, "Path", "Click points on the scan: one NURBS curve through them. Flat faces catch the points, and so do their edges (where two faces' planes meet, sharp) and corners");
    let mut p = st.path.as_ref().map(|pe| pe.params).unwrap_or(st.path_params);
    let mut changed = false;
    if toggle_row(ui, "Catch faces, edges, corners", p.planes) {
        p.planes = !p.planes;
        changed = true;
    }
    let mut sm = p.smooth as f32;
    if slider_row(ui, "Smooth", &mut sm, 0.0..=1.0, 0.05, "").0 {
        p.smooth = sm as f64;
        changed = true;
    }
    let n = st.path.as_ref().map(|pe| pe.pts.len()).unwrap_or(0);
    if n >= 3 && toggle_row(ui, "Closed", p.closed) {
        p.closed = !p.closed;
        changed = true;
    }
    if changed {
        st.path_set(p);
    }
    let mut apply = false;
    let mut back = false;
    let mut cancel = false;
    match &st.path {
        None => line(ui, "Between two points on one face the path is straight on it; from a face to the next it folds over their edge; elsewhere it follows the scan, smooth. Ctrl held: the scan only."),
        Some(pe) => {
            let count = |k: SnapKind| pe.pts.iter().filter(|s| s.kind == k).count();
            if pe.params.planes {
                line(ui, format!("{n} points: {} on corners, {} on edges, {} on faces, {} on the scan", count(SnapKind::Corner), count(SnapKind::Edge), count(SnapKind::Plane), count(SnapKind::Scan)));
            } else {
                line(ui, format!("{n} points on the scan"));
            }
            if let Some(b) = &pe.built {
                let k = |x: PieceKind| b.kinds.iter().filter(|&&y| y == x).count();
                let mut parts = vec![];
                if k(PieceKind::Line) > 0 {
                    parts.push(format!("{} straight on a face", k(PieceKind::Line)));
                }
                if k(PieceKind::Fold) > 0 {
                    parts.push(format!("{} folded over an edge", k(PieceKind::Fold)));
                }
                if k(PieceKind::Scan) > 0 {
                    parts.push(format!("{} on the scan", k(PieceKind::Scan)));
                }
                line(ui, format!("Segments: {}", parts.join(", ")));
                line(ui, format!("NURBS degree 3 · {} control points · {} mm{}", b.curve.pts.len(), fmt(b.length, 1), if pe.params.closed { " · closed" } else { "" }));
                if k(PieceKind::Scan) > 0 {
                    line(ui, format!("The scan is {} mm from the parts that follow it", fmt(b.off, 3)));
                }
            }
            ui.add_space(6.0);
            let w = ui.available_width();
            apply = n >= 2 && primary_btn(ui, Some(Icon::Sketch), "Keep (Enter)", Some(w)).clicked();
            back = secondary_btn(ui, "Remove the last point (Backspace)", w).clicked();
            cancel = secondary_btn(ui, "Remove (Esc)", w).clicked();
        }
    }
    if apply {
        st.path_apply();
    }
    if back {
        st.path_back();
    }
    if cancel {
        st.path_cancel();
    }
}

fn objects_card(ui: &mut Ui, st: &mut AppState) {
    title(ui, "NURBS", "");
    if st.objects.is_empty() && st.undone.is_empty() {
        line(ui, "Nothing made yet.");
        return;
    }
    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
        for (i, o) in st.objects.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(egui::RichText::new(format!("{}", i + 1)).font(mono(10.5)).color(TEXT2()));
                ui.label(egui::RichText::new(o.tool.label()).font(medium(12.0)).color(accent_text()));
                ui.label(egui::RichText::new(&o.what).font(regular(12.0)).color(TEXT()));
            });
        }
        for o in st.undone.iter().rev() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(egui::RichText::new("·").font(mono(10.5)).color(TEXT2()));
                ui.label(egui::RichText::new(o.tool.label()).font(medium(12.0)).color(TEXT2()));
                ui.label(egui::RichText::new(format!("{} (undone)", o.what)).font(regular(12.0)).color(TEXT2()));
            });
        }
    });
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
    let (f, esc, z, y, enter) = ctx.input(|i| (i.key_pressed(Key::F), i.key_pressed(Key::Escape), i.key_pressed(Key::Z), i.key_pressed(Key::Y), i.key_pressed(Key::Enter)));
    let (c, h, pk, back) = ctx.input(|i| (i.key_pressed(Key::C), i.key_pressed(Key::H), i.key_pressed(Key::P), i.key_pressed(Key::Backspace) || i.key_pressed(Key::Delete)));
    let (sk, bk) = ctx.input(|i| (i.key_pressed(Key::S), i.key_pressed(Key::B)));
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
    if c && !cmd {
        st.set_tool(Tool::Circle);
    }
    if pk && !cmd {
        st.set_tool(Tool::Path);
    }
    if sk && !cmd {
        st.set_tool(Tool::Slot);
    }
    if bk && !cmd {
        st.set_tool(Tool::Box);
    }
    if back && st.path.is_some() {
        st.path_back();
    }
    if h && !cmd && st.doc.is_some() {
        st.hide_scan = !st.hide_scan;
    }
    if enter && st.tool == Tool::Path {
        st.path_apply();
    }
    if enter && matches!(st.tool, Tool::Slot | Tool::Box | Tool::Circle) {
        st.slot_apply();
    }
    if f {
        st.frame_all();
    }
    if esc && st.path.is_some() {
        st.path_cancel();
    } else if esc && st.slot.is_some() && st.slot_drag.is_none() {
        st.slot_cancel();
    } else if esc {
        if st.slot_drag.is_some() {
            st.slot_drag = None;
            st.overlay_gen += 1;
        } else {
            st.set_tool(Tool::Navigate);
        }
        st.files_open = false;
    }
}
