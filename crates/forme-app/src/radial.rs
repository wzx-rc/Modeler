//! Radial interface (look B): a near-black view with one acid green accent.
//! A pill at the top centre names the active tool and switches Add / Carve;
//! a column of tool cards runs down the right edge; the marking menu (right
//! click) is drawn as a ring of wedges (marking.rs); the inspector appears
//! on the left only while something is selected. Labels in JetBrains Mono.

use crate::camera::ViewPreset;
use crate::icons::{self, Icon};
use crate::state::*;
use crate::theme::*;
use crate::ui;
use egui::{pos2, vec2, Align2, CornerRadius, Frame, Id, Margin, Order, Rect, Response, Sense, Stroke, StrokeKind, Ui};
use forme_core::*;

/// Right column of tool cards.
const COL_W: f32 = 176.0;
const CARD_H: f32 = 40.0;
/// Contextual inspector (inner width 248, as the shared rows expect).
const INSP_W: f32 = 272.0;
const PILL_H: f32 = 36.0;

fn glass() -> Frame {
    Frame::new()
        .fill(tint(PANEL, 0.94))
        .stroke(Stroke::new(1.0, PANEL_BORDER))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(8))
}

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    st.screen = [screen.width(), screen.height()];
    let inspector = st.selection.and_then(|id| st.scene.node(id)).is_some();
    let left = if inspector { 10.0 + INSP_W } else { 0.0 };
    st.set_view_rect([screen.min.x + left, screen.min.y + 56.0, screen.max.x - COL_W - 20.0, screen.max.y - 48.0]);

    app_pill(ctx, st, screen);
    tool_pill(ctx, st, screen);
    tool_column(ctx, st, screen);
    if inspector {
        contextual_inspector(ctx, st, screen);
    }
    bottom(ctx, st, screen);
    ui::dialogs(ctx, st, screen);
    ui::shortcuts(ctx, st);
}

/// Name of the tool in the top pill.
fn tool_name(st: &AppState) -> String {
    match (&st.placement, st.tool) {
        (Some(p), _) => p.kind.label().to_uppercase(),
        (None, Tool::Place(k)) => k.label().to_uppercase(),
        (None, Tool::Select) if st.pick_mode == PickMode::Edge => "SÉLECTION D'ARÊTES".to_string(),
        _ => "SÉLECTION".to_string(),
    }
}

fn round_btn(ui: &mut Ui, icon: Icon, on: bool, tip: &str) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(PILL_H - 6.0, PILL_H - 6.0), Sense::click());
    let p = ui.painter();
    if on {
        p.circle(r.center(), r.width() * 0.5, accent_bg(), Stroke::new(1.0, accent()));
    } else if resp.hovered() {
        p.circle_filled(r.center(), r.width() * 0.5, RAISED);
    }
    icons::paint(p, Rect::from_center_size(r.center(), vec2(15.0, 15.0)), icon, if on { accent() } else { TEXT_ICON });
    resp.on_hover_text(tip)
}

/// Top left: the document (files), undo / redo, light, settings.
fn app_pill(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    egui::Area::new(Id::new("radial-app")).order(Order::Middle).fixed_pos(screen.min + vec2(12.0, 12.0)).show(ctx, |ui| {
        glass().corner_radius(CornerRadius::same(18)).inner_margin(Margin::symmetric(6, 3)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let (r, _) = ui.allocate_exact_size(vec2(PILL_H - 6.0, PILL_H - 6.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 11.0, accent());
                icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(13.0, 13.0)), Icon::Cube, on_accent());
                let name = st.doc_name.to_uppercase();
                let g = ui.painter().layout_no_wrap(name, mono(11.5), TEXT);
                let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 16.0, PILL_H - 6.0), Sense::click());
                if resp.hovered() || st.files_open {
                    ui.painter().rect_filled(r, 12.0, RAISED);
                }
                ui.painter().galley(pos2(r.min.x + 8.0, r.center().y - g.size().y * 0.5), g, TEXT);
                if resp.on_hover_text("Ouvrir, nouveau, enregistrer").clicked() {
                    st.files_open = !st.files_open;
                }
                if round_btn(ui, Icon::Undo, false, "Annuler (Ctrl+Z)").clicked() {
                    st.undo();
                }
                if round_btn(ui, Icon::Redo, false, "Rétablir (Ctrl+Y)").clicked() {
                    st.redo();
                }
                let r = round_btn(ui, Icon::Light, st.display_open, "Affichage : lumière, ombres, anticrénelage");
                ui::light_toggle(st, &r);
                if round_btn(ui, Icon::Settings, st.settings_open, "Réglages").clicked() {
                    st.settings_open = !st.settings_open;
                }
            });
        });
    });
}

/// Top centre: the active tool, then ADD | CARVE.
fn tool_pill(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let cx = (screen.min.x + screen.max.x - COL_W) * 0.5;
    egui::Area::new(Id::new("radial-tool")).order(Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(pos2(cx, screen.min.y + 12.0)).show(ctx, |ui| {
        glass().corner_radius(CornerRadius::same(18)).inner_margin(Margin::symmetric(6, 3)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let g = ui.painter().layout_no_wrap(tool_name(st), mono(12.5), accent_text());
                let (r, _) = ui.allocate_exact_size(vec2(g.size().x.max(120.0) + 28.0, PILL_H - 6.0), Sense::hover());
                ui.painter().galley(pos2(r.min.x + 14.0, r.center().y - g.size().y * 0.5), g, accent_text());
                let shown = ui::shown_mode(st);
                for m in MODES {
                    let label = if m == CombineMode::Add { "ADD" } else { "CARVE" };
                    let c = ui::mode_color(m);
                    let (r, resp) = ui.allocate_exact_size(vec2(74.0, PILL_H - 6.0), Sense::click());
                    let on = shown == m;
                    let p = ui.painter();
                    if on {
                        p.rect_filled(r, 15.0, c);
                    } else if resp.hovered() {
                        p.rect_filled(r, 15.0, RAISED);
                    }
                    p.text(r.center(), Align2::CENTER_CENTER, label, mono(12.0), if on { on_accent() } else { TEXT2 });
                    let tip = if m == CombineMode::Add { "Add : ajoute la forme" } else { "Carve : creuse" };
                    if resp.on_hover_text(tip).clicked() {
                        st.set_mode(m);
                    }
                }
            });
        });
    });
}

/// One tool card: icon tile, title, small line under it.
fn tool_card(ui: &mut Ui, icon: Icon, title: &str, sub: &str, on: bool, key: Option<&str>) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(COL_W - 16.0, CARD_H), Sense::click());
    let p = ui.painter();
    if on {
        p.rect(r, 9.0, accent_bg(), Stroke::new(1.5, accent()), StrokeKind::Inside);
    } else {
        p.rect(r, 9.0, if resp.hovered() { RAISED } else { tint(RAISED, 0.45) }, Stroke::new(1.0, PANEL_BORDER), StrokeKind::Inside);
    }
    let tile = Rect::from_min_size(r.min + vec2(6.0, 6.0), vec2(28.0, 28.0));
    p.rect(tile, 7.0, if on { tint(accent(), 0.18) } else { WELL }, Stroke::new(1.0, if on { accent_border() } else { DIVIDER }), StrokeKind::Inside);
    icons::paint(p, tile.shrink(6.0), icon, if on { accent() } else { TEXT_ICON });
    let tx = tile.max.x + 9.0;
    p.text(pos2(tx, r.min.y + 13.0), Align2::LEFT_CENTER, title, mono(11.5), if on { accent_text() } else { TEXT });
    p.text(pos2(tx, r.min.y + 28.0), Align2::LEFT_CENTER, sub, mono(9.5), if on { accent() } else { TEXT2 });
    if let Some(k) = key {
        let kr = Rect::from_min_size(pos2(r.max.x - 22.0, r.center().y - 8.0), vec2(16.0, 16.0));
        p.rect(kr, 4.0, WELL, Stroke::new(1.0, DIVIDER), StrokeKind::Inside);
        p.text(kr.center(), Align2::CENTER_CENTER, k, mono(9.5), TEXT2);
    }
    resp
}

fn column_title(ui: &mut Ui, t: &str) {
    ui.add_space(2.0);
    ui.label(egui::RichText::new(t).font(mono(9.5)).color(TEXT2));
}

/// Right edge: selection, shapes, aids, as cards in a tall glass column.
fn tool_column(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let rect = Rect::from_min_max(pos2(screen.max.x - COL_W - 10.0, screen.min.y + 10.0), pos2(screen.max.x - 10.0, screen.max.y - 10.0));
    egui::Area::new(Id::new("radial-column")).order(Order::Middle).fixed_pos(rect.min).show(ctx, |ui| {
        glass().show(ui, |ui| {
            ui.set_width(COL_W - 16.0);
            ui.set_min_height(rect.height() - 16.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            let sel = st.tool == Tool::Select;
            column_title(ui, "SÉLECTION");
            if tool_card(ui, Icon::Select, "OBJETS", "CLIC · RECTANGLE", sel && st.pick_mode == PickMode::Object, Some("Q")).on_hover_text("Sélection d'objets (Q ou 1)").clicked() {
                st.set_pick_mode(PickMode::Object);
            }
            if tool_card(ui, Icon::Fillet, "ARÊTES", "FILLET · CHANFREIN", sel && st.pick_mode == PickMode::Edge, Some("2")).on_hover_text("Sélection d'arêtes (2)").clicked() {
                st.set_pick_mode(PickMode::Edge);
            }
            column_title(ui, "FORMES");
            let mode = if st.effective_mode() == CombineMode::Carve { "CARVE" } else { "ADD" };
            for k in PRIMS {
                let on = st.tool == Tool::Place(k);
                if tool_card(ui, ui::prim_icon(k), &k.label().to_uppercase(), mode, on, ui::prim_key(k)).clicked() {
                    st.commit_placement();
                    st.tool = Tool::Place(k);
                }
            }
            ui::creation_btn(ui, st, COL_W - 16.0, false);
            column_title(ui, "AIDES");
            let onoff = |b: bool| if b { "ACTIVÉ" } else { "DÉSACTIVÉ" };
            if tool_card(ui, Icon::Mirror, "SYMÉTRIE X", onoff(st.sym_x), st.sym_x, None).clicked() {
                st.sym_x = !st.sym_x;
                st.toast(if st.sym_x { "Symétrie X : les nouvelles formes sont doublées" } else { "Symétrie X désactivée" });
            }
            if tool_card(ui, Icon::Magnet, "SNAP", onoff(st.snap), st.snap, None).clicked() {
                st.snap = !st.snap;
            }
            if tool_card(ui, Icon::Grid, "GRILLE", onoff(st.grid), st.grid, None).clicked() {
                st.grid = !st.grid;
                st.view_changed();
            }
        });
    });
}

/// Left: the inspector of the selection, only while there is one.
fn contextual_inspector(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let top = screen.min.y + 12.0 + PILL_H + 12.0;
    let max_h = screen.max.y - 48.0 - top - 8.0;
    egui::Area::new(Id::new("radial-inspector")).order(Order::Middle).fixed_pos(pos2(screen.min.x + 12.0, top)).show(ctx, |ui| {
        glass().inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(INSP_W - 24.0);
            ui.label(egui::RichText::new("INSPECTEUR").font(mono(9.5)).color(TEXT2));
            egui::ScrollArea::vertical().id_salt("radial-insp").max_height(max_h - 40.0).min_scrolled_height(max_h - 40.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.set_width(INSP_W - 24.0);
                ui::properties(ui, st);
            });
        });
    });
}

/// Bottom: view presets on the left, what the tool expects in the middle.
fn bottom(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    egui::Area::new(Id::new("radial-views")).order(Order::Middle).pivot(Align2::LEFT_BOTTOM).fixed_pos(pos2(screen.min.x + 12.0, screen.max.y - 12.0)).show(ctx, |ui| {
        glass().corner_radius(CornerRadius::same(14)).inner_margin(Margin::symmetric(4, 3)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let cur = st.camera.preset_of();
                for (p, l) in [(ViewPreset::Perspective, "3D"), (ViewPreset::Front, "FACE"), (ViewPreset::Right, "DROITE"), (ViewPreset::Top, "DESSUS")] {
                    let on = cur == p;
                    let g = ui.painter().layout_no_wrap(l.to_string(), mono(10.5), TEXT);
                    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 16.0, 22.0), Sense::click());
                    let pt = ui.painter();
                    if on {
                        pt.rect_filled(r, 11.0, accent_bg());
                    } else if resp.hovered() {
                        pt.rect_filled(r, 11.0, RAISED);
                    }
                    pt.text(r.center(), Align2::CENTER_CENTER, l, mono(10.5), if on { accent() } else { TEXT2 });
                    if resp.clicked() {
                        st.camera.preset(p);
                        st.view_changed();
                    }
                }
            });
        });
    });
    let hint = st.hint();
    let cx = st.principal()[0];
    egui::Area::new(Id::new("radial-hint")).order(Order::Middle).pivot(Align2::CENTER_BOTTOM).fixed_pos(pos2(cx, screen.max.y - 14.0)).interactable(false).show(ctx, |ui| {
        let max_w = (screen.width() - COL_W - 2.0 * 330.0).max(240.0);
        let mut job = egui::text::LayoutJob::single_section(hint, egui::TextFormat::simple(regular(12.0), LABEL));
        job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let g = ui.painter().layout_job(job);
        let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 36.0, 26.0), Sense::hover());
        let p = ui.painter();
        p.rect(r, 13.0, tint(PANEL, 0.9), Stroke::new(1.0, PANEL_BORDER), StrokeKind::Inside);
        p.circle_filled(pos2(r.min.x + 14.0, r.center().y), 3.5, ui::mode_color(st.effective_mode()));
        p.galley(pos2(r.min.x + 24.0, r.center().y - g.size().y * 0.5), g, LABEL);
    });
}
