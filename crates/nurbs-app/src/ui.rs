//! The interface, the look of the first Forme NURBS (Établi): the view
//! first, floating islands on top (document, tool and Add / Carve, view),
//! the tool column on the right, the scene and the selected shape on the
//! left, the history strip at the bottom.

use crate::camera::ViewPreset;
use crate::files;
use crate::icons::{self, Icon};
use crate::state::*;
use crate::theme::*;
use crate::widgets::*;
use egui::{pos2, vec2, Align2, Color32, Id, Margin, Order, Rect, Response, Sense, Stroke, StrokeKind, Ui};
use glam::{DQuat, DVec3, EulerRot};
use nurbs_kernel::doc::PrimKind;
use nurbs_kernel::{Feature, FeatureId, Op, Prim};

/// Margin between the islands and the screen edges.
const EDGE: f32 = 14.0;
/// Height of the top islands.
const ISLAND_H: f32 = 40.0;
/// Width of the left inspector.
const LEFT_W: f32 = 296.0;
/// Width of the tool cards.
const CARDS_W: f32 = 150.0;
/// The history strip's height from the bottom of the screen (with its margin).
const HISTORY_H: f32 = 100.0;

pub fn op_color(op: Op) -> Color32 {
    match op {
        Op::Add => accent(),
        Op::Carve => carve(),
    }
}

/// + and − drawn as vectors.
pub fn paint_op_glyph(p: &egui::Painter, c: egui::Pos2, op: Op, color: Color32, size: f32) {
    let s = size * 0.5;
    let st = Stroke::new((size * 0.13).max(1.4), color);
    p.line_segment([c - vec2(s, 0.0), c + vec2(s, 0.0)], st);
    if op == Op::Add {
        p.line_segment([c - vec2(0.0, s), c + vec2(0.0, s)], st);
    }
}

pub fn prim_icon(k: PrimKind) -> Icon {
    match k {
        PrimKind::Box => Icon::Box,
        PrimKind::Cylinder => Icon::Cylinder,
    }
}

pub fn prim_key(k: PrimKind) -> &'static str {
    match k {
        PrimKind::Box => "B",
        PrimKind::Cylinder => "C",
    }
}

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    let left = if st.panel_visible { 2.0 * EDGE + LEFT_W } else { EDGE };
    st.view_rect = [left, EDGE + ISLAND_H, screen.width() - 2.0 * EDGE - CARDS_W, screen.height() - HISTORY_H];
    top_bar(ctx, st, screen);
    rail(ctx, st, screen);
    history_strip(ctx, st, screen);
    hud(ctx, st);
    if st.panel_visible {
        inspector(ctx, st, screen);
    }
    context_bar(ctx, st);
    files_dialog(ctx, st, screen);
    toast(ctx, st, screen);
    shortcuts(ctx, st);
}

// ------------------------------------------------------------- top bar

fn top_bar(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    // left: the document
    egui::Area::new(Id::new("isl-doc")).order(Order::Middle).fixed_pos(screen.min + vec2(EDGE, EDGE)).show(ctx, |ui| {
        island().show(ui, |ui| {
            ui.set_height(ISLAND_H - 10.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (r, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                paint_logo(ui, Rect::from_center_size(r.center(), vec2(26.0, 26.0)));
                let n = st.doc.features.len();
                let g = ui.painter().layout_no_wrap(st.doc_name.clone(), semibold(13.5), TEXT());
                let sub = ui.painter().layout_no_wrap(format!("· {n} shape{}", if n == 1 { "" } else { "s" }), regular(12.0), TEXT2());
                let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + sub.size().x + 36.0, 28.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, 7.0, RAISED());
                }
                let gw = g.size().x;
                ui.painter().galley(pos2(r.min.x + 6.0, r.center().y - g.size().y * 0.5), g, TEXT());
                ui.painter().galley(pos2(r.min.x + 12.0 + gw, r.center().y - sub.size().y * 0.5), sub, TEXT2());
                icons::paint(ui.painter(), Rect::from_min_size(pos2(r.max.x - 18.0, r.center().y - 6.0), vec2(12.0, 12.0)), Icon::Chevron, TEXT2());
                if resp.on_hover_text("Open · New · Save").clicked() {
                    st.files_open = !st.files_open;
                }
                island_sep(ui);
                if icon_btn(ui, Icon::Undo, false, "Undo (Ctrl+Z)", 28.0).clicked() {
                    st.undo();
                }
                if icon_btn(ui, Icon::Redo, false, "Redo (Ctrl+Y)", 28.0).clicked() {
                    st.redo();
                }
            });
        });
    });

    // centre: the tool, ADD | CARVE, the helpers
    let cx = st.principal()[0];
    egui::Area::new(Id::new("isl-modes")).order(Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(pos2(cx, screen.min.y + EDGE)).show(ctx, |ui| {
        island().show(ui, |ui| {
            ui.set_height(ISLAND_H - 10.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let tool = match st.tool {
                    Tool::Create(k) => k.label().to_uppercase(),
                    Tool::Select => "SELECTION · OBJECT".to_string(),
                };
                let g = ui.painter().layout_no_wrap(tool, mono(11.5), TEXT());
                let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 20.0, 28.0), Sense::hover());
                ui.painter().galley(pos2(r.min.x + 10.0, r.center().y - g.size().y * 0.5), g, TEXT());
                let shown = shown_op(st);
                let (well, _) = ui.allocate_exact_size(vec2(2.0 * 62.0 + 6.0, 28.0), Sense::hover());
                ui.painter().rect(well, 14.0, WELL(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
                for (i, op) in [Op::Add, Op::Carve].into_iter().enumerate() {
                    let r = Rect::from_min_size(well.min + vec2(3.0 + i as f32 * 62.0, 3.0), vec2(62.0, 22.0));
                    let resp = ui.interact(r, Id::new(("isl-mode", i)), Sense::click());
                    let on = shown == op;
                    let c = op_color(op);
                    if on {
                        ui.painter().rect_filled(r, 11.0, c);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(r, 11.0, RAISED());
                    }
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, if op == Op::Add { "ADD" } else { "CARVE" }, mono(11.0), if on { on_accent() } else { TEXT2() });
                    if resp.on_hover_text("New shapes add matter or carve it (X)").clicked() {
                        set_op_from_ui(st, op);
                    }
                }
                island_sep(ui);
                if icon_btn(ui, Icon::Magnet, st.settings.snap, &format!("Snap to {} mm", fmt_mm(st.settings.step as f32)), 28.0).clicked() {
                    st.settings.snap = !st.settings.snap;
                    st.save_settings();
                }
                if icon_btn(ui, Icon::Grid, st.settings.grid, "Ground grid", 28.0).clicked() {
                    st.settings.grid = !st.settings.grid;
                    st.save_settings();
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
                let cur = st.camera.preset_of();
                let presets = [(ViewPreset::Perspective, "3D", ""), (ViewPreset::Front, "Front", ""), (ViewPreset::Right, "Right", ""), (ViewPreset::Top, "Top", "")];
                if let Some(p) = seg_inline(ui, "isl-preset", &presets, cur) {
                    st.camera.preset(p);
                }
                island_sep(ui);
                if icon_btn(ui, Icon::Frame, false, "Frame the whole part (F with nothing selected)", 28.0).clicked() {
                    st.frame_all();
                }
                if icon_btn(ui, Icon::Light, st.settings.light, "Light theme", 28.0).clicked() {
                    st.settings.light = !st.settings.light;
                    st.save_settings();
                }
            });
        });
    });
}

/// The mode shown: the selected shape's with the Select tool, else the one
/// for new shapes.
fn shown_op(st: &AppState) -> Op {
    if st.tool == Tool::Select {
        if let Some(f) = st.single() {
            return f.op;
        }
    }
    st.mode
}

fn set_op_from_ui(st: &mut AppState, op: Op) {
    st.mode = op;
    if st.tool == Tool::Select {
        let ids = st.selection.clone();
        if !ids.is_empty() {
            st.edit(op.label(), |d| {
                for id in &ids {
                    if let Some(f) = d.feature_mut(*id) {
                        f.op = op;
                    }
                }
            });
        }
    }
}

// ---------------------------------------------------------- tool cards

fn card(ui: &mut Ui, icon: Option<Icon>, name: &str, sub: &str, on: bool, sub_color: Color32, tip: &str) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(CARDS_W - 12.0, 38.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect(r, 7.0, accent_bg(), Stroke::new(1.0, accent()), StrokeKind::Inside);
    } else {
        p.rect(r, 7.0, if resp.hovered() { RAISED() } else { tint(RAISED(), 0.35) }, Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
    }
    let ir = Rect::from_min_size(pos2(r.min.x + 7.0, r.center().y - 12.0), vec2(24.0, 24.0));
    p.rect(ir, 6.0, if on { tint(accent(), 0.18) } else { WELL() }, Stroke::new(1.0, if on { accent_border() } else { DIVIDER() }), StrokeKind::Inside);
    if let Some(i) = icon {
        icons::paint(p, ir.shrink(5.0), i, if on { accent() } else { TEXT_ICON() });
    }
    p.text(pos2(ir.max.x + 8.0, r.center().y - 7.0), Align2::LEFT_CENTER, name, mono(11.0), if on { accent_text() } else { TEXT() });
    p.text(pos2(ir.max.x + 8.0, r.center().y + 7.5), Align2::LEFT_CENTER, sub, mono(9.0), sub_color);
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
            if card(ui, Some(Icon::Select), "SELECTION", "OBJECT · Q", st.tool == Tool::Select, TEXT2(), "Select shapes by their faces (Q)").clicked() {
                st.select_tool();
            }
            divider(ui);
            let mode = st.mode;
            let m = if mode == Op::Add { "ADD" } else { "CARVE" };
            for k in PRIMS {
                let key = prim_key(k);
                let on = st.tool == Tool::Create(k);
                if card(ui, Some(prim_icon(k)), &k.label().to_uppercase(), &format!("{m} · {key}"), on, op_color(mode), &format!("{} ({key})", k.label())).clicked() {
                    st.arm(k);
                }
            }
            divider(ui);
            let has = !st.selection.is_empty();
            if card(ui, Some(Icon::Duplicate), "DUPLICATE", "CTRL+D", false, TEXT2(), "Duplicate the selection").clicked() && has {
                st.duplicate_selected();
            }
        });
    });
}

// --------------------------------------------------------- history strip

fn history_strip(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let cx = st.principal()[0];
    let max_w = (st.view_rect[2] - st.view_rect[0] - 24.0).max(320.0);
    let off = panel_offset(ctx, "history");
    egui::Area::new(Id::new("history")).order(Order::Middle).pivot(Align2::CENTER_BOTTOM).fixed_pos(pos2(cx, screen.max.y - 14.0) + off).show(ctx, |ui| {
        panel().inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            grip(ui, "history");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(egui::RichText::new("History").font(semibold(13.0)).color(TEXT()));
                    let n = st.doc.features.len();
                    let sub = match n {
                        0 => "empty".to_string(),
                        1 => "1 operation".to_string(),
                        _ => format!("{n} operations"),
                    };
                    ui.label(egui::RichText::new(sub).font(regular(11.5)).color(TEXT2()));
                });
                let (sep, _) = ui.allocate_exact_size(vec2(1.0, 36.0), Sense::hover());
                ui.painter().rect_filled(sep, 0.0, DIVIDER());
                if st.doc.features.is_empty() {
                    ui.label(egui::RichText::new("Pick a shape on the right, then drag in the view").font(regular(13.0)).color(TEXT2()));
                    return;
                }
                let feats = st.doc.features.clone();
                egui::ScrollArea::horizontal().id_salt("history-scroll").max_width(max_w - 220.0).min_scrolled_width(max_w - 220.0).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for (i, f) in feats.iter().enumerate() {
                            if i > 0 {
                                let (lr, _) = ui.allocate_exact_size(vec2(12.0, 44.0), Sense::hover());
                                ui.painter().rect_filled(Rect::from_center_size(lr.center(), vec2(10.0, 2.0)), 1.0, DIVIDER());
                            }
                            if history_chip(ui, st, f).clicked() {
                                st.select_tool();
                                let add = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl);
                                st.select(Some(f.id), add);
                            }
                        }
                    });
                });
                let (er, _) = ui.allocate_exact_size(vec2(40.0, 44.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(Rect::from_center_size(pos2(er.min.x + 4.0, er.center().y), vec2(3.0, 30.0)), 2.0, accent());
                p.text(pos2(er.min.x + 12.0, er.center().y), Align2::LEFT_CENTER, "End", regular(12.0), TEXT2());
            });
        });
    });
}

fn history_chip(ui: &mut Ui, st: &AppState, f: &Feature) -> Response {
    let sub = format!("{} · {}", f.prim.kind().label(), f.op.label());
    let tg = ui.painter().layout_no_wrap(f.name.clone(), semibold(12.5), TEXT());
    let sg = ui.painter().layout_no_wrap(sub, regular(11.0), TEXT2());
    let w = 8.0 + 28.0 + 8.0 + tg.size().x.max(sg.size().x) + 12.0;
    let (r, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
    let selected = st.is_selected(f.id);
    let p = ui.painter();
    if selected {
        p.rect(r, 10.0, accent_bg(), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(r, 10.0, RAISED());
    }
    let dim = if f.visible { 1.0 } else { 0.45 };
    let ir = Rect::from_min_size(pos2(r.min.x + 8.0, r.center().y - 13.0), vec2(26.0, 26.0));
    p.rect_filled(ir, 7.0, RAISED());
    icons::paint(p, ir.shrink(5.0), prim_icon(f.prim.kind()), tint(TEXT_ICON(), dim));
    p.circle_filled(ir.right_bottom() - vec2(1.0, 1.0), 5.5, PANEL());
    p.circle_filled(ir.right_bottom() - vec2(1.0, 1.0), 4.0, op_color(f.op));
    let tx = ir.max.x + 8.0;
    p.galley(pos2(tx, r.center().y - tg.size().y - 0.5), tg, if selected { accent_text() } else { tint(TEXT(), dim) });
    p.galley(pos2(tx, r.center().y + 1.5), sg, TEXT2());
    resp.on_hover_text("Select this operation")
}

// ------------------------------------------------------------------ hud

fn hud(ctx: &egui::Context, st: &mut AppState) {
    let cx = st.principal()[0];
    let text = crate::viewport::hint(st);
    let color = match st.tool {
        Tool::Create(_) => op_color(st.mode),
        Tool::Select => st.single().map(|f| op_color(f.op)).unwrap_or(TEXT2()),
    };
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
        ui.painter().rect(r, 18.0, pick(Color32::from_rgb(0x2A, 0x2D, 0x33), Color32::WHITE), Stroke::new(1.0, if warn { carve() } else { accent_border() }), StrokeKind::Inside);
        ui.painter().galley(pos2(r.min.x + 16.0, r.center().y - g.size().y * 0.5), g, TEXT());
    });
}

// ----------------------------------------------------- context bar

/// The actions of the selection, floating just above it.
fn context_bar(ctx: &egui::Context, st: &mut AppState) {
    if st.tool != Tool::Select || st.creation.is_some() || st.drag.is_some() || st.selection.is_empty() {
        return;
    }
    let b = st.doc.features.iter().filter(|f| st.selection.contains(&f.id)).map(|f| f.bounds()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)));
    let Some((lo, hi)) = b else { return };
    let view = st.view();
    let mut top = f32::INFINITY;
    let (mut x0, mut x1) = (f32::INFINITY, f32::NEG_INFINITY);
    for i in 0..8 {
        let c = DVec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
        if let Some(p) = view.project(c.as_vec3()) {
            top = top.min(p[1]);
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
        }
    }
    if !top.is_finite() {
        return;
    }
    let vr = st.view_rect;
    let at = pos2(((x0 + x1) * 0.5).clamp(vr[0] + 120.0, vr[2] - 120.0), (top - 16.0).clamp(vr[1] + 56.0, vr[3] - 40.0));
    egui::Area::new(Id::new("ctx-bar")).order(Order::Middle).pivot(Align2::CENTER_BOTTOM).fixed_pos(at).show(ctx, |ui| {
        island().inner_margin(Margin::same(3)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let op = shown_op(st);
                let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
                let c = op_color(op);
                ui.painter().rect(r, 7.0, tint(c, if resp.hovered() { 0.24 } else { 0.14 }), Stroke::new(1.0, tint(c, 0.6)), StrokeKind::Inside);
                paint_op_glyph(ui.painter(), r.center(), op, c, 11.0);
                if resp.on_hover_text(format!("{}: switch to {}", op.label(), op.other().label())).clicked() {
                    set_op_from_ui(st, op.other());
                }
                let (r, _) = ui.allocate_exact_size(vec2(7.0, 28.0), Sense::hover());
                ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(1.0, 18.0)), 0.0, DIVIDER());
                if icon_btn(ui, Icon::Duplicate, false, "Duplicate (Ctrl+D)", 28.0).clicked() {
                    st.duplicate_selected();
                }
                if icon_btn(ui, Icon::Frame, false, "Frame (F)", 28.0).clicked() {
                    st.frame_selection();
                }
                if icon_btn(ui, Icon::Trash, false, "Delete (Del)", 28.0).clicked() {
                    st.delete_selected();
                }
            });
        });
    });
}

// ------------------------------------------------------------ inspector

fn inspector(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let top = EDGE + ISLAND_H + 10.0;
    let inner_w = LEFT_W - 24.0;
    let total = screen.max.y - EDGE - top;
    let rows = st.doc.features.len().max(1) as f32;
    let tree_h = if st.settings.show_tree { (rows * 28.0 + 34.0).clamp(96.0, (total * 0.36).max(120.0)) } else { 0.0 };
    let off = panel_offset(ctx, "props");
    egui::Area::new(Id::new("props")).order(Order::Middle).fixed_pos(pos2(screen.min.x + EDGE, top) + off).show(ctx, |ui| {
        island().inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(inner_w);
            grip(ui, "props");
            if st.settings.show_tree {
                tree(ui, st, tree_h);
                ui.add_space(4.0);
                divider(ui);
                ui.add_space(6.0);
            }
            let props_max = (total - tree_h - 40.0).max(160.0);
            // an Area's Ui starts at last frame's size: without a minimum the scroll area never grows
            egui::ScrollArea::vertical().id_salt("props-scroll").max_height(props_max).min_scrolled_height(props_max).auto_shrink([false, true]).show(ui, |ui| {
                ui.set_width(inner_w);
                if st.single().is_some() {
                    properties(ui, st);
                } else if !st.selection.is_empty() {
                    ui.label(egui::RichText::new(format!("{} shapes selected", st.selection.len())).font(semibold(14.0)).color(TEXT()));
                    ui.label(egui::RichText::new("Move them with the arrows, or delete them (Del)").font(regular(12.0)).color(TEXT2()));
                } else {
                    defaults(ui, st);
                }
            });
        });
    });
}

fn tree(ui: &mut Ui, st: &mut AppState, height: f32) {
    let n = st.doc.features.len();
    ui.horizontal(|ui| {
        ui.set_height(26.0);
        let (lr, _) = ui.allocate_exact_size(vec2(ui.available_width() - 2.0 * 28.0, 26.0), Sense::hover());
        ui.painter().text(lr.left_center() + vec2(2.0, 0.0), Align2::LEFT_CENTER, "SCENE", semibold(10.5), TEXT2());
        ui.painter().text(lr.left_center() + vec2(52.0, 0.0), Align2::LEFT_CENTER, format!("{n} shape{}", if n == 1 { "" } else { "s" }), regular(11.5), pick(Color32::from_rgb(0x64, 0x6B, 0x75), Color32::from_rgb(0x8A, 0x90, 0x99)));
        ui.spacing_mut().item_spacing.x = 2.0;
        let one = st.single().map(|f| f.id);
        if icon_btn(ui, Icon::Up, false, "Earlier in the history", 26.0).clicked() {
            if let Some(id) = one {
                st.reorder(id, -1);
            }
        }
        if icon_btn(ui, Icon::Down, false, "Later in the history", 26.0).clicked() {
            if let Some(id) = one {
                st.reorder(id, 1);
            }
        }
    });
    let feats = st.doc.features.clone();
    egui::ScrollArea::vertical().id_salt("tree-scroll").max_height(height - 32.0).min_scrolled_height(height - 32.0).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        if feats.is_empty() {
            ui.label(egui::RichText::new("Empty · pick a shape on the right (B, C)").font(regular(12.0)).color(TEXT2()));
        }
        for f in &feats {
            let w = ui.available_width();
            let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
            let selected = st.is_selected(f.id);
            let hovered_3d = st.hover_feature == Some(f.id);
            let p = ui.painter();
            if selected {
                p.rect(r, 6.0, accent_bg(), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
            } else if resp.hovered() || hovered_3d {
                p.rect_filled(r, 6.0, RAISED());
            }
            let x0 = r.min.x + 6.0;
            let dim = if f.visible { 1.0 } else { 0.4 };
            let c = op_color(f.op);
            let br = Rect::from_min_size(pos2(x0, r.center().y - 8.0), vec2(16.0, 16.0));
            p.rect_filled(br, 4.0, tint(c, 0.16 * dim));
            paint_op_glyph(p, br.center(), f.op, tint(c, dim), 9.0);
            let name_color = if selected { TEXT_STRONG() } else { tint(TEXT(), dim) };
            let info_g = p.layout_no_wrap(f.prim.kind().label().to_string(), regular(12.0), tint(TEXT2(), dim));
            let info_x = r.max.x - 40.0 - info_g.size().x;
            p.galley(pos2(info_x, r.center().y - info_g.size().y * 0.5), info_g, Color32::WHITE);
            let max_w = (info_x - 8.0 - (x0 + 24.0)).max(20.0);
            let mut job = egui::text::LayoutJob::single_section(f.name.clone(), egui::TextFormat::simple(if selected { semibold(13.0) } else { regular(13.0) }, name_color));
            job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            let name_g = ui.painter().layout_job(job);
            ui.painter().galley(pos2(x0 + 24.0, r.center().y - name_g.size().y * 0.5), name_g, name_color);
            let er = Rect::from_center_size(pos2(r.max.x - 18.0, r.center().y), vec2(16.0, 16.0));
            icons::paint(ui.painter(), er, if f.visible { Icon::Eye } else { Icon::EyeOff }, tint(TEXT2(), 0.8));
            let eresp = ui.interact(er.expand(6.0), Id::new(("eye", f.id)), Sense::click());
            if eresp.clicked() {
                st.toggle_visible(f.id);
            } else if resp.clicked() {
                st.select_tool();
                let add = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl);
                st.select(Some(f.id), add);
            }
        }
    });
}

/// Nothing selected: what new shapes will be.
fn defaults(ui: &mut Ui, st: &mut AppState) {
    section(ui, "New shapes");
    let items = [(Op::Add, "Add", accent()), (Op::Carve, "Carve", carve())];
    if let Some(m) = segmented(ui, &items, st.mode, 40.0) {
        st.mode = m;
    }
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Press on the ground or on a flat face and drag the base, release, then move for the height. A click alone places a 20 mm shape.").font(regular(12.0)).color(TEXT2()));
    section(ui, "Display");
    if toggle_row(ui, "Shape list", st.settings.show_tree) {
        st.settings.show_tree = !st.settings.show_tree;
        st.save_settings();
    }
    if toggle_row(ui, "World origin", st.settings.origin) {
        st.settings.origin = !st.settings.origin;
        st.save_settings();
    }
    if toggle_row(ui, "Snap", st.settings.snap) {
        st.settings.snap = !st.settings.snap;
        st.save_settings();
    }
    let mut step = st.settings.step as f32;
    let (ch, _) = slider_row(ui, "Snap step", &mut step, 0.1..=10.0, 0.1, "mm");
    if ch {
        st.settings.step = step.max(0.1) as f64;
        st.save_settings();
    }
}

/// One edit of a feature from the panel (sliders: one undo step per drag).
fn feature_edit(st: &mut AppState, id: FeatureId, label: &str, started: bool, f: impl FnOnce(&mut Feature)) {
    if started {
        st.end_live();
        st.begin_live(label);
    } else if !st.is_live() {
        st.begin_live(label);
    }
    st.live(|d| {
        if let Some(x) = d.feature_mut(id) {
            f(x);
        }
    });
}

fn properties(ui: &mut Ui, st: &mut AppState) {
    let Some(f) = st.single().cloned() else { return };
    let id = f.id;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.label(egui::RichText::new("Selection").font(regular(12.0)).color(TEXT2()));
        ui.horizontal(|ui| {
            if st.renaming {
                let mut name = f.name.clone();
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(120.0).font(semibold(15.0)));
                if r.changed() {
                    st.edit("Rename", |d| {
                        if let Some(x) = d.feature_mut(id) {
                            x.name = name.clone();
                        }
                    });
                }
                if r.lost_focus() {
                    st.renaming = false;
                }
            } else {
                let r = ui.add(egui::Label::new(egui::RichText::new(&f.name).font(semibold(15.0)).color(TEXT())).sense(Sense::click()).truncate());
                if r.on_hover_text("Double-click to rename").double_clicked() {
                    st.renaming = true;
                }
            }
            chip_label(ui, f.prim.kind().label(), RAISED(), LABEL());
            let c = op_color(f.op);
            chip_label(ui, f.op.label(), tint(c, 0.16), c);
        });
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if icon_btn(ui, Icon::Duplicate, false, "Duplicate", 30.0).clicked() {
            st.duplicate_selected();
        }
        if icon_btn(ui, if f.visible { Icon::Eye } else { Icon::EyeOff }, false, "Hide", 30.0).clicked() {
            st.toggle_visible(id);
        }
        if icon_btn(ui, Icon::Frame, false, "Frame", 30.0).clicked() {
            st.frame_selection();
        }
        if icon_btn(ui, Icon::Trash, false, "Delete", 30.0).clicked() {
            st.delete_selected();
        }
    });
    if st.single().is_none() {
        return;
    }
    divider(ui);

    // a slider drag is one undo step: it ends when no pointer button is down
    let any_down = ui.input(|i| i.pointer.any_down());

    section(ui, "Shape");
    match f.prim {
        Prim::Box { x, y, z } => {
            for (k, label, v) in [(0, "Width", x), (1, "Height", y), (2, "Depth", z)] {
                let mut val = v as f32;
                let (ch, start) = slider_row(ui, label, &mut val, 0.1..=2000.0, 0.1, "mm");
                if ch {
                    feature_edit(st, id, "Size", start, |g| {
                        if let Prim::Box { x, y, z } = &mut g.prim {
                            *[x, y, z][k] = val.max(0.1) as f64;
                        }
                    });
                }
            }
        }
        Prim::Cylinder { radius, height } => {
            let mut r = radius as f32;
            let (ch, start) = slider_row(ui, "Radius", &mut r, 0.05..=1000.0, 0.1, "mm");
            if ch {
                feature_edit(st, id, "Radius", start, |g| {
                    if let Prim::Cylinder { radius, .. } = &mut g.prim {
                        *radius = r.max(0.05) as f64;
                    }
                });
            }
            let mut h = height as f32;
            let (ch, start) = slider_row(ui, "Height", &mut h, 0.1..=2000.0, 0.1, "mm");
            if ch {
                feature_edit(st, id, "Height", start, |g| g.prim.set_height(h.max(0.1) as f64));
            }
        }
    }

    section(ui, "Combine");
    let items = [(Op::Add, "Add", accent()), (Op::Carve, "Carve", carve())];
    if let Some(m) = segmented(ui, &items, f.op, 40.0) {
        st.set_op(id, m);
    }

    section(ui, "Position and rotation");
    position_rows(ui, st, &f);
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() {
        st.end_live();
    }
}

fn axis_field(ui: &mut Ui, label: &str, color: Color32, v: &mut f32, suffix: &str) -> egui::Response {
    let (lr, _) = ui.allocate_exact_size(vec2(18.0, 22.0), Sense::hover());
    ui.painter().text(lr.center(), Align2::CENTER_CENTER, label, semibold(12.0), color);
    value_field(ui, v, -100000.0..=100000.0, if suffix == "°" { 1.0 } else { 0.1 }, suffix, 66.0)
}

fn position_rows(ui: &mut Ui, st: &mut AppState, f: &Feature) {
    let id = f.id;
    let colors = [AXIS_X, AXIS_Y, AXIS_Z];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for k in 0..3 {
            let mut v = f.origin[k] as f32;
            let r = axis_field(ui, ["X", "Y", "Z"][k], colors[k], &mut v, "");
            if r.changed() {
                feature_edit(st, id, "Move", r.drag_started(), |g| g.origin[k] = v as f64);
            }
        }
    });
    let (ex, ey, ez) = f.rot.to_euler(EulerRot::XYZ);
    let mut e = [ex.to_degrees() as f32, ey.to_degrees() as f32, ez.to_degrees() as f32];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for k in 0..3 {
            let r = axis_field(ui, ["rX", "rY", "rZ"][k], colors[k], &mut e[k], "°");
            if r.changed() {
                let e2 = e;
                feature_edit(st, id, "Rotate", r.drag_started(), |g| {
                    g.rot = DQuat::from_euler(EulerRot::XYZ, (e2[0] as f64).to_radians(), (e2[1] as f64).to_radians(), (e2[2] as f64).to_radians());
                });
            }
        }
    });
    ui.label(egui::RichText::new("Position: the centre of the base").font(regular(11.0)).color(TEXT2()));
}

// -------------------------------------------------------------- files

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
            ui.set_width(380.0);
            ui.label(egui::RichText::new("Documents").font(semibold(16.0)).color(TEXT()));
            ui.label(egui::RichText::new(files::documents_dir().display().to_string()).font(regular(11.0)).color(TEXT2()));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut st.save_name).desired_width(240.0).hint_text("Name"));
                if primary_btn(ui, None, "Save", Some(100.0)).clicked() {
                    let name = files::sanitize(&st.save_name);
                    match files::save(&st.doc, &name) {
                        Ok(_) => {
                            st.doc_name = name.clone();
                            st.toast(&format!("Saved: {name}"), false);
                            st.files_open = false;
                        }
                        Err(e) => st.toast(&format!("Not saved: {e}"), true),
                    }
                }
            });
            ui.add_space(6.0);
            if secondary_btn(ui, "New document", 348.0).clicked() {
                st.new_doc();
                st.files_open = false;
            }
            ui.add_space(6.0);
            section(ui, "Open");
            let list = files::list();
            if list.is_empty() {
                ui.label(egui::RichText::new("No saved document yet").font(regular(12.0)).color(TEXT2()));
            }
            egui::ScrollArea::vertical().max_height(300.0).min_scrolled_height(300.0).show(ui, |ui| {
                for (name, path) in list {
                    let (r, resp) = ui.allocate_exact_size(vec2(348.0, 30.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 6.0, RAISED());
                    }
                    ui.painter().text(r.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, &name, regular(13.0), TEXT());
                    if resp.clicked() {
                        match files::load(&path) {
                            Ok(d) => {
                                st.open(&name, d);
                                st.files_open = false;
                            }
                            Err(e) => st.toast(&format!("Cannot open {name}: {e}"), true),
                        }
                    }
                }
            });
        });
    });
}

// ----------------------------------------------------------- shortcuts

fn shortcuts(ctx: &egui::Context, st: &mut AppState) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    use egui::Key;
    let (ctrl, shift) = ctx.input(|i| (i.modifiers.ctrl || i.modifiers.mac_cmd, i.modifiers.shift));
    let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
    if ctrl {
        if pressed(Key::Z) {
            if shift {
                st.redo();
            } else {
                st.undo();
            }
        }
        if pressed(Key::Y) {
            st.redo();
        }
        if pressed(Key::D) {
            st.duplicate_selected();
        }
        if pressed(Key::S) {
            st.files_open = true;
        }
        return;
    }
    if pressed(Key::Escape) {
        if st.creation.is_some() || st.drag.is_some() {
            st.abort_gestures();
        } else if st.tool != Tool::Select {
            st.select_tool();
        } else {
            st.selection.clear();
        }
    }
    if pressed(Key::Q) {
        st.select_tool();
    }
    if pressed(Key::B) {
        st.arm(PrimKind::Box);
    }
    if pressed(Key::C) {
        st.arm(PrimKind::Cylinder);
    }
    if pressed(Key::X) {
        let op = shown_op(st).other();
        set_op_from_ui(st, op);
    }
    if pressed(Key::F) {
        if st.selection.is_empty() {
            st.frame_all();
        } else {
            st.frame_selection();
        }
    }
    if pressed(Key::Delete) || pressed(Key::Backspace) {
        st.delete_selected();
    }
    let _ = DVec3::ZERO;
}
