//! The interface (Forme NURBS's islands, in orange): the file and the view
//! mode at the top, the five steps as cards on the right, their settings and
//! the picked region on the left, a summary at the bottom.

use crate::icons::{self, Icon};
use crate::state::*;
use crate::theme::*;
use crate::widgets::*;
use egui::{pos2, vec2, Align2, Id, Key, Margin, Order, Rect, Sense, Stroke, StrokeKind, Ui};
use rectify_core::{Kind, Prim, Step};

const EDGE: f32 = 14.0;
const ISLAND_H: f32 = 40.0;
const LEFT_W: f32 = 300.0;
const CARDS_W: f32 = 168.0;
const BOTTOM_H: f32 = 70.0;

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    let left = if st.panel_visible { 2.0 * EDGE + LEFT_W } else { EDGE };
    st.view_rect = [left, EDGE + ISLAND_H, screen.width() - 2.0 * EDGE - CARDS_W, screen.height() - BOTTOM_H];
    top_bar(ctx, st, screen);
    rail(ctx, st, screen);
    summary(ctx, st, screen);
    hud(ctx, st);
    if st.panel_visible {
        inspector(ctx, st, screen);
    }
    files_dialog(ctx, st, screen);
    toast(ctx, st, screen);
    shortcuts(ctx, st);
}

fn fmt(v: f64, digits: usize) -> String {
    format!("{:.*}", digits, v).replace('.', ",")
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
                let sub = match &st.pipe {
                    Some(p) => format!("· {} k triangles", p.original.tris.len() / 1000),
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

    // centre: what the view shows
    let cx = st.principal()[0];
    egui::Area::new(Id::new("isl-modes")).order(Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(pos2(cx, screen.min.y + EDGE)).show(ctx, |ui| {
        island().show(ui, |ui| {
            ui.set_height(ISLAND_H - 10.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let modes = [ViewMode::Original, ViewMode::Regions, ViewMode::Result];
                let (well, _) = ui.allocate_exact_size(vec2(3.0 * 86.0 + 6.0, 28.0), Sense::hover());
                ui.painter().rect(well, 14.0, WELL(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
                for (i, m) in modes.into_iter().enumerate() {
                    let r = Rect::from_min_size(well.min + vec2(3.0 + i as f32 * 86.0, 3.0), vec2(86.0, 22.0));
                    let resp = ui.interact(r, Id::new(("isl-mode", i)), Sense::click());
                    let on = st.shown() == m;
                    if on {
                        ui.painter().rect_filled(r, 11.0, accent());
                    } else if resp.hovered() {
                        ui.painter().rect_filled(r, 11.0, RAISED());
                    }
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, m.label().to_uppercase(), mono(11.0), if on { on_accent() } else { TEXT2() });
                    let tip = match m {
                        ViewMode::Original => "The mesh as it came (1, or hold Space)",
                        ViewMode::Regions => "The regions found: planes in cool colours, cylinders warm, free faces dark (2)",
                        ViewMode::Result => "The rectified mesh (3)",
                    };
                    if resp.on_hover_text(tip).clicked() {
                        st.set_mode(m);
                    }
                }
                island_sep(ui);
                if icon_btn(ui, Icon::Fillet, st.settings.edges, "Lines where the regions meet", 28.0).clicked() {
                    st.settings.edges = !st.settings.edges;
                    st.gen += 1;
                }
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
                let can = st.pipe.as_ref().map(|p| p.result.is_some()).unwrap_or(false);
                if icon_btn(ui, Icon::Export, false, "Export the result as .obj, with the original UVs (Documents/Forme Rectify)", 28.0).clicked() && can {
                    st.export();
                }
                if icon_btn(ui, Icon::Light, st.settings.light, "Light theme", 28.0).clicked() {
                    st.settings.light = !st.settings.light;
                }
            });
        });
    });
}

// ------------------------------------------------------------ step cards

fn step_icon(s: Step) -> Icon {
    match s {
        Step::Symmetry => Icon::Mirror,
        Step::Smooth => Icon::Sphere,
        Step::Segment => Icon::Quads,
        Step::Regularize => Icon::Grid,
        Step::Snap => Icon::Magnet,
    }
}

fn card(ui: &mut Ui, n: usize, icon: Icon, name: &str, sub: &str, on: bool, sub_color: egui::Color32, progress: Option<f32>, tip: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(CARDS_W - 12.0, 40.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect(r, 7.0, accent_bg(), Stroke::new(1.0, accent()), StrokeKind::Inside);
    } else {
        p.rect(r, 7.0, if resp.hovered() { RAISED() } else { tint(RAISED(), 0.35) }, Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
    }
    let ir = Rect::from_min_size(pos2(r.min.x + 7.0, r.center().y - 12.0), vec2(24.0, 24.0));
    p.rect(ir, 6.0, if on { tint(accent(), 0.18) } else { WELL() }, Stroke::new(1.0, if on { accent_border() } else { DIVIDER() }), StrokeKind::Inside);
    icons::paint(p, ir.shrink(5.0), icon, if on { accent() } else { TEXT_ICON() });
    p.text(pos2(ir.max.x + 8.0, r.center().y - 7.0), Align2::LEFT_CENTER, format!("{n} {name}"), mono(11.0), if on { accent_text() } else { TEXT() });
    p.text(pos2(ir.max.x + 8.0, r.center().y + 7.5), Align2::LEFT_CENTER, sub, mono(9.0), sub_color);
    if let Some(f) = progress {
        let bar = Rect::from_min_size(pos2(r.min.x + 6.0, r.max.y - 4.0), vec2((r.width() - 12.0) * f.clamp(0.0, 1.0), 2.0));
        p.rect_filled(bar, 1.0, accent());
    }
    resp.on_hover_text(tip)
}

fn rail(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let top = EDGE + ISLAND_H + 10.0;
    let off = panel_offset(ctx, "rail");
    egui::Area::new(Id::new("rail")).order(Order::Middle).pivot(Align2::RIGHT_TOP).fixed_pos(pos2(screen.max.x - EDGE, top) + off).show(ctx, |ui| {
        island().inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.set_width(CARDS_W - 12.0);
            grip(ui, "rail");
            ui.spacing_mut().item_spacing.y = 5.0;
            for (i, s) in Step::ALL.into_iter().enumerate() {
                let running = st.job.as_ref().and_then(|j| j.step) == Some(s);
                let done = st.pipe.as_ref().and_then(|p| p.times[s.index()]);
                let (sub, col) = if running {
                    (format!("RUNNING {:.0} %", st.job.as_ref().map(|j| j.progress * 100.0).unwrap_or(0.0)), accent())
                } else if let Some(t) = done {
                    (format!("DONE · {} s", fmt(t, 1)), OK)
                } else if !step_on(st, s) {
                    ("OFF".to_string(), TEXT2())
                } else {
                    ("TO RUN".to_string(), TEXT2())
                };
                let tip = match s {
                    Step::Symmetry => "Finds the mirror plane and averages both sides: half of the noise goes",
                    Step::Smooth => "Smoothing that keeps the edges: flat areas flatten, edges tighten",
                    Step::Segment => "Cuts the mesh into regions, each a plane, a cylinder or a sphere",
                    Step::Regularize => "Makes the shapes agree: parallel, square, on the object's axes, same radii, coaxial, mirrored",
                    Step::Snap => "Puts the vertices on their shapes; edges where two shapes meet",
                };
                let progress = if running { st.job.as_ref().map(|j| j.progress) } else { None };
                if card(ui, i + 1, step_icon(s), &s.label().to_uppercase(), &sub, st.step == s, col, progress, tip).clicked() {
                    st.step = s;
                    st.panel_visible = true;
                }
            }
            ui.add_space(2.0);
            divider(ui);
            ui.add_space(2.0);
            let label = if st.busy() { "RUNNING…" } else { "RUN ALL" };
            if primary_btn(ui, Some(Icon::Magnet), label, Some(CARDS_W - 12.0)).on_hover_text("Run every step (R)").clicked() {
                st.run_all();
            }
        });
    });
}

fn step_on(st: &AppState, s: Step) -> bool {
    match s {
        Step::Symmetry => st.params.symmetry,
        Step::Smooth => st.params.smooth_on,
        Step::Regularize => st.params.reg_on,
        _ => true,
    }
}

// --------------------------------------------------------------- summary

fn summary(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let cx = st.principal()[0];
    egui::Area::new(Id::new("summary")).order(Order::Middle).pivot(Align2::CENTER_BOTTOM).fixed_pos(pos2(cx, screen.max.y - 14.0)).interactable(false).show(ctx, |ui| {
        panel().inner_margin(Margin::symmetric(14, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let Some(p) = &st.pipe else {
                    ui.label(egui::RichText::new("No mesh").font(regular(12.5)).color(TEXT2()));
                    return;
                };
                let (lo, hi) = p.original.bounds();
                let d = hi - lo;
                let mut parts = vec![format!("{} × {} × {} mm", fmt(d.x, 1), fmt(d.y, 1), fmt(d.z, 1)), format!("{} k vertices", p.original.pos.len() / 1000)];
                if let Some(s) = p.seg_final.as_ref().or(p.seg.as_ref()).or(p.seg_raw.as_ref()) {
                    let areas = p.smooth_out().areas();
                    parts.push(format!("{} planes · {} cylinders · {} spheres", s.count(Kind::Plane), s.count(Kind::Cylinder), s.count(Kind::Sphere)));
                    parts.push(format!("{:.0} % on a shape", s.covered(&areas) * 100.0));
                }
                if let Some(r) = &p.snap {
                    parts.push(format!("moved {} mm on average", fmt(r.mean_move, 2)));
                }
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
    let total = screen.max.y - EDGE - top;
    let off = panel_offset(ctx, "props");
    egui::Area::new(Id::new("props")).order(Order::Middle).fixed_pos(pos2(screen.min.x + EDGE, top) + off).show(ctx, |ui| {
        island().inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(inner_w);
            grip(ui, "props");
            let _ = total;
            if st.selected.is_some() {
                region_card(ui, st);
                ui.add_space(6.0);
                divider(ui);
                ui.add_space(6.0);
            }
            step_card(ui, st);
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

fn slider_usize(ui: &mut Ui, label: &str, v: &mut usize, range: std::ops::RangeInclusive<f32>) -> bool {
    let mut f = *v as f32;
    let (changed, _) = slider_row(ui, label, &mut f, range, 1.0, "");
    if changed {
        *v = f.round() as usize;
    }
    changed
}

fn step_card(ui: &mut Ui, st: &mut AppState) {
    let s = st.step;
    let before = st.params;
    let p = &mut st.params;
    let pipe = st.pipe.as_ref();
    match s {
        Step::Symmetry => {
            title(ui, "1 · Symmetry", "The mirror plane, then each side averaged with the other");
            if toggle_row(ui, "Mirror", p.symmetry) {
                p.symmetry = !p.symmetry;
            }
            slider_f64(ui, "Match", &mut p.sym_tol, 0.05..=3.0, 0.05, "mm");
            if let Some(r) = pipe.and_then(|p| p.sym.as_ref()) {
                let n = r.mirror.n;
                line(ui, format!("Plane normal {} {} {}", fmt(n.x, 3), fmt(n.y, 3), fmt(n.z, 3)));
                line(ui, format!("{:.0} % of the points have a partner · gap {} mm", r.matched * 100.0, fmt(r.median, 2)));
                if r.matched <= 0.5 {
                    line(ui, "Not symmetric enough: left as it is");
                }
            }
        }
        Step::Smooth => {
            title(ui, "2 · Smooth", "Edge-preserving: bilateral filter on the normals");
            if toggle_row(ui, "Smooth", p.smooth_on) {
                p.smooth_on = !p.smooth_on;
            }
            slider_f64(ui, "Radius", &mut p.smooth.radius, 0.1..=3.0, 0.05, "mm");
            slider_f64(ui, "Edge angle", &mut p.smooth.angle, 2.0..=60.0, 1.0, "°");
            slider_usize(ui, "Passes", &mut p.smooth.normal_iters, 0.0..=12.0);
            slider_usize(ui, "Vertex", &mut p.smooth.vertex_iters, 0.0..=40.0);
        }
        Step::Segment => {
            title(ui, "3 · Regions", "Planes, cylinders and spheres grown from seed faces");
            slider_f64(ui, "Tolerance", &mut p.seg.tol, 0.01..=1.0, 0.01, "mm");
            slider_f64(ui, "Angle", &mut p.seg.angle, 2.0..=45.0, 1.0, "°");
            slider_f64(ui, "Min area", &mut p.seg.min_area, 0.1..=50.0, 0.5, "mm²");
            if toggle_row(ui, "Cylinders", p.seg.cylinders) {
                p.seg.cylinders = !p.seg.cylinders;
            }
            if toggle_row(ui, "Spheres", p.seg.spheres) {
                p.seg.spheres = !p.seg.spheres;
            }
            if let Some(sg) = pipe.and_then(|p| p.seg_raw.as_ref()) {
                line(ui, format!("{} planes · {} cylinders · {} spheres", sg.count(Kind::Plane), sg.count(Kind::Cylinder), sg.count(Kind::Sphere)));
            }
        }
        Step::Regularize => {
            title(ui, "4 · Regularize", "Parallel, square, on the axes, same radii, coaxial, mirrored");
            if toggle_row(ui, "Regularize", p.reg_on) {
                p.reg_on = !p.reg_on;
            }
            slider_f64(ui, "Angle", &mut p.reg.angle, 0.5..=15.0, 0.5, "°");
            slider_f64(ui, "Offset", &mut p.reg.offset, 0.01..=1.0, 0.01, "mm");
            slider_f64(ui, "Radius", &mut p.reg.radius_abs, 0.01..=1.0, 0.01, "mm");
            if toggle_row(ui, "Object axes", p.reg.axes) {
                p.reg.axes = !p.reg.axes;
            }
            if toggle_row(ui, "Mirror pairs", p.reg.mirror) {
                p.reg.mirror = !p.reg.mirror;
            }
            if let Some(r) = pipe.and_then(|p| p.reg.as_ref()) {
                line(ui, format!("{} directions, {} on the axes", r.directions, r.on_axes));
                line(ui, format!("{} coplanar · {} radius groups · {} coaxial", r.coplanar, r.radii, r.coaxial));
                line(ui, format!("{} mirror pairs", r.mirrored));
            }
        }
        Step::Snap => {
            title(ui, "5 · Snap", "Edges rebuilt where two shapes meet (fillet of fitted radius, or sharp); the faces move only near them");
            use rectify_core::snap::Edges;
            let items = [(Edges::Fillet, "Fillet", accent()), (Edges::Sharp, "Sharp", accent()), (Edges::Soft, "Keep", accent())];
            if let Some(e) = segmented(ui, &items, p.snap.edges, 28.0) {
                p.snap.edges = e;
            }
            slider_f64(ui, "Edge gap", &mut p.snap.gap, 0.2..=6.0, 0.1, "mm");
            slider_f64(ui, "Edge angle", &mut p.snap.edge_angle, 5.0..=80.0, 1.0, "°");
            slider_f64(ui, "Edge fit", &mut p.snap.edge_tol, 0.02..=0.6, 0.01, "mm");
            slider_f64(ui, "Min radius", &mut p.snap.min_radius, 0.0..=2.0, 0.05, "mm");
            slider_f64(ui, "Radius step", &mut p.snap.round, 0.0..=0.5, 0.05, "mm");
            slider_f64(ui, "Fade", &mut p.snap.fade, 0.0..=3.0, 0.05, "mm");
            slider_f64(ui, "Flatten", &mut p.snap.flatten, 0.0..=1.0, 0.05, "");
            slider_f64(ui, "Max move", &mut p.snap.max_move, 0.05..=5.0, 0.05, "mm");
            if let Some(r) = pipe.and_then(|p| p.snap.as_ref()) {
                line(ui, format!("{} edges rebuilt{}", r.edges, if r.radii.is_empty() { String::new() } else { format!(" · {} radii, median {} mm", r.radius_groups, fmt(r.radii[r.radii.len() / 2], 2)) }));
                line(ui, format!("{} continue each other · {} left alone", r.tangent, r.rejected));
                line(ui, format!("{} k on an edge · {} k faded · {} k untouched", r.on_edge / 1000, r.faded / 1000, r.free / 1000));
                line(ui, format!("Moved {} mm on average, {} at most", fmt(r.mean_move, 3), fmt(r.max_move, 2)));
            }
        }
    }
    if st.params != before {
        // a changed setting: this step and the ones after it are out of date
        if let Some(p) = &mut st.pipe {
            if p.done(s) {
                p.clear_from(s);
                st.gen += 1;
            }
        }
    }
    ui.add_space(8.0);
    let w = ui.available_width();
    if primary_btn(ui, None, &format!("Run from {}", s.label()), Some(w)).clicked() {
        st.run_from(s);
    }
}

fn region_card(ui: &mut Ui, st: &mut AppState) {
    let Some(r) = st.selected else { return };
    let Some(seg) = st.shown_seg() else { return };
    let Some(reg) = seg.regions.get(r as usize) else { return };
    let kind = reg.prim.kind();
    title(ui, &format!("Region {} · {}", r + 1, kind.label()), &format!("{} mm² · {} faces · distance {} mm", fmt(reg.area, 1), reg.faces.len(), fmt(reg.rms, 3)));
    match reg.prim {
        Prim::Plane { n, d } => line(ui, format!("Normal {} {} {} · offset {} mm", fmt(n.x, 3), fmt(n.y, 3), fmt(n.z, 3), fmt(d, 2))),
        Prim::Cylinder { a, r, .. } => line(ui, format!("Radius {} mm · axis {} {} {}", fmt(r, 3), fmt(a.x, 3), fmt(a.y, 3), fmt(a.z, 3))),
        Prim::Sphere { r, .. } => line(ui, format!("Radius {} mm", fmt(r, 3))),
    }
    ui.add_space(4.0);
    let items = [(Kind::Plane, "Plane", accent()), (Kind::Cylinder, "Cylinder", accent()), (Kind::Sphere, "Sphere", accent())];
    if let Some(k) = segmented(ui, &items, kind, 28.0) {
        if k != kind {
            st.retype(Some(k));
        }
    }
    ui.add_space(4.0);
    let w = ui.available_width();
    if secondary_btn(ui, "Free (no shape)", w).clicked() {
        st.retype(None);
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
    let (k1, k2, k3, f, r, esc, space, z) = ctx.input(|i| (i.key_pressed(Key::Num1), i.key_pressed(Key::Num2), i.key_pressed(Key::Num3), i.key_pressed(Key::F), i.key_pressed(Key::R), i.key_pressed(Key::Escape), i.key_down(Key::Space), i.key_pressed(Key::Z)));
    if z {
        st.settings.zebra = !st.settings.zebra;
    }
    if k1 {
        st.set_mode(ViewMode::Original);
    }
    if k2 {
        st.set_mode(ViewMode::Regions);
    }
    if k3 {
        st.set_mode(ViewMode::Result);
    }
    if f {
        st.frame_all();
    }
    if r {
        st.run_all();
    }
    if esc {
        st.selected = None;
        st.files_open = false;
    }
    st.compare = space;
}
