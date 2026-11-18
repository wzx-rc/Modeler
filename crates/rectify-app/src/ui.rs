//! The interface (Forme NURBS's islands, in orange): the file, the tools and
//! the view at the top, the tool's settings and the history on the left, a
//! summary at the bottom.

use crate::icons::{self, Icon};
use crate::state::*;
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
                    Some(d) => format!("· {} k triangles", d.original.tris.len() / 1000),
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
                let tools = [(Tool::Navigate, Icon::Select, "Navigate: the left button turns around the mesh (Esc)", "Esc"), (Tool::Line, Icon::Sketch, "Line: drag along an edge, it becomes straight and sharp (L)", "L")];
                for (t, icon, tip, key) in tools {
                    let r = icon_btn(ui, icon, st.tool == t, tip, 28.0);
                    key_hint(ui, &r, key);
                    if r.clicked() {
                        st.set_tool(t);
                    }
                }
                island_sep(ui);
                let can_undo = !st.history.is_empty();
                if icon_btn(ui, Icon::Undo, false, "Undo (Ctrl+Z)", 28.0).clicked() && can_undo {
                    st.undo();
                }
                if icon_btn(ui, Icon::Redo, false, "Redo (Ctrl+Shift+Z)", 28.0).clicked() && !st.undone.is_empty() {
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
                if icon_btn(ui, Icon::Export, false, "Export the mesh as .obj, with the original UVs (Documents/Forme Rectify)", 28.0).clicked() && st.doc.is_some() {
                    st.export();
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
                let (lo, hi) = d.original.bounds();
                let s = hi - lo;
                let parts = [
                    format!("{} × {} × {} mm", fmt(s.x, 1), fmt(s.y, 1), fmt(s.z, 1)),
                    format!("{} k vertices · edges {} mm", d.original.pos.len() / 1000, fmt(d.edge, 2)),
                    match st.history.len() {
                        0 => "No edit yet".to_string(),
                        1 => "1 edit".to_string(),
                        n => format!("{n} edits"),
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
            history_card(ui, st);
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
            title(ui, "Tools", "Each tool puts right what you point at");
            let w = ui.available_width();
            if secondary_btn(ui, "Line (L): straight, sharp edges", w).clicked() {
                st.set_tool(Tool::Line);
            }
        }
        Tool::Line => {
            title(ui, "Line", "Press on an edge, drag along it, release: there the edge becomes straight and sharp");
            slider_f64(ui, "Width", &mut st.line.width, 0.2..=8.0, 0.1, "mm");
            slider_f64(ui, "Hold", &mut st.line.hold, 0.0..=0.95, 0.05, "");
            line(ui, "Width: how far from the edge the sides are put on their planes. Hold: the part of it moved fully, the rest fades.");
        }
    }
}

fn history_card(ui: &mut Ui, st: &mut AppState) {
    title(ui, "History", "");
    if st.history.is_empty() && st.undone.is_empty() {
        line(ui, "No edit yet.");
        return;
    }
    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
        for (i, s) in st.history.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(egui::RichText::new(format!("{}", i + 1)).font(mono(10.5)).color(TEXT2()));
                ui.label(egui::RichText::new(s.tool.label()).font(medium(12.0)).color(accent_text()));
                ui.label(egui::RichText::new(&s.what).font(regular(12.0)).color(TEXT()));
            });
        }
        for s in st.undone.iter().rev() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(egui::RichText::new("·").font(mono(10.5)).color(TEXT2()));
                ui.label(egui::RichText::new(s.tool.label()).font(medium(12.0)).color(TEXT2()));
                ui.label(egui::RichText::new(format!("{} (undone)", s.what)).font(regular(12.0)).color(TEXT2()));
            });
        }
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 6.0) * 0.5;
        if secondary_btn(ui, "Undo", w).clicked() {
            st.undo();
        }
        if secondary_btn(ui, "Redo", w).clicked() {
            st.redo();
        }
    });
    if !st.history.is_empty() {
        let w = ui.available_width();
        if secondary_btn(ui, "Back to the mesh as it came", w).clicked() {
            st.revert_all();
        }
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
    let (f, esc, space, z, y, l) = ctx.input(|i| (i.key_pressed(Key::F), i.key_pressed(Key::Escape), i.key_down(Key::Space), i.key_pressed(Key::Z), i.key_pressed(Key::Y), i.key_pressed(Key::L)));
    let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
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
    if l && !cmd {
        st.set_tool(Tool::Line);
    }
    if f {
        st.frame_all();
    }
    if esc {
        if st.line_drag.is_some() {
            st.line_drag = None;
            st.overlay_gen += 1;
        } else {
            st.set_tool(Tool::Navigate);
        }
        st.files_open = false;
    }
    st.compare = space;
}
