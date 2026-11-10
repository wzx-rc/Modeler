//! Studio interface: docked panels in the spirit of 3ds Max. Menu bar and
//! toolbar on top, scene explorer on the left, command panel (Créer /
//! Modifier / Affichage) on the right, history slider and status bar at the
//! bottom; the 3D view fills the centre. Same state, tools and renderer as
//! the Atelier interface (`ui.rs`), whose dialogs it reuses.

use crate::camera::ViewPreset;
use crate::icons::{self, Icon};
use crate::settings::UiMode;
use crate::state::*;
use crate::theme::{medium, regular, semibold, AXIS_X, AXIS_Y, AXIS_Z, CARVE, EDGE_SEL, OK};
use crate::ui;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Id, LayerId, Order, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui};
use forme_core::*;
use glam::{EulerRot, Quat, Vec3};

// Neutral greys of a classic DCC, one blue accent, yellow for the active view.
const B0: Color32 = Color32::from_rgb(0x1D, 0x1E, 0x20);
const B1: Color32 = Color32::from_rgb(0x26, 0x27, 0x2A);
const B2: Color32 = Color32::from_rgb(0x2E, 0x30, 0x34);
const B3: Color32 = Color32::from_rgb(0x3A, 0x3C, 0x41);
const B4: Color32 = Color32::from_rgb(0x46, 0x49, 0x4F);
const LINE: Color32 = Color32::from_rgb(0x15, 0x16, 0x18);
const T1: Color32 = Color32::from_rgb(0xD9, 0xDC, 0xE0);
const T2: Color32 = Color32::from_rgb(0x9A, 0xA0, 0xA8);
const T3: Color32 = Color32::from_rgb(0x6C, 0x72, 0x7B);
const SEL: Color32 = Color32::from_rgb(0x2F, 0x4F, 0x73);
const SEL_T: Color32 = Color32::from_rgb(0xE4, 0xEF, 0xFC);
const AC: Color32 = Color32::from_rgb(0x4B, 0x93, 0xE0);
const ACTIVE: Color32 = Color32::from_rgb(0xF2, 0xC1, 0x2E);
const PROMPT: Color32 = Color32::from_rgb(0xE6, 0xC3, 0x5C);

const MENU_H: f32 = 26.0;
const TOOL_H: f32 = 38.0;
const HIST_H: f32 = 34.0;
const STATUS_H: f32 = 26.0;
const LEFT_W: f32 = 262.0;
const RIGHT_W: f32 = 300.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Create,
    Modify,
    Display,
}

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    st.screen = [screen.width(), screen.height()];
    let top = screen.min.y + MENU_H + TOOL_H;
    let bottom = screen.max.y - STATUS_H - HIST_H;
    let view = Rect::from_min_max(pos2(screen.min.x + LEFT_W, top), pos2(screen.max.x - RIGHT_W, bottom));
    st.view_rect = [view.min.x, view.min.y, view.max.x, view.max.y];

    menu_bar(ctx, st, Rect::from_min_size(screen.min, vec2(screen.width(), MENU_H)));
    toolbar(ctx, st, Rect::from_min_size(screen.min + vec2(0.0, MENU_H), vec2(screen.width(), TOOL_H)));
    let left = Rect::from_min_max(pos2(screen.min.x, top), pos2(screen.min.x + LEFT_W, bottom));
    if st.bake.open {
        bake_panel(ctx, st, left);
    } else {
        explorer(ctx, st, left);
    }
    command_panel(ctx, st, Rect::from_min_max(pos2(screen.max.x - RIGHT_W, top), pos2(screen.max.x, screen.max.y - STATUS_H)));
    history_bar(ctx, st, Rect::from_min_max(pos2(screen.min.x, bottom), pos2(screen.max.x - RIGHT_W, screen.max.y - STATUS_H)));
    status_bar(ctx, st, Rect::from_min_max(pos2(screen.min.x, screen.max.y - STATUS_H), screen.max));
    view_frame(ctx, st, view);
    ui::dialogs(ctx, st, screen);
    ui::shortcuts(ctx, st);
}

// ------------------------------------------------------------------ style

/// Dense desktop widgets: small text, 20 pt rows, square-ish corners.
fn studio_style(s: &mut egui::Style) {
    s.text_styles.insert(egui::TextStyle::Body, regular(12.5));
    s.text_styles.insert(egui::TextStyle::Button, regular(12.5));
    s.text_styles.insert(egui::TextStyle::Small, regular(11.0));
    s.spacing.item_spacing = vec2(6.0, 4.0);
    s.spacing.button_padding = vec2(6.0, 2.0);
    s.spacing.interact_size = vec2(40.0, 20.0);
    s.spacing.icon_width = 12.0;
    s.spacing.icon_width_inner = 6.0;
    s.spacing.indent = 14.0;
    let v = &mut s.visuals;
    v.override_text_color = Some(T1);
    v.extreme_bg_color = B0;
    v.faint_bg_color = B2;
    v.selection.bg_fill = SEL;
    v.selection.stroke = Stroke::new(1.0, AC);
    v.window_fill = B2;
    v.panel_fill = B1;
    v.window_corner_radius = CornerRadius::same(4);
    v.menu_corner_radius = CornerRadius::same(4);
    v.window_stroke = Stroke::new(1.0, LINE);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(3);
    }
    v.widgets.inactive.bg_fill = B3;
    v.widgets.inactive.weak_bg_fill = B3;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x4A, 0x4D, 0x53));
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, T1);
    v.widgets.hovered.bg_fill = B4;
    v.widgets.hovered.weak_bg_fill = B4;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, AC);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.active.bg_fill = SEL;
    v.widgets.active.weak_bg_fill = SEL;
    v.widgets.active.bg_stroke = Stroke::new(1.0, AC);
    v.widgets.open.weak_bg_fill = B4;
    v.widgets.open.bg_fill = B4;
}

fn menu_cfg() -> egui::containers::menu::MenuConfig {
    egui::containers::menu::MenuConfig::new().style(|s: &mut egui::Style| {
        egui::containers::menu::menu_style(s);
        studio_style(s);
        s.spacing.button_padding = vec2(8.0, 3.0);
    })
}

/// A fixed, opaque docked region. Covers its whole rect so the viewport below
/// never gets the pointer there.
fn dock(ctx: &egui::Context, id: &str, rect: Rect, fill: Color32, add: impl FnOnce(&mut Ui)) {
    egui::Area::new(Id::new(id)).order(Order::Middle).fixed_pos(rect.min).constrain(false).show(ctx, |ui| {
        studio_style(ui.style_mut());
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        ui.painter().rect_filled(r, 0.0, fill);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(r);
        add(&mut child);
    });
}

fn header(ui: &mut Ui, title: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, 0.0, B2);
    p.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, LINE));
    p.text(r.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, title, semibold(12.0), T1);
}

fn tool_btn(ui: &mut Ui, icon: Icon, on: bool, tip: &str) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(30.0, 28.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect(r, 3.0, Color32::from_rgb(0x3D, 0x5F, 0x86), Stroke::new(1.0, Color32::from_rgb(0x5D, 0x8F, 0xCA)), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(r, 3.0, B3);
    }
    icons::paint(p, Rect::from_center_size(r.center(), vec2(17.0, 17.0)), icon, if on { Color32::WHITE } else { T1 });
    resp.on_hover_text(tip)
}

fn mode_tool_btn(ui: &mut Ui, m: CombineMode, on: bool) -> Response {
    let g = ui.painter().layout_no_wrap(m.label().to_string(), regular(12.0), T1);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 28.0, 26.0), Sense::click());
    let c = mode_color(m);
    let p = ui.painter();
    if on {
        p.rect(r, 3.0, crate::theme::tint(c, 0.18), Stroke::new(1.0, c), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(r, 3.0, B3);
    }
    ui::paint_mode_glyph(p, pos2(r.min.x + 11.0, r.center().y), m, c, 10.0);
    p.galley(pos2(r.min.x + 21.0, r.center().y - g.size().y * 0.5), g, if on { Color32::WHITE } else { T1 });
    resp.on_hover_text("Mode des nouvelles formes (et de la forme sélectionnée)")
}

fn text_btn(ui: &mut Ui, icon: Option<Icon>, label: &str, primary: bool) -> Response {
    let g = ui.painter().layout_no_wrap(label.to_string(), medium(12.5), Color32::WHITE);
    let w = g.size().x + 20.0 + if icon.is_some() { 20.0 } else { 0.0 };
    let (r, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
    let p = ui.painter();
    let fill = match (primary, resp.hovered()) {
        (true, false) => AC,
        (true, true) => Color32::from_rgb(0x5E, 0xA3, 0xEC),
        (false, false) => B3,
        (false, true) => B4,
    };
    p.rect_filled(r, 3.0, fill);
    let mut x = r.min.x + 10.0;
    if let Some(i) = icon {
        icons::paint(p, Rect::from_min_size(pos2(x, r.center().y - 7.5), vec2(15.0, 15.0)), i, Color32::WHITE);
        x += 20.0;
    }
    p.galley(pos2(x, r.center().y - g.size().y * 0.5), g, Color32::WHITE);
    resp
}

fn vsep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(9.0, 22.0), Sense::hover());
    ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, B4));
}

fn small_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(regular(11.5)).color(T3));
}

fn mode_color(m: CombineMode) -> Color32 {
    match m {
        CombineMode::Add => AC,
        CombineMode::Carve => CARVE,
        _ => Color32::from_rgb(0xC8, 0xCC, 0xD2),
    }
}

fn prim_icon(k: PrimKind) -> Icon {
    match k {
        PrimKind::Box => Icon::Box,
        PrimKind::Cylinder => Icon::Cylinder,
        PrimKind::Sphere => Icon::Sphere,
        PrimKind::Capsule => Icon::Capsule,
        PrimKind::Torus => Icon::Torus,
        PrimKind::Cone => Icon::Cone,
        PrimKind::Prism => Icon::Prism,
        PrimKind::Tube => Icon::Tube,
        PrimKind::Profile => Icon::Profile,
    }
}

/// True on the first change of a drag or typed edit: one history entry per
/// gesture.
fn live_start(ui: &Ui, resp: &Response) -> bool {
    let key = resp.id.with("live");
    let active = ui.data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
    let start = resp.changed() && !active;
    let now_active = resp.dragged() && (active || resp.changed());
    ui.data_mut(|d| d.insert_temp(key, now_active));
    start
}

/// "Label :  [ value ⇕ ]" row, label right-aligned like 3ds Max rollouts.
/// Returns (changed, first change of the gesture).
/// A millimetre field with a French decimal comma.
fn mm_drag(v: &mut f32, decimals: usize) -> egui::DragValue<'_> {
    egui::DragValue::new(v)
        .speed(0.1)
        .suffix(" mm")
        .custom_formatter(move |x, _| format!("{:.*}", decimals, x).replace('.', ","))
        .custom_parser(|t| t.replace(',', ".").trim_end_matches(|c: char| c.is_alphabetic() || c == ' ').parse().ok())
}

fn spin(ui: &mut Ui, label: &str, v: &mut f32, speed: f64, range: std::ops::RangeInclusive<f32>, suffix: &str, decimals: usize) -> (bool, bool) {
    let mut out = (false, false);
    ui.horizontal(|ui| {
        let (lr, _) = ui.allocate_exact_size(vec2(118.0, 20.0), Sense::hover());
        ui.painter().text(lr.right_center() - vec2(4.0, 0.0), Align2::RIGHT_CENTER, label, regular(12.0), T2);
        let resp = ui.add_sized(
            vec2(ui.available_width().min(132.0), 20.0),
            egui::DragValue::new(v).speed(speed).range(range).suffix(suffix).fixed_decimals(decimals).custom_formatter(move |x, _| format!("{:.*}", decimals, x).replace('.', ",") + "").custom_parser(|t| t.replace(',', ".").trim_end_matches(|c: char| c.is_alphabetic() || c == ' ').parse().ok()),
        );
        let start = live_start(ui, &resp);
        out = (resp.changed(), start);
    });
    out
}

fn rollout(ui: &mut Ui, id: &str, title: &str, open: bool, add: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).font(semibold(12.0)).color(T1)).id_salt(id).default_open(open).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        add(ui);
        ui.add_space(2.0);
    });
}

// --------------------------------------------------------------- menu bar

fn menu_bar(ctx: &egui::Context, st: &mut AppState, rect: Rect) {
    dock(ctx, "studio-menu", rect, B1, |ui| {
        ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, LINE));
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            egui::containers::menu::MenuBar::new().config(menu_cfg()).ui(ui, |ui| {
                ui.menu_button("Fichier", |ui| {
                    if ui.add(egui::Button::new("Nouveau · Ouvrir…")).clicked() {
                        st.files_open = true;
                    }
                    if ui.add(egui::Button::new("Enregistrer").shortcut_text("Ctrl+S")).clicked() {
                        st.save();
                    }
                    ui.separator();
                    if ui.button("Exporter le mesh…").clicked() {
                        st.export_open = true;
                    }
                    if ui.button("Bake et export en quads…").clicked() {
                        st.bake.open = true;
                    }
                    ui.separator();
                    if ui.button("Réglages…").clicked() {
                        st.settings_open = true;
                    }
                });
                ui.menu_button("Édition", |ui| {
                    if ui.add(egui::Button::new("Annuler").shortcut_text("Ctrl+Z")).clicked() {
                        st.undo();
                    }
                    if ui.add(egui::Button::new("Rétablir").shortcut_text("Ctrl+Y")).clicked() {
                        st.redo();
                    }
                    ui.separator();
                    let has = st.selection.is_some();
                    if ui.add_enabled(has, egui::Button::new("Dupliquer").shortcut_text("Ctrl+D")).clicked() {
                        st.duplicate_selected();
                    }
                    if ui.add_enabled(has, egui::Button::new("Supprimer").shortcut_text("Suppr")).clicked() {
                        st.delete_selected();
                    }
                    if ui.add_enabled(has, egui::Button::new("Grouper")).clicked() {
                        st.group_selected();
                    }
                    ui.separator();
                    if ui.add_enabled(has || !st.edge_sel.is_empty(), egui::Button::new("Désélectionner").shortcut_text("Échap")).clicked() {
                        st.clear_edge();
                        st.select(None);
                    }
                    ui.separator();
                    if ui.button("Réglages…").clicked() {
                        st.settings_open = true;
                    }
                });
                ui.menu_button("Créer", |ui| {
                    for k in st.settings.placeable() {
                        if ui.add(egui::Button::new(k.label()).selected(st.tool == Tool::Place(k))).clicked() {
                            st.commit_placement();
                            st.tool = Tool::Place(k);
                        }
                    }
                    ui.separator();
                    small_label(ui, "Mode des nouvelles formes");
                    for m in st.settings.modes() {
                        if ui.radio(st.mode == m, m.label()).clicked() {
                            st.mode = m;
                        }
                    }
                });
                ui.menu_button("Modifier", |ui| {
                    let target = st.selection.filter(|id| st.scene.node(*id).map(|n| n.shape().is_some()).unwrap_or(false));
                    small_label(ui, "Combinaison de la sélection");
                    for m in st.settings.modes() {
                        let cur = target.and_then(|id| st.scene.node(id)).map(|n| n.combine.mode);
                        if ui.add_enabled(target.is_some(), egui::RadioButton::new(cur == Some(m), m.label())).clicked() {
                            if let Some(id) = target {
                                st.edit("Mode", id, |n| n.combine.mode = m);
                            }
                        }
                    }
                    ui.separator();
                    if ui.checkbox(&mut st.sym_x, "Symétrie X des nouvelles formes").changed() {
                        st.view_changed();
                    }
                    if ui.add_enabled(st.selection.is_some(), egui::Button::new("Cadrer la sélection").shortcut_text("F")).clicked() {
                        st.frame_selection();
                    }
                });
                ui.menu_button("Affichage", |ui| {
                    let cur = st.camera.preset_of();
                    for (p, l) in presets() {
                        if ui.radio(cur == p, l).clicked() {
                            st.camera.preset(p);
                            st.view_changed();
                        }
                    }
                    ui.separator();
                    if ui.checkbox(&mut st.grid, "Grille").changed() {
                        st.view_changed();
                    }
                    ui.checkbox(&mut st.snap, "Snap");
                    if ui.button("Cadrer toute la scène").clicked() {
                        st.frame_all();
                    }
                    ui.separator();
                    if ui.button("Passer à l'interface Atelier").clicked() {
                        st.set_ui_mode(UiMode::Atelier);
                    }
                });
                ui.menu_button("Bake", |ui| {
                    let running = st.bake.job.is_some();
                    if ui.add_enabled(!running, egui::Button::new("Lancer le Bake")).clicked() {
                        st.start_bake();
                    }
                    if ui.button("Fenêtre Bake et export…").clicked() {
                        st.bake.open = true;
                    }
                    ui.separator();
                    let has = st.bake.result.is_some();
                    let mut show = st.bake.show;
                    if ui.add_enabled(has, egui::Checkbox::new(&mut show, "Afficher le maillage baké")).changed() {
                        st.bake.show = show;
                        st.view_changed();
                    }
                    let mut wire = st.bake.wire;
                    if ui.add_enabled(has, egui::Checkbox::new(&mut wire, "Filaire")).changed() {
                        st.bake.wire = wire;
                        st.view_changed();
                    }
                });
                ui.menu_button("Aide", |ui| {
                    small_label(ui, "Raccourcis");
                    for (k, what) in [
                        ("Ctrl+Z / Ctrl+Y", "annuler / rétablir"),
                        ("Alt maintenu", "creuser (Carve)"),
                        ("Ctrl + clic", "ajouter une arête"),
                        ("Clic droit glissé", "orbite"),
                        ("Molette · Maj + glisser", "zoom · pan"),
                        ("F", "cadrer la sélection"),
                        ("Suppr", "supprimer"),
                        ("Échap", "désélectionner"),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(k).font(medium(12.0)).color(T1));
                            ui.label(RichText::new(what).font(regular(12.0)).color(T2));
                        });
                    }
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                let r = ui.add(egui::Label::new(RichText::new("Interface : Studio").font(regular(12.0)).color(T2)).sense(Sense::click()));
                if r.on_hover_text("Changer d'interface (Réglages)").clicked() {
                    st.settings_open = true;
                }
            });
        });
    });
}

fn presets() -> [(ViewPreset, &'static str); 4] {
    [(ViewPreset::Perspective, "Perspective"), (ViewPreset::Front, "Face"), (ViewPreset::Right, "Droite"), (ViewPreset::Top, "Dessus")]
}

// ---------------------------------------------------------------- toolbar

fn toolbar(ctx: &egui::Context, st: &mut AppState, rect: Rect) {
    dock(ctx, "studio-tools", rect, B2, |ui| {
        ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, LINE));
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.add_space(6.0);
            if tool_btn(ui, Icon::Undo, false, "Annuler (Ctrl+Z)").clicked() {
                st.undo();
            }
            if tool_btn(ui, Icon::Redo, false, "Rétablir (Ctrl+Y)").clicked() {
                st.redo();
            }
            vsep(ui);
            let sel = st.tool == Tool::Select;
            if tool_btn(ui, Icon::Select, sel && st.pick_mode == PickMode::Object, "Sélection d'objets (1)").clicked() {
                st.set_pick_mode(PickMode::Object);
            }
            if tool_btn(ui, Icon::Fillet, sel && st.pick_mode == PickMode::Edge, "Sélection d'arêtes (2) : fillets, chanfreins").clicked() {
                st.set_pick_mode(PickMode::Edge);
            }
            vsep(ui);
            ui.label(RichText::new("Créer").font(regular(11.5)).color(T3));
            for k in st.settings.placeable() {
                if tool_btn(ui, prim_icon(k), st.tool == Tool::Place(k), &format!("{} : glisser la base, puis la hauteur", k.label())).clicked() {
                    st.commit_placement();
                    st.tool = Tool::Place(k);
                }
            }
            vsep(ui);
            let shown = st.effective_mode();
            for m in st.settings.modes() {
                if mode_tool_btn(ui, m, shown == m).clicked() {
                    st.set_mode(m);
                }
            }
            vsep(ui);
            if tool_btn(ui, Icon::Mirror, st.sym_x, "Symétrie X des nouvelles formes").clicked() {
                st.sym_x = !st.sym_x;
            }
            if tool_btn(ui, Icon::Magnet, st.snap, "Snap").clicked() {
                st.snap = !st.snap;
            }
            if tool_btn(ui, Icon::Grid, st.grid, "Grille").clicked() {
                st.grid = !st.grid;
                st.view_changed();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(6.0);
                let running = st.bake.job.is_some();
                if text_btn(ui, Some(Icon::Bake), if running { "Bake…" } else { "Bake" }, true).clicked() {
                    st.bake.open = true;
                }
                if text_btn(ui, Some(Icon::Export), "Exporter", false).clicked() {
                    st.export_open = true;
                }
                let r = tool_btn(ui, Icon::Light, st.display_open, "Affichage : lumière, ombres, anticrénelage");
                ui::light_toggle(st, &r);
                if tool_btn(ui, Icon::Settings, st.settings_open, "Réglages").clicked() {
                    st.settings_open = true;
                }
            });
        });
    });
}

// ---------------------------------------------------------- scene explorer

/// The Bake panel in place of the scene explorer (left column), with its
/// diagnostic layers; « Explorateur » goes back.
fn bake_panel(ctx: &egui::Context, st: &mut AppState, rect: Rect) {
    dock(ctx, "studio-bake", rect, B1, |ui| {
        ui.painter().line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, LINE));
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Bake").font(semibold(12.5)).color(T1));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                if text_btn(ui, None, "Explorateur", false).clicked() {
                    st.bake.open = false;
                }
            });
        });
        egui::ScrollArea::vertical().id_salt("studio-bake-scroll").auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                ui.set_width(rect.width() - 22.0);
                ui::bake_body(ui, st, true);
            });
        });
    });
}

fn explorer(ctx: &egui::Context, st: &mut AppState, rect: Rect) {
    dock(ctx, "studio-explorer", rect, B1, |ui| {
        ui.painter().line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, LINE));
        ui.spacing_mut().item_spacing.y = 0.0;
        header(ui, "Explorateur de scène");
        let w = rect.width() - 1.0;
        // column titles
        let (hr, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
        {
            let p = ui.painter();
            p.text(pos2(hr.min.x + 8.0, hr.center().y), Align2::LEFT_CENTER, "Nom", regular(11.5), T3);
            p.text(pos2(hr.max.x - 138.0, hr.center().y), Align2::LEFT_CENTER, "Type", regular(11.5), T3);
            p.text(pos2(hr.max.x - 78.0, hr.center().y), Align2::LEFT_CENTER, "Mode", regular(11.5), T3);
            p.line_segment([hr.left_bottom(), hr.right_bottom()], Stroke::new(1.0, B3));
        }
        let bake_h = 224.0;
        let list_h = (rect.height() - 24.0 - 22.0 - bake_h).max(60.0);
        let rows = st.scene.rows();
        egui::ScrollArea::vertical().id_salt("explorer-rows").max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            if rows.is_empty() {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new("Scène vide · Créer > Cube ou Cylindre").font(regular(12.0)).color(T2));
                });
            }
            for (i, (id, depth)) in rows.iter().enumerate() {
                explorer_row(ui, st, *id, *depth, i, w);
            }
        });
        // bake summary
        let (sep, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
        ui.painter().rect_filled(sep, 0.0, LINE);
        bake_box(ui, st);
    });
}

fn explorer_row(ui: &mut Ui, st: &mut AppState, id: NodeId, depth: usize, i: usize, w: f32) {
    let Some(n) = st.scene.node(id).cloned() else { return };
    let (r, resp) = ui.allocate_exact_size(vec2(w, 22.0), Sense::click());
    let selected = st.selection == Some(id);
    let dim = if n.visible { 1.0 } else { 0.45 };
    let p = ui.painter();
    if selected {
        p.rect_filled(r, 0.0, SEL);
    } else if resp.hovered() {
        p.rect_filled(r, 0.0, B2);
    } else if i % 2 == 1 {
        p.rect_filled(r, 0.0, Color32::from_rgb(0x29, 0x2A, 0x2D));
    }
    let x0 = r.min.x + 8.0 + depth as f32 * 14.0;
    let fg = if selected { SEL_T } else { crate::theme::tint(T1, dim) };
    let fg2 = if selected { SEL_T } else { crate::theme::tint(T2, dim) };
    let mut toggled = false;
    if n.is_group() {
        let cr = Rect::from_min_size(pos2(x0, r.center().y - 5.0), vec2(10.0, 10.0));
        icons::paint(p, cr, if n.expanded { Icon::Chevron } else { Icon::ChevronRight }, fg2);
        toggled = ui.interact(cr.expand(5.0), Id::new(("x-exp", id)), Sense::click()).clicked();
    }
    let p = ui.painter();
    let icon = n.shape().map(|s| prim_icon(s.kind)).unwrap_or(Icon::Folder);
    icons::paint(p, Rect::from_min_size(pos2(x0 + 12.0, r.center().y - 7.0), vec2(14.0, 14.0)), icon, fg2);
    let name_max = (r.max.x - 142.0) - (x0 + 32.0);
    let mut job = egui::text::LayoutJob::single_section(n.name.clone(), egui::TextFormat::simple(regular(12.0), fg));
    job.wrap = egui::text::TextWrapping { max_width: name_max.max(20.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = ui.painter().layout_job(job);
    let p = ui.painter();
    p.galley(pos2(x0 + 32.0, r.center().y - g.size().y * 0.5), g, fg);
    p.text(pos2(r.max.x - 138.0, r.center().y), Align2::LEFT_CENTER, n.type_label(), regular(12.0), fg2);
    if n.shape().is_some() || matches!(n.kind, NodeKind::Group { combined: true, .. }) {
        let c = mode_color(n.combine.mode);
        p.circle_filled(pos2(r.max.x - 74.0, r.center().y), 3.5, c);
        p.text(pos2(r.max.x - 66.0, r.center().y), Align2::LEFT_CENTER, n.combine.mode.label(), regular(12.0), fg2);
    }
    let er = Rect::from_center_size(pos2(r.max.x - 12.0, r.center().y), vec2(13.0, 13.0));
    icons::paint(p, er, if n.visible { Icon::Eye } else { Icon::EyeOff }, fg2);
    let eye = ui.interact(er.expand(4.0), Id::new(("x-eye", id)), Sense::click());
    if eye.clicked() {
        st.toggle_visible(id);
    } else if toggled {
        if let Some(nm) = st.scene.node_mut(id) {
            nm.expanded = !nm.expanded;
        }
    } else if resp.clicked() {
        st.commit_placement();
        st.tool = Tool::Select;
        st.select(Some(id));
    } else if resp.double_clicked() {
        st.frame_selection();
    }
}

fn bake_box(ui: &mut Ui, st: &mut AppState) {
    use forme_core::bake::Quality;
    let w = ui.available_width();
    ui.add_space(6.0);
    let stale = st.bake.stale(st.scene_version);
    let (state, color) = match (&st.bake.job, &st.bake.result) {
        (Some(_), _) => ("En cours…", AC),
        (None, Some(_)) if st.bake.pass.is_some() => ("Passe en cours…", AC),
        (None, Some(_)) if stale => ("Périmé", CARVE),
        (None, Some(_)) => ("À jour", OK),
        (None, None) => ("Pas encore baké", T3),
    };
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(RichText::new("Bake").font(semibold(12.0)).color(T1));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            ui.label(RichText::new(state).font(medium(11.5)).color(color));
        });
    });
    ui.add_space(4.0);
    let rows: Vec<(String, String)> = match &st.bake.result {
        Some(b) => {
            let s = &b.stats;
            let faces = b.faces.len().max(1);
            let mut rows: Vec<(String, String)> = vec![
                ("Méthode".into(), if s.constructive { "Par opération".into() } else { "Par surfaces (repli)".into() }),
                ("Faces".into(), fmt_int(b.faces.len())),
                ("Quads".into(), format!("{:.0} %", 100.0 * s.quads as f64 / faces as f64)),
                ("Bords ouverts".into(), fmt_int(s.open_edges)),
                ("Non-manifold".into(), fmt_int(s.nonmanifold_edges)),
                ("Écart max".into(), format!("{:.3} mm", s.max_dev).replace('.', ",")),
                ("Durée".into(), if s.seconds < 1.0 { format!("{:.0} ms", s.seconds * 1000.0) } else { format!("{:.1} s", s.seconds).replace('.', ",") }),
            ];
            if st.bake.flat && s.faces_before_flat > 0 {
                rows.push(("Loops plates".into(), format!("{} · {} → {}", s.flat_loops, fmt_int(s.faces_before_flat), fmt_int(b.faces.len()))));
            }
            rows
        }
        None => vec![("Méthode".into(), "—".into()), ("Faces".into(), "—".into()), ("Quads".into(), "—".into())],
    };
    for (k, v) in rows {
        let (r, _) = ui.allocate_exact_size(vec2(w, 17.0), Sense::hover());
        let p = ui.painter();
        p.text(pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, k, regular(12.0), T2);
        p.text(pos2(r.max.x - 10.0, r.center().y), Align2::RIGHT_CENTER, v, medium(12.0), T1);
    }
    ui.add_space(4.0);
    // optional second pass, off by default
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let mut on = st.bake.flat;
        let deg = forme_core::bake::decimate::FLAT_LOOPS_DEG;
        let r = ui.checkbox(&mut on, format!("Retirer les loops plates (< {deg:.0}°)"));
        if r.on_hover_text("Passe secondaire en option, désactivée par défaut : fusionne les quads des edge loops fermées qui ne font pas de volume.").changed() {
            st.set_flat_pass(on);
        }
    });
    ui.add_space(4.0);
    // density + action on one line
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        egui::ComboBox::from_id_salt("bake-quality").width(110.0).selected_text(format!("Densité : {}", st.bake.quality.label())).show_ui(ui, |ui| {
            for q in [Quality::Draft, Quality::Standard, Quality::Max] {
                ui.selectable_value(&mut st.bake.quality, q, q.label());
            }
        });
        if st.bake.job.is_some() {
            if text_btn(ui, None, "Annuler", false).clicked() {
                st.cancel_bake();
            }
        } else if text_btn(ui, Some(Icon::Bake), "Bake", true).clicked() {
            st.start_bake();
        }
    });
}

// ----------------------------------------------------------- command panel

fn command_panel(ctx: &egui::Context, st: &mut AppState, rect: Rect) {
    let tab_id = Id::new("studio-tab");
    let mut tab = ctx.data(|d| d.get_temp::<u8>(tab_id)).map(|t| match t {
        0 => Tab::Create,
        2 => Tab::Display,
        _ => Tab::Modify,
    });
    // like 3ds Max: picking another object opens Modifier, starting a
    // placement opens Créer; otherwise the tab stays where the user put it
    let seen_id = Id::new("studio-tab-seen");
    let now = (st.selection, matches!(st.tool, Tool::Place(_)));
    let seen = ctx.data(|d| d.get_temp::<(Option<NodeId>, bool)>(seen_id));
    if seen != Some(now) {
        if now.1 && st.selection.is_none() && seen.map_or(true, |s| !s.1) {
            tab = Some(Tab::Create);
        } else if st.selection.is_some() && seen.map_or(true, |s| s.0 != st.selection) {
            tab = Some(Tab::Modify);
        }
        ctx.data_mut(|d| d.insert_temp(seen_id, now));
    }
    let mut tab = tab.unwrap_or(if st.selection.is_some() { Tab::Modify } else { Tab::Create });
    dock(ctx, "studio-command", rect, B1, |ui| {
        ui.painter().line_segment([rect.left_top(), rect.left_bottom()], Stroke::new(1.0, LINE));
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let w = rect.width();
        let (tr, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
        ui.painter().rect_filled(tr, 0.0, B2);
        ui.painter().line_segment([tr.left_bottom(), tr.right_bottom()], Stroke::new(1.0, LINE));
        let tabs = [(Tab::Create, Icon::Box, "Créer"), (Tab::Modify, Icon::Settings, "Modifier"), (Tab::Display, Icon::Eye, "Affichage")];
        let tw = w / 3.0;
        for (i, (t, icon, label)) in tabs.iter().enumerate() {
            let r = Rect::from_min_size(tr.min + vec2(i as f32 * tw, 0.0), vec2(tw, 30.0));
            let resp = ui.interact(r, Id::new(("ctab", i)), Sense::click());
            let on = tab == *t;
            let p = ui.painter();
            if on {
                p.rect_filled(r, 0.0, B1);
                p.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 2.0)), 0.0, AC);
            } else if resp.hovered() {
                p.rect_filled(r, 0.0, B3);
            }
            if i > 0 {
                p.line_segment([r.left_top(), r.left_bottom()], Stroke::new(1.0, LINE));
            }
            let g = p.layout_no_wrap(label.to_string(), medium(12.0), if on { T1 } else { T2 });
            let x0 = r.center().x - (g.size().x + 20.0) * 0.5;
            icons::paint(p, Rect::from_min_size(pos2(x0, r.center().y - 7.0), vec2(14.0, 14.0)), *icon, if on { T1 } else { T2 });
            p.galley(pos2(x0 + 20.0, r.center().y - g.size().y * 0.5), g, if on { T1 } else { T2 });
            if resp.clicked() {
                tab = *t;
            }
        }
        let body_h = rect.height() - 30.0;
        egui::ScrollArea::vertical().id_salt("command-scroll").max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 8)).show(ui, |ui| {
                ui.set_width(w - 18.0);
                ui.spacing_mut().item_spacing = vec2(6.0, 5.0);
                match tab {
                    Tab::Create => create_tab(ui, st),
                    Tab::Modify => modify_tab(ui, st),
                    Tab::Display => display_tab(ui, st),
                }
            });
        });
    });
    let code = match tab {
        Tab::Create => 0u8,
        Tab::Modify => 1,
        Tab::Display => 2,
    };
    ctx.data_mut(|d| d.insert_temp(tab_id, code));
}

fn create_tab(ui: &mut Ui, st: &mut AppState) {
    rollout(ui, "c-prims", "Primitives", true, |ui| {
        for k in st.settings.placeable() {
            let on = st.tool == Tool::Place(k);
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
            let p = ui.painter();
            p.rect(r, 3.0, if on { SEL } else if resp.hovered() { B4 } else { B3 }, Stroke::new(1.0, if on { AC } else { Color32::from_rgb(0x4A, 0x4D, 0x53) }), StrokeKind::Inside);
            icons::paint(p, Rect::from_min_size(pos2(r.min.x + 10.0, r.center().y - 8.0), vec2(16.0, 16.0)), prim_icon(k), if on { Color32::WHITE } else { T1 });
            p.text(pos2(r.min.x + 34.0, r.center().y), Align2::LEFT_CENTER, k.label(), medium(12.5), if on { Color32::WHITE } else { T1 });
            if resp.clicked() {
                st.commit_placement();
                st.tool = Tool::Place(k);
            }
        }
    });
    rollout(ui, "c-mode", "Mode des nouvelles formes", true, |ui| {
        ui.horizontal_wrapped(|ui| {
            for m in st.settings.modes() {
                if ui.radio(st.mode == m, m.label()).clicked() {
                    st.mode = m;
                }
            }
        });
        ui.checkbox(&mut st.sym_x, "Symétrie X (doubler les nouvelles formes)");
    });
    rollout(ui, "c-how", "Méthode", true, |ui| {
        for line in [
            "Glisser dans la vue : la base (le cube se trace coin à coin, le cylindre depuis le centre).",
            "Relâcher, puis monter ou descendre pour la hauteur ; clic pour valider.",
            "Clic simple : taille par défaut.",
            "Alt maintenu : la forme creuse (Carve).",
        ] {
            ui.label(RichText::new(line).font(regular(12.0)).color(T2));
        }
    });
}

fn modify_tab(ui: &mut Ui, st: &mut AppState) {
    let Some(id) = st.selection else {
        ui.label(RichText::new("Aucune sélection").font(semibold(12.5)).color(T1));
        ui.label(RichText::new("Clique une forme dans la vue ou dans l'explorateur de scène.").font(regular(12.0)).color(T2));
        return;
    };
    let Some(node) = st.scene.node(id).cloned() else { return };

    // name + summary of the operation, as a modifier stack
    let mut name = node.name.clone();
    let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY).font(medium(12.5)));
    if r.changed() {
        st.edit("Renommer", id, |n| n.name = name.clone());
    }
    small_label(ui, "Pile d'opérations");
    egui::Frame::new().fill(B0).stroke(Stroke::new(1.0, B3)).corner_radius(CornerRadius::same(3)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let w = ui.available_width();
        let mut lines: Vec<(String, String, bool)> = vec![];
        if let Some(sh) = node.shape() {
            let edges = if sh.edges.is_empty() {
                if sh.round > 0.0 || sh.chamfer > 0.0 {
                    "uniformes".to_string()
                } else {
                    "vives".to_string()
                }
            } else {
                format!("{} réglées", sh.edges.iter().filter(|r| **r != 0.0).count())
            };
            lines.push(("Arêtes".into(), edges, false));
            lines.push((format!("Combinaison · {}", node.combine.mode.label()), if node.combine.blend > 0.0 { format!("raccord {}", fmt_mm2(node.combine.blend)) } else { "vif".into() }, false));
            lines.push((sh.kind.label().to_string(), "forme de base".into(), true));
        } else {
            lines.push((node.type_label(), format!("{} éléments", node.children().len()), true));
        }
        for (a, b, base) in lines {
            let (r, _) = ui.allocate_exact_size(vec2(w, 21.0), Sense::hover());
            let p = ui.painter();
            if base {
                p.rect_filled(r, 0.0, SEL);
            }
            p.circle(pos2(r.min.x + 12.0, r.center().y), 4.0, Color32::from_rgb(0xE7, 0xE9, 0xEC), Stroke::new(1.0, Color32::from_gray(0x88)));
            p.text(pos2(r.min.x + 24.0, r.center().y), Align2::LEFT_CENTER, a, regular(12.0), if base { Color32::WHITE } else { T1 });
            p.text(pos2(r.max.x - 8.0, r.center().y), Align2::RIGHT_CENTER, b, regular(11.5), if base { SEL_T } else { T3 });
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if tool_btn(ui, Icon::Duplicate, false, "Dupliquer (Ctrl+D)").clicked() {
            st.duplicate_selected();
        }
        if tool_btn(ui, if node.visible { Icon::Eye } else { Icon::EyeOff }, false, "Masquer / afficher").clicked() {
            st.toggle_visible(id);
        }
        if tool_btn(ui, Icon::Frame, false, "Cadrer (F)").clicked() {
            st.frame_selection();
        }
        if tool_btn(ui, Icon::Trash, false, "Supprimer (Suppr)").clicked() {
            st.delete_selected();
        }
    });
    if st.selection.is_none() {
        return;
    }

    if let Some(sh) = node.shape().cloned() {
        rollout(ui, "m-params", "Paramètres", true, |ui| shape_params(ui, st, id, &sh));
    }
    if node.shape().is_some() || matches!(node.kind, NodeKind::Group { combined: true, .. }) {
        rollout(ui, "m-combine", "Combinaison", true, |ui| {
            ui.horizontal_wrapped(|ui| {
                for m in st.settings.modes() {
                    if ui.radio(node.combine.mode == m, m.label()).clicked() {
                        st.mode = m;
                        st.edit("Mode", id, |n| n.combine.mode = m);
                    }
                }
            });
            ui.horizontal(|ui| {
                if ui.radio(node.combine.kind == BlendKind::Smooth, "Raccord lisse").clicked() {
                    st.edit("Type de raccord", id, |n| n.combine.kind = BlendKind::Smooth);
                }
                if ui.radio(node.combine.kind == BlendKind::Chamfer, "Chanfrein").clicked() {
                    st.edit("Type de raccord", id, |n| n.combine.kind = BlendKind::Chamfer);
                }
            });
            let mut b = node.combine.blend;
            let (ch, start) = spin(ui, "Rayon du raccord :", &mut b, 0.05, 0.0..=12.0, " mm", 2);
            if ch {
                st.edit_live("Raccord", id, start, |n| n.combine.blend = b);
            }
        });
    }
    rollout(ui, "m-edges", "Arêtes", true, |ui| edges_rollout(ui, st));
    rollout(ui, "m-sym", "Symétrie", false, |ui| {
        ui.horizontal(|ui| {
            let m = node.mirror;
            for (i, l, on) in [(0, "X", m.x), (1, "Y", m.y), (2, "Z", m.z)] {
                let mut v = on;
                if ui.checkbox(&mut v, l).changed() {
                    st.edit("Symétrie", id, |n| match i {
                        0 => n.mirror.x = v,
                        1 => n.mirror.y = v,
                        _ => n.mirror.z = v,
                    });
                }
            }
        });
    });
    rollout(ui, "m-xform", "Transformation", false, |ui| {
        let mut pos = node.transform.pos;
        for (i, l) in ["Position X :", "Position Y :", "Position Z :"].iter().enumerate() {
            let (ch, start) = spin(ui, l, &mut pos[i], 0.1, -10000.0..=10000.0, " mm", 2);
            if ch {
                st.edit_live("Position", id, start, |n| n.transform.pos = pos);
            }
        }
        let (ex, ey, ez) = node.transform.rot.to_euler(EulerRot::XYZ);
        let mut rot = Vec3::new(ex.to_degrees(), ey.to_degrees(), ez.to_degrees());
        for (i, l) in ["Rotation X :", "Rotation Y :", "Rotation Z :"].iter().enumerate() {
            let (ch, start) = spin(ui, l, &mut rot[i], 0.5, -360.0..=360.0, "°", 1);
            if ch {
                let q = Quat::from_euler(EulerRot::XYZ, rot.x.to_radians(), rot.y.to_radians(), rot.z.to_radians());
                st.edit_live("Rotation", id, start, |n| n.transform.rot = q);
            }
        }
    });
}

fn fmt_mm2(v: f32) -> String {
    format!("{:.2} mm", v).replace('.', ",")
}

/// Dimensions as the user reads them (full width / height, radius).
fn shape_params(ui: &mut Ui, st: &mut AppState, id: NodeId, sh: &Shape) {
    let mut s = sh.clone();
    let mut any = false;
    let mut start = false;
    {
        let mut row = |ui: &mut Ui, label: &str, v: &mut f32, scale: f32, max: f32| {
            let mut shown = *v * scale;
            let (c, s0) = spin(ui, label, &mut shown, 0.1, 0.1..=max, " mm", 2);
            if c {
                *v = shown / scale;
                any = true;
                start |= s0;
            }
        };
        match sh.kind {
            PrimKind::Box => {
                row(ui, "Largeur :", &mut s.size.x, 2.0, 400.0);
                row(ui, "Hauteur :", &mut s.size.y, 2.0, 400.0);
                row(ui, "Profondeur :", &mut s.size.z, 2.0, 400.0);
            }
            PrimKind::Cylinder | PrimKind::Prism => {
                row(ui, "Rayon :", &mut s.size.x, 1.0, 200.0);
                row(ui, "Hauteur :", &mut s.size.y, 2.0, 400.0);
            }
            PrimKind::Sphere => {
                row(ui, "Rayon X :", &mut s.size.x, 1.0, 200.0);
                row(ui, "Rayon Y :", &mut s.size.y, 1.0, 200.0);
                row(ui, "Rayon Z :", &mut s.size.z, 1.0, 200.0);
            }
            PrimKind::Capsule => {
                row(ui, "Rayon :", &mut s.size.x, 1.0, 200.0);
                row(ui, "Longueur :", &mut s.size.y, 2.0, 400.0);
            }
            PrimKind::Torus => {
                row(ui, "Rayon :", &mut s.size.x, 1.0, 200.0);
                row(ui, "Épaisseur :", &mut s.size.z, 1.0, 80.0);
            }
            PrimKind::Cone => {
                row(ui, "Rayon bas :", &mut s.size.x, 1.0, 200.0);
                row(ui, "Rayon haut :", &mut s.size.z, 1.0, 200.0);
                row(ui, "Hauteur :", &mut s.size.y, 2.0, 400.0);
            }
            PrimKind::Tube => {
                row(ui, "Rayon ext. :", &mut s.size.x, 1.0, 200.0);
                row(ui, "Rayon int. :", &mut s.size.z, 1.0, 200.0);
                row(ui, "Hauteur :", &mut s.size.y, 2.0, 400.0);
            }
            PrimKind::Profile => {
                row(ui, "Épaisseur :", &mut s.size.z, 2.0, 200.0);
            }
        }
    }
    if matches!(sh.kind, PrimKind::Box | PrimKind::Cylinder) {
        let max_round = match sh.kind {
            PrimKind::Box => s.size.min_element(),
            _ => s.size.x.min(s.size.y.max(0.1)),
        };
        let (r0, c0) = (s.round, s.chamfer);
        let (c, s0) = spin(ui, "Arrondi (toutes) :", &mut s.round, 0.05, 0.0..=max_round.max(0.1), " mm", 2);
        any |= c;
        start |= s0;
        let (c, s0) = spin(ui, "Chanfrein (toutes) :", &mut s.chamfer, 0.05, 0.0..=max_round.max(0.1), " mm", 2);
        any |= c;
        start |= s0;
        if s.round != r0 || s.chamfer != c0 {
            s.edges.clear();
        }
        if !sh.edges.is_empty() {
            ui.label(RichText::new("Arêtes réglées une par une : les valeurs « toutes » les remettent pareilles.").font(regular(11.5)).color(EDGE_SEL));
        }
    }
    if any {
        st.edit_live("Forme", id, start, |n| {
            if let NodeKind::Shape(x) = &mut n.kind {
                *x = s.clone();
            }
        });
    }
}

fn edges_rollout(ui: &mut Ui, st: &mut AppState) {
    let Some(e) = st.edge_sel.last().cloned() else {
        ui.label(RichText::new("Survole une arête dans la vue puis clique. Ctrl + clic pour en ajouter ou en retirer.").font(regular(12.0)).color(T2));
        return;
    };
    let n = st.edge_sel.len();
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 4.0, EDGE_SEL);
        let title = if n > 1 { format!("{n} arêtes sélectionnées") } else { e.label.clone() };
        ui.label(RichText::new(title).font(medium(12.5)).color(T1));
    });
    ui.horizontal(|ui| {
        if ui.radio(!e.chamfer, "Arrondi").clicked() {
            st.set_fillet(e.radius, false, true);
        }
        if ui.radio(e.chamfer, "Chanfrein").clicked() {
            st.set_fillet(e.radius, true, true);
        }
    });
    let max = st.fillet_max().max(0.1);
    let mut r = e.radius;
    let (ch, start) = spin(ui, if e.chamfer { "Chanfrein :" } else { "Rayon :" }, &mut r, 0.05, 0.0..=max, " mm", 2);
    if ch {
        st.set_fillet(r, e.chamfer, start);
    }
    ui.horizontal(|ui| {
        if text_btn(ui, None, "Arête vive", false).clicked() {
            st.set_fillet(0.0, e.chamfer, true);
        }
        if text_btn(ui, None, "Désélectionner", false).clicked() {
            st.clear_edge();
        }
    });
}

fn display_tab(ui: &mut Ui, st: &mut AppState) {
    rollout(ui, "d-view", "Vue", true, |ui| {
        let cur = st.camera.preset_of();
        ui.horizontal_wrapped(|ui| {
            for (p, l) in presets() {
                if ui.radio(cur == p, l).clicked() {
                    st.camera.preset(p);
                    st.view_changed();
                }
            }
        });
        if ui.checkbox(&mut st.grid, "Grille").changed() {
            st.view_changed();
        }
        ui.horizontal(|ui| {
            if text_btn(ui, Some(Icon::Frame), "Cadrer tout", false).clicked() {
                st.frame_all();
            }
            if text_btn(ui, None, "Cadrer la sélection", false).clicked() {
                st.frame_selection();
            }
        });
    });
    rollout(ui, "d-bake", "Maillage baké", true, |ui| {
        let has = st.bake.result.is_some();
        let mut show = st.bake.show;
        if ui.add_enabled(has, egui::Checkbox::new(&mut show, "Afficher le maillage baké")).changed() {
            st.bake.show = show;
            st.view_changed();
        }
        let mut wire = st.bake.wire;
        if ui.add_enabled(has, egui::Checkbox::new(&mut wire, "Filaire")).changed() {
            st.bake.wire = wire;
            st.view_changed();
        }
        if !has {
            ui.label(RichText::new("Lance un Bake (en bas de l'explorateur) pour voir le maillage en quads.").font(regular(12.0)).color(T2));
        }
    });
    rollout(ui, "d-ui", "Interface", true, |ui| {
        ui.label(RichText::new("Studio : panneaux ancrés, façon 3ds Max.").font(regular(12.0)).color(T2));
        if text_btn(ui, None, "Passer à l'interface Atelier", false).clicked() {
            st.set_ui_mode(UiMode::Atelier);
        }
    });
}

// ----------------------------------------------------- history and status

fn history_bar(ctx: &egui::Context, st: &mut AppState, rect: Rect) {
    dock(ctx, "studio-history", rect, B2, |ui| {
        ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, LINE));
        let rows: Vec<NodeId> = st.scene.rows().into_iter().map(|(id, _)| id).collect();
        let cur = st.selection.and_then(|s| rows.iter().position(|r| *r == s));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Historique").font(semibold(12.0)).color(T2));
            let prev = tool_btn(ui, Icon::ChevronLeft, false, "Opération précédente");
            let track_w = ui.available_width() - 44.0;
            let (tr, _) = ui.allocate_exact_size(vec2(track_w, 22.0), Sense::hover());
            let next = tool_btn(ui, Icon::ChevronRight, false, "Opération suivante");
            let p = ui.painter();
            p.rect(tr, 3.0, B0, Stroke::new(1.0, B3), StrokeKind::Inside);
            let n = rows.len();
            if n == 0 {
                p.text(tr.center(), Align2::CENTER_CENTER, "Aucune opération", regular(11.5), T3);
            }
            let x_of = |i: usize| tr.min.x + 34.0 + (tr.width() - 68.0) * if n > 1 { i as f32 / (n - 1) as f32 } else { 0.5 };
            if let Some(c) = cur {
                p.rect_filled(Rect::from_min_max(tr.min, pos2(x_of(c), tr.max.y)), 3.0, Color32::from_rgba_unmultiplied(0x4B, 0x93, 0xE0, 40));
            }
            for (i, id) in rows.iter().enumerate() {
                let x = x_of(i);
                let name = st.scene.node(*id).map(|n| n.name.clone()).unwrap_or_default();
                let on = cur == Some(i);
                let mut job = egui::text::LayoutJob::single_section(format!("{} · {}", i + 1, name), egui::TextFormat::simple(regular(11.0), if on { Color32::WHITE } else { T2 }));
                job.wrap = egui::text::TextWrapping { max_width: 140.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let g = p.layout_job(job);
                // the label stays inside the track at both ends
                let half = g.size().x * 0.5 + 4.0;
                let cx = x.clamp(tr.min.x + half + 2.0, (tr.max.x - half - 2.0).max(tr.min.x + half + 2.0));
                let hit = Rect::from_center_size(pos2(cx, tr.center().y), vec2(half * 2.0, 22.0));
                let resp = ui.interact(hit, Id::new(("hist-tick", i)), Sense::click());
                let p = ui.painter();
                p.line_segment([pos2(x, tr.min.y), pos2(x, tr.min.y + 5.0)], Stroke::new(1.0, B4));
                if on {
                    p.rect_filled(hit, 3.0, Color32::from_rgb(0x5D, 0x8F, 0xCA));
                } else if resp.hovered() {
                    p.rect_filled(hit, 3.0, B3);
                }
                p.galley(hit.center() - g.size() * 0.5, g, T2);
                if resp.on_hover_text("Sélectionner cette opération").clicked() {
                    st.commit_placement();
                    st.tool = Tool::Select;
                    st.select(Some(*id));
                }
            }
            if n > 0 {
                if prev.clicked() {
                    let i = cur.map(|c| c.saturating_sub(1)).unwrap_or(n - 1);
                    st.select(Some(rows[i]));
                }
                if next.clicked() {
                    let i = cur.map(|c| (c + 1).min(n - 1)).unwrap_or(0);
                    st.select(Some(rows[i]));
                }
            }
        });
    });
}

fn status_bar(ctx: &egui::Context, st: &mut AppState, rect: Rect) {
    dock(ctx, "studio-status", rect, B0, |ui| {
        ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, LINE));
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_space(8.0);
            let sel = match (st.selection, st.edge_sel.len()) {
                (_, n) if n > 1 => format!("{n} arêtes sélectionnées"),
                (_, 1) => "1 arête sélectionnée".to_string(),
                (Some(_), _) => "1 objet sélectionné".to_string(),
                (None, _) => "Aucune sélection".to_string(),
            };
            ui.label(RichText::new(sel).font(regular(12.0)).color(T2));
            vsep(ui);
            let right_w = 470.0;
            let hint_w = (ui.available_width() - right_w).max(80.0);
            let mut job = egui::text::LayoutJob::single_section(st.hint(), egui::TextFormat::simple(regular(12.0), PROMPT));
            job.wrap = egui::text::TextWrapping { max_width: hint_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            let g = ui.painter().layout_job(job);
            let (hr, _) = ui.allocate_exact_size(vec2(hint_w, 20.0), Sense::hover());
            ui.painter().galley(pos2(hr.min.x, hr.center().y - g.size().y * 0.5), g, PROMPT);
            // exact position of the selection
            let target = st.selection.and_then(|id| st.scene.node(id).map(|n| (id, n.transform.pos)));
            for (i, (l, c)) in [("X", AXIS_X), ("Y", AXIS_Y), ("Z", AXIS_Z)].iter().enumerate() {
                ui.label(RichText::new(*l).font(semibold(12.0)).color(*c));
                match target {
                    Some((id, mut pos)) => {
                        let resp = ui.add_sized(vec2(78.0, 18.0), mm_drag(&mut pos[i], 2));
                        let start = live_start(ui, &resp);
                        if resp.changed() {
                            st.edit_live("Position", id, start, |n| n.transform.pos = pos);
                        }
                    }
                    None => {
                        ui.add_sized(vec2(78.0, 18.0), egui::Label::new(RichText::new("—").color(T3)));
                    }
                }
            }
            vsep(ui);
            ui.label(RichText::new(if st.snap { "Snap activé" } else { "Snap désactivé" }).font(regular(12.0)).color(T2));
            if tool_btn(ui, Icon::Frame, false, "Cadrer la sélection (F)").clicked() {
                st.frame_selection();
            }
        });
    });
}

// ------------------------------------------------------------ view frame

/// Border and label of the 3D view: "[Perspective] [Ombré]", the view menu.
fn view_frame(ctx: &egui::Context, st: &mut AppState, view: Rect) {
    let p = ctx.layer_painter(LayerId::new(Order::Middle, Id::new("studio-view-border")));
    p.rect_stroke(view.shrink(1.0), 0.0, Stroke::new(2.0, ACTIVE), StrokeKind::Inside);
    egui::Area::new(Id::new("studio-view-label")).order(Order::Foreground).fixed_pos(view.min + vec2(8.0, 6.0)).show(ctx, |ui| {
        studio_style(ui.style_mut());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let cur = st.camera.preset_of();
            let name = presets().iter().find(|(p, _)| *p == cur).map(|(_, l)| *l).unwrap_or("Perspective");
            let shading = if st.bake.show && st.bake.result.is_some() { "Maillage baké" } else { "Ombré" };
            ui.style_mut().visuals.widgets.inactive.weak_bg_fill = Color32::from_black_alpha(90);
            ui.style_mut().visuals.widgets.inactive.bg_stroke = Stroke::NONE;
            ui.menu_button(RichText::new(format!("[{name}]")).font(regular(12.0)).color(T1), |ui| {
                for (p, l) in presets() {
                    if ui.radio(cur == p, l).clicked() {
                        st.camera.preset(p);
                        st.view_changed();
                    }
                }
                ui.separator();
                if ui.button("Cadrer tout").clicked() {
                    st.frame_all();
                }
            });
            ui.menu_button(RichText::new(format!("[{shading}]")).font(regular(12.0)).color(T1), |ui| {
                let has = st.bake.result.is_some();
                if ui.radio(!st.bake.show || !has, "Ombré").clicked() {
                    st.bake.show = false;
                    st.view_changed();
                }
                if ui.add_enabled(has, egui::RadioButton::new(st.bake.show && has, "Maillage baké")).clicked() {
                    st.bake.show = true;
                    st.view_changed();
                }
                let mut g = st.grid;
                if ui.checkbox(&mut g, "Grille").changed() {
                    st.grid = g;
                    st.view_changed();
                }
            });
        });
    });
}
