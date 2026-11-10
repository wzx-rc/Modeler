//! Plaque interface (look A): sober, in the spirit of Plasticity. The 3D view
//! fills the screen; a thin tool bar of small icon groups on the left, a menu
//! pill top left, view presets top centre, a floating inspector in mm next to
//! the docked scene tree on the right, and a thin status bar at the bottom
//! that says what the tool expects. One warm orange accent (= Add).

use crate::camera::ViewPreset;
use crate::icons::{self, Icon};
use crate::settings::Navigation;
use crate::state::*;
use crate::theme::*;
use crate::ui;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Frame, Id, Margin, Order, Rect, Response, Sense, Stroke, StrokeKind, Ui};
use forme_core::*;

/// Docked scene tree on the right.
const TREE_W: f32 = 240.0;
/// Status bar at the bottom.
const STATUS_H: f32 = 26.0;
/// Floating inspector (inner width 248, as the shared property rows expect).
const INSP_W: f32 = 272.0;
/// Tool bar buttons.
const BTN: f32 = 32.0;
const GAP: f32 = 10.0;

/// Small card: tight corners, thin border (Plaque look).
fn card() -> Frame {
    Frame::new().fill(pal().panel).stroke(Stroke::new(1.0, pal().panel_border)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(4))
}

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    st.screen = [screen.width(), screen.height()];
    let view = Rect::from_min_max(pos2(screen.min.x + 56.0, screen.min.y), pos2(screen.max.x - TREE_W, screen.max.y - STATUS_H));
    st.set_view_rect([view.min.x, view.min.y, view.max.x, view.max.y]);

    menu_pill(ctx, st, screen);
    view_pill(ctx, st, view);
    tools(ctx, st, screen);
    scene_dock(ctx, st, screen);
    inspector(ctx, st, screen);
    status_bar(ctx, st, screen);
    ui::dialogs(ctx, st, screen);
    ui::shortcuts(ctx, st);
}

/// Icon button of the Plaque look: square, 6 pt corners, orange when on.
fn tool_btn(ui: &mut Ui, icon: Icon, on: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(BTN, BTN), Sense::click());
    let p = ui.painter();
    if on {
        p.rect(rect.shrink(0.5), 6.0, pal().accent_bg, Stroke::new(1.0, pal().accent), StrokeKind::Inside);
    } else if resp.hovered() || resp.is_pointer_button_down_on() {
        p.rect_filled(rect, 6.0, pal().raised);
    }
    let c = if on { pal().accent_light } else { pal().text_icon };
    icons::paint(p, Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), icon, c);
    resp.on_hover_text(tip)
}

/// Shortcut letter in the bottom-right corner of a tool button.
fn key(ui: &Ui, r: &Response, k: &str) {
    ui.painter().text(r.rect.right_bottom() - vec2(3.0, 2.0), Align2::RIGHT_BOTTOM, k, medium(8.5), pal().text2);
}

/// Top left: the app, the document (opens the files), undo / redo.
fn menu_pill(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    egui::Area::new(Id::new("plaque-menu")).order(Order::Middle).fixed_pos(screen.min + vec2(10.0, 10.0)).show(ctx, |ui| {
        card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let (r, _) = ui.allocate_exact_size(vec2(BTN, BTN), Sense::hover());
                let logo = Rect::from_center_size(r.center(), vec2(22.0, 22.0));
                ui.painter().rect_filled(logo, 5.0, pal().accent);
                icons::paint(ui.painter(), logo.shrink(4.0), Icon::Cube, pal().on_accent);
                let name = st.doc_name.clone();
                let g = ui.painter().layout_no_wrap(name, medium(12.5), pal().text);
                let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 30.0, BTN), Sense::click());
                if resp.hovered() || st.files_open {
                    ui.painter().rect_filled(r, 6.0, pal().raised);
                }
                ui.painter().galley(pos2(r.min.x + 6.0, r.center().y - g.size().y * 0.5), g, pal().text);
                icons::paint(ui.painter(), Rect::from_min_size(pos2(r.max.x - 18.0, r.center().y - 6.0), vec2(12.0, 12.0)), Icon::Chevron, pal().text2);
                if resp.on_hover_text("Ouvrir, nouveau, enregistrer").clicked() {
                    st.files_open = !st.files_open;
                }
                sep(ui);
                if tool_btn(ui, Icon::Undo, false, "Annuler (Ctrl+Z)").clicked() {
                    st.undo();
                }
                if tool_btn(ui, Icon::Redo, false, "Rétablir (Ctrl+Y)").clicked() {
                    st.redo();
                }
            });
        });
    });
}

fn sep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(9.0, BTN), Sense::hover());
    ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(1.0, 18.0)), 0.0, pal().divider);
}

/// Top centre of the view: view presets, grid, light, settings.
fn view_pill(ctx: &egui::Context, st: &mut AppState, view: Rect) {
    egui::Area::new(Id::new("plaque-view")).order(Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(pos2(view.center().x, view.min.y + 10.0)).show(ctx, |ui| {
        card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let cur = st.camera.preset_of();
                for (p, l) in [(ViewPreset::Perspective, "3D"), (ViewPreset::Front, "Face"), (ViewPreset::Right, "Droite"), (ViewPreset::Top, "Dessus")] {
                    let on = cur == p;
                    let g = ui.painter().layout_no_wrap(l.to_string(), medium(12.0), pal().text);
                    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 18.0, BTN), Sense::click());
                    let pt = ui.painter();
                    if on {
                        pt.rect_filled(r, 6.0, pal().raised);
                    } else if resp.hovered() {
                        pt.rect_filled(r, 6.0, pal().well);
                    }
                    pt.text(r.center(), Align2::CENTER_CENTER, l, medium(12.0), if on { pal().text_strong } else { pal().text2 });
                    if resp.clicked() {
                        st.camera.preset(p);
                        st.view_changed();
                    }
                }
                sep(ui);
                if tool_btn(ui, Icon::Grid, st.grid, "Grille au sol").clicked() {
                    st.grid = !st.grid;
                    st.view_changed();
                }
                let r = tool_btn(ui, Icon::Light, st.display_open, "Affichage : lumière, ombres, anticrénelage");
                ui::light_toggle(st, &r);
                if tool_btn(ui, Icon::Settings, st.settings_open, "Réglages").clicked() {
                    st.settings_open = !st.settings_open;
                }
            });
        });
    });
}

/// Left: the tool bar, in small groups (selection, shapes, mode, aids).
fn tools(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    egui::Area::new(Id::new("plaque-tools")).order(Order::Middle).fixed_pos(screen.min + vec2(10.0, 62.0)).show(ctx, |ui| {
        ui.spacing_mut().item_spacing.y = GAP;
        let sel = st.tool == Tool::Select;
        card().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let r = tool_btn(ui, Icon::Select, sel && st.pick_mode == PickMode::Object, "Sélection d'objets (Q ou 1)");
            key(ui, &r, "Q");
            if r.clicked() {
                st.set_pick_mode(PickMode::Object);
            }
            let r = tool_btn(ui, Icon::Fillet, sel && st.pick_mode == PickMode::Edge, "Sélection d'arêtes (2) : fillets, chanfreins");
            key(ui, &r, "2");
            if r.clicked() {
                st.set_pick_mode(PickMode::Edge);
            }
        });
        card().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for k in PRIMS {
                let letter = ui::prim_key(k);
                let tip = match letter {
                    Some(l) => format!("{} ({l})", k.label()),
                    None => k.label().to_string(),
                };
                let r = tool_btn(ui, ui::prim_icon(k), st.tool == Tool::Place(k), &tip);
                if let Some(l) = letter {
                    key(ui, &r, l);
                }
                if r.clicked() {
                    st.commit_placement();
                    st.tool = Tool::Place(k);
                }
            }
            ui::creation_btn(ui, st, BTN, false);
        });
        card().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let shown = ui::shown_mode(st);
            for m in MODES {
                let (r, resp) = ui.allocate_exact_size(vec2(BTN, BTN), Sense::click());
                let c = ui::mode_color(m);
                let on = shown == m;
                let p = ui.painter();
                if on {
                    p.rect(r.shrink(0.5), 6.0, tint(c, 0.16), Stroke::new(1.0, c), StrokeKind::Inside);
                } else if resp.hovered() {
                    p.rect_filled(r, 6.0, pal().raised);
                }
                ui::paint_mode_glyph(p, r.center(), m, if on { c } else { pal().text_icon }, 12.0);
                let tip = if m == CombineMode::Add { "Add : ajoute la forme" } else { "Carve : creuse" };
                if resp.on_hover_text(tip).clicked() {
                    st.set_mode(m);
                }
            }
        });
        card().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            if tool_btn(ui, Icon::Mirror, st.sym_x, "Symétrie X des nouvelles formes").clicked() {
                st.sym_x = !st.sym_x;
                st.toast(if st.sym_x { "Symétrie X : les nouvelles formes sont doublées" } else { "Symétrie X désactivée" });
            }
            if tool_btn(ui, Icon::Magnet, st.snap, "Snap aux arêtes et aux sommets").clicked() {
                st.snap = !st.snap;
            }
        });
    });
}

/// Right: the scene tree, docked from top to the status bar.
fn scene_dock(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let rect = Rect::from_min_max(pos2(screen.max.x - TREE_W, screen.min.y), pos2(screen.max.x, screen.max.y - STATUS_H));
    egui::Area::new(Id::new("plaque-scene")).order(Order::Middle).fixed_pos(rect.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, 0.0, pal().panel);
        p.rect_filled(Rect::from_min_size(r.min, vec2(1.0, r.height())), 0.0, pal().panel_border);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(vec2(8.0, 10.0))));
        let ui = &mut child;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Scène").font(semibold(13.0)).color(pal().text));
            ui.label(egui::RichText::new(format!("{} formes", st.scene.shape_count())).font(regular(11.5)).color(pal().text2));
        });
        ui.add_space(2.0);
        let h = ui.available_height();
        ui::tree(ui, st, h);
    });
}

/// Floating inspector next to the scene tree, only with a selection.
fn inspector(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    if st.selection.and_then(|id| st.scene.node(id)).is_none() {
        return;
    }
    let x = screen.max.x - TREE_W - 10.0 - INSP_W;
    let max_h = screen.height() - STATUS_H - 62.0 - 10.0;
    egui::Area::new(Id::new("plaque-inspector")).order(Order::Middle).fixed_pos(pos2(x, screen.min.y + 62.0)).show(ctx, |ui| {
        card().inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(INSP_W - 24.0);
            egui::ScrollArea::vertical().id_salt("plaque-insp").max_height(max_h - 24.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.set_width(INSP_W - 24.0);
                ui::properties(ui, st);
            });
        });
    });
}

/// Bottom: what the tool expects (mode dot + text), then the navigation and
/// the shape count.
fn status_bar(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let rect = Rect::from_min_max(pos2(screen.min.x, screen.max.y - STATUS_H), screen.max);
    let hint = st.hint();
    let dot = match &st.placement {
        Some(p) => ui::mode_color(p.mode),
        None => match st.tool {
            Tool::Place(_) => ui::mode_color(st.effective_mode()),
            _ => st.selection.and_then(|id| st.scene.node(id)).map(|n| ui::mode_color(n.combine.mode)).unwrap_or(pal().text2),
        },
    };
    let nav = match st.settings.navigation {
        Navigation::Maya => "Alt + clic gauche : orbite · Alt + milieu : pan · Alt + droit : zoom · F : cadrer",
        Navigation::Tripo => "Glisser dans le vide : orbite · droit : pan · molette : zoom · F : cadrer",
    };
    egui::Area::new(Id::new("plaque-status")).order(Order::Middle).fixed_pos(rect.min).interactable(false).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, 0.0, pal().well);
        p.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 1.0)), 0.0, pal().panel_border);
        p.circle_filled(pos2(r.min.x + 14.0, r.center().y), 3.5, dot);
        let right = format!("{nav}   |   {}", st.settings.navigation.label());
        let rg = p.layout_no_wrap(right, regular(11.5), pal().text2);
        let rx = r.max.x - 12.0 - rg.size().x;
        let mut job = egui::text::LayoutJob::single_section(hint, egui::TextFormat::simple(regular(12.0), pal().label));
        job.wrap = egui::text::TextWrapping { max_width: (rx - r.min.x - 40.0).max(60.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let hg = p.layout_job(job);
        p.galley(pos2(r.min.x + 26.0, r.center().y - hg.size().y * 0.5), hg, pal().label);
        p.galley(pos2(rx, r.center().y - rg.size().y * 0.5), rg, Color32::WHITE);
    });
}
