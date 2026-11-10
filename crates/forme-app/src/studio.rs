//! Studio interface: a docked CAD station in the spirit of Plasticity. Top bar
//! with the menus, the document and the command palette (Espace); a slim tool
//! column on the left; a fixed panel on the right (scene, properties,
//! history); a status bar with the shortcuts of the chosen navigation. Its own
//! components: the command palette and the sizes panel (typed dimensions while
//! a creation tool is armed). Same state, tools and renderer as Établi
//! (`ui.rs`), whose dialogs it reuses.

use crate::camera::ViewPreset;
use crate::icons::{self, Icon};
use crate::settings::{Navigation, UiMode};
use crate::state::*;
use crate::theme::{accent, accent_bg, accent_border, accent_text, carve, medium, regular, semibold, tint, AXIS_X, AXIS_Y, AXIS_Z, EDGE_SEL};
use crate::ui;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Id, Order, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui};
use forme_core::*;
use glam::{EulerRot, Quat, Vec3};

// Graphite of the redesign (same family as Établi, flatter: docked panels).
const B0: Color32 = Color32::from_rgb(0x0F, 0x11, 0x14);
const B1: Color32 = Color32::from_rgb(0x17, 0x19, 0x1D);
const B2: Color32 = Color32::from_rgb(0x1E, 0x21, 0x26);
const B3: Color32 = Color32::from_rgb(0x25, 0x29, 0x30);
const B4: Color32 = Color32::from_rgb(0x2C, 0x31, 0x39);
const LINE: Color32 = Color32::from_rgb(0x2A, 0x2E, 0x35);
const T1: Color32 = Color32::from_rgb(0xE4, 0xE7, 0xEB);
const T2: Color32 = Color32::from_rgb(0x9A, 0xA1, 0xAB);
const T3: Color32 = Color32::from_rgb(0x64, 0x6B, 0x75);

const TOP_H: f32 = 38.0;
const RAIL_W: f32 = 48.0;
const RIGHT_W: f32 = 316.0;
const STATUS_H: f32 = 28.0;
const HIST_H: f32 = 168.0;

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    st.screen = [screen.width(), screen.height()];
    let view = Rect::from_min_max(pos2(screen.min.x + RAIL_W, screen.min.y + TOP_H), pos2(screen.max.x - RIGHT_W, screen.max.y - STATUS_H));
    st.set_view_rect([view.min.x, view.min.y, view.max.x, view.max.y]);

    top_bar(ctx, st, Rect::from_min_size(screen.min, vec2(screen.width(), TOP_H)));
    rail(ctx, st, Rect::from_min_max(pos2(screen.min.x, view.min.y), pos2(view.min.x, view.max.y)));
    side_panel(ctx, st, Rect::from_min_max(pos2(view.max.x, view.min.y), pos2(screen.max.x, view.max.y)));
    status_bar(ctx, st, Rect::from_min_max(pos2(screen.min.x, view.max.y), screen.max));
    view_chips(ctx, st, view);
    sizes_panel(ctx, st, view);
    palette(ctx, st, screen);
    ui::dialogs(ctx, st, screen);
    // Espace: the command palette (never while typing)
    if !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        open_palette(ctx);
    }
    ui::shortcuts(ctx, st);
}

// ------------------------------------------------------------------ style

/// Dense desktop widgets: small text, 22 pt rows, 6 pt corners.
fn studio_style(s: &mut egui::Style) {
    s.text_styles.insert(egui::TextStyle::Body, regular(12.5));
    s.text_styles.insert(egui::TextStyle::Button, regular(12.5));
    s.text_styles.insert(egui::TextStyle::Small, regular(11.0));
    s.spacing.item_spacing = vec2(6.0, 4.0);
    s.spacing.button_padding = vec2(8.0, 3.0);
    s.spacing.interact_size = vec2(40.0, 22.0);
    s.spacing.icon_width = 12.0;
    s.spacing.icon_width_inner = 6.0;
    s.spacing.indent = 14.0;
    let v = &mut s.visuals;
    v.override_text_color = Some(T1);
    v.extreme_bg_color = B0;
    v.faint_bg_color = B2;
    v.selection.bg_fill = accent_bg();
    v.selection.stroke = Stroke::new(1.0, accent());
    v.window_fill = B2;
    v.panel_fill = B1;
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, LINE);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(6);
    }
    v.widgets.inactive.bg_fill = B0;
    v.widgets.inactive.weak_bg_fill = B0;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, T1);
    v.widgets.hovered.bg_fill = B3;
    v.widgets.hovered.weak_bg_fill = B3;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent_border());
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.active.bg_fill = accent_bg();
    v.widgets.active.weak_bg_fill = accent_bg();
    v.widgets.active.bg_stroke = Stroke::new(1.0, accent());
    v.widgets.open.weak_bg_fill = B3;
    v.widgets.open.bg_fill = B3;
}

fn menu_cfg() -> egui::containers::menu::MenuConfig {
    egui::containers::menu::MenuConfig::new().style(|s: &mut egui::Style| {
        egui::containers::menu::menu_style(s);
        studio_style(s);
        s.spacing.button_padding = vec2(10.0, 4.0);
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

/// Section title of the side panel: « SCÈNE  4 formes ».
fn section_head(ui: &mut Ui, title: &str, info: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let p = ui.painter();
    p.text(r.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, title, semibold(10.5), T2);
    p.text(r.right_center() - vec2(12.0, 0.0), Align2::RIGHT_CENTER, info, regular(11.5), T3);
}

fn tool_btn(ui: &mut Ui, icon: Icon, on: bool, tip: &str) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(34.0, 32.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect(r, 7.0, accent_bg(), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(r, 7.0, B3);
    }
    icons::paint(p, Rect::from_center_size(r.center(), vec2(17.0, 17.0)), icon, if on { accent_text() } else { Color32::from_rgb(0xC3, 0xC8, 0xCF) });
    resp.on_hover_text(tip)
}

fn small_btn(ui: &mut Ui, icon: Icon, on: bool, tip: &str) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(26.0, 24.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect(r, 6.0, accent_bg(), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(r, 6.0, B3);
    }
    icons::paint(p, Rect::from_center_size(r.center(), vec2(14.0, 14.0)), icon, if on { accent_text() } else { T2 });
    resp.on_hover_text(tip)
}

fn text_btn(ui: &mut Ui, icon: Option<Icon>, label: &str, primary: bool) -> Response {
    let g = ui.painter().layout_no_wrap(label.to_string(), medium(12.5), Color32::WHITE);
    let w = g.size().x + 20.0 + if icon.is_some() { 20.0 } else { 0.0 };
    let (r, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
    let p = ui.painter();
    let (fill, fg) = match (primary, resp.hovered()) {
        (true, false) => (accent(), Color32::WHITE),
        (true, true) => (Color32::from_rgb(0x6A, 0xA0, 0xFF), Color32::WHITE),
        (false, false) => (B3, T1),
        (false, true) => (B4, Color32::WHITE),
    };
    p.rect_filled(r, 6.0, fill);
    let mut x = r.min.x + 10.0;
    if let Some(i) = icon {
        icons::paint(p, Rect::from_min_size(pos2(x, r.center().y - 7.5), vec2(15.0, 15.0)), i, fg);
        x += 20.0;
    }
    p.galley(pos2(x, r.center().y - g.size().y * 0.5), g, fg);
    resp
}

fn small_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(regular(11.5)).color(T3));
}

fn mode_color(m: CombineMode) -> Color32 {
    match m {
        CombineMode::Add => accent(),
        CombineMode::Carve => carve(),
        _ => Color32::from_rgb(0xC8, 0xCC, 0xD2),
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

/// A millimetre field with a French decimal comma.
fn mm_drag(v: &mut f32, decimals: usize) -> egui::DragValue<'_> {
    egui::DragValue::new(v)
        .speed(0.1)
        .suffix(" mm")
        .custom_formatter(move |x, _| format!("{:.*}", decimals, x).replace('.', ","))
        .custom_parser(|t| t.replace(',', ".").trim_end_matches(|c: char| c.is_alphabetic() || c == ' ').parse().ok())
}

/// « Label   [ value ] » row. Returns (changed, first change of the gesture).
fn spin(ui: &mut Ui, label: &str, v: &mut f32, speed: f64, range: std::ops::RangeInclusive<f32>, suffix: &str, decimals: usize) -> (bool, bool) {
    let mut out = (false, false);
    ui.horizontal(|ui| {
        let (lr, _) = ui.allocate_exact_size(vec2(104.0, 22.0), Sense::hover());
        ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, label, regular(12.0), T2);
        let resp = ui.add_sized(
            vec2(ui.available_width().min(150.0), 22.0),
            egui::DragValue::new(v)
                .speed(speed)
                .range(range)
                .suffix(suffix)
                .fixed_decimals(decimals)
                .custom_formatter(move |x, _| format!("{:.*}", decimals, x).replace('.', ","))
                .custom_parser(|t| t.replace(',', ".").trim_end_matches(|c: char| c.is_alphabetic() || c == ' ' || c == '°').parse().ok()),
        );
        let start = live_start(ui, &resp);
        out = (resp.changed(), start);
    });
    out
}

fn sub_head(ui: &mut Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title.to_uppercase()).font(semibold(10.0)).color(T3));
}

fn presets() -> [(ViewPreset, &'static str); 4] {
    [(ViewPreset::Perspective, "Perspective"), (ViewPreset::Front, "Face"), (ViewPreset::Right, "Droite"), (ViewPreset::Top, "Dessus")]
}

// ---------------------------------------------------------------- top bar

fn top_bar(ctx: &egui::Context, st: &mut AppState, rect: Rect) {
    dock(ctx, "studio-top", rect, B1, |ui| {
        ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, LINE));
        // the document, centred
        let name = st.doc_name.clone();
        let dirty = st.scene_version != st.saved_version;
        let g = ui.painter().layout_no_wrap(name, semibold(13.0), T1);
        let sub = ui.painter().layout_no_wrap(if dirty { "· modifié".to_string() } else { "· enregistré".to_string() }, regular(11.5), T3);
        let total = g.size().x + 6.0 + sub.size().x;
        let x0 = rect.center().x - total * 0.5;
        let gw = g.size().x;
        ui.painter().galley(pos2(x0, rect.center().y - g.size().y * 0.5), g, T1);
        ui.painter().galley(pos2(x0 + gw + 6.0, rect.center().y - sub.size().y * 0.5), sub, T3);

        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.add_space(9.0);
            let (r, _) = ui.allocate_exact_size(vec2(24.0, 26.0), Sense::hover());
            crate::theme::paint_logo(ui, Rect::from_center_size(r.center(), vec2(24.0, 24.0)));
            ui.add_space(4.0);
            egui::containers::menu::MenuBar::new().config(menu_cfg()).ui(ui, |ui| {
                menus(ui, st);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.add_space(8.0);
                if small_btn(ui, Icon::Settings, st.settings_open, "Réglages : interface, navigation, création").clicked() {
                    st.settings_open = !st.settings_open;
                }
                let r = small_btn(ui, Icon::Light, st.display_open, "Affichage : lumière, ombres, anticrénelage");
                ui::light_toggle(st, &r);
                ui.add_space(6.0);
                // the command palette field
                let (r, resp) = ui.allocate_exact_size(vec2(300.0, 26.0), Sense::click());
                let p = ui.painter();
                p.rect(r, 7.0, if resp.hovered() { B2 } else { B0 }, Stroke::new(1.0, if resp.hovered() { accent_border() } else { LINE }), StrokeKind::Inside);
                icons::paint(p, Rect::from_min_size(pos2(r.min.x + 9.0, r.center().y - 6.5), vec2(13.0, 13.0)), Icon::Frame, T3);
                p.text(pos2(r.min.x + 30.0, r.center().y), Align2::LEFT_CENTER, "Commande, forme, réglage…", regular(12.0), T3);
                let kr = Rect::from_min_size(pos2(r.max.x - 56.0, r.min.y + 4.0), vec2(50.0, 18.0));
                p.rect(kr, 4.0, B2, Stroke::new(1.0, LINE), StrokeKind::Inside);
                p.text(kr.center(), Align2::CENTER_CENTER, "Espace", medium(10.5), T2);
                if resp.on_hover_text("Palette de commandes (Espace)").clicked() {
                    open_palette(ui.ctx());
                }
            });
        });
    });
}

fn menus(ui: &mut Ui, st: &mut AppState) {
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
        if ui.add_enabled(has, egui::Button::new("Symétrie X de la sélection")).clicked() {
            if let Some(id) = st.selection {
                st.edit("Symétrie", id, |n| n.mirror.x = !n.mirror.x);
            }
        }
        ui.separator();
        if ui.add_enabled(has || !st.edge_sel.is_empty(), egui::Button::new("Désélectionner").shortcut_text("Échap")).clicked() {
            st.clear_edge();
            st.select(None);
        }
    });
    ui.menu_button("Créer", |ui| {
        for k in PRIMS {
            let key = ui::prim_key(k).unwrap_or("");
            if ui.add(egui::Button::new(k.label()).shortcut_text(key).selected(st.tool == Tool::Place(k))).clicked() {
                st.commit_placement();
                st.tool = Tool::Place(k);
            }
        }
        ui.separator();
        small_label(ui, "Mode des nouvelles formes");
        for m in MODES {
            if ui.radio(st.mode == m, m.label()).clicked() {
                st.mode = m;
            }
        }
        if ui.checkbox(&mut st.sym_x, "Symétrie X des nouvelles formes").changed() {
            st.view_changed();
        }
    });
    ui.menu_button("Vue", |ui| {
        let cur = st.camera.preset_of();
        for (p, l) in presets() {
            if ui.radio(cur == p, l).clicked() {
                st.camera.preset(p);
                st.view_changed();
            }
        }
        ui.separator();
        if ui.add(egui::Button::new("Cadrer la sélection").shortcut_text("F")).clicked() {
            st.frame_selection();
        }
        if ui.button("Cadrer toute la scène").clicked() {
            let (lo, hi) = st.scene.bounds();
            st.frame_smooth(lo, hi);
        }
        ui.separator();
        if ui.checkbox(&mut st.grid, "Grille").changed() {
            st.view_changed();
        }
        ui.checkbox(&mut st.snap, "Snap");
        ui.separator();
        if ui.button("Passer à l'interface Établi").clicked() {
            st.set_ui_mode(UiMode::Atelier);
        }
    });
    ui.menu_button("Aide", |ui| {
        small_label(ui, "Raccourcis");
        for (k, what) in [
            ("Espace", "palette de commandes"),
            ("Q · 1 · 2", "sélection · objet · arête"),
            ("R · C · S · A · T", "cube · cylindre · sphère · capsule · tore"),
            ("Tab · Entrée", "cotes au clavier pendant la création"),
            ("Ctrl pendant la création", "inverse + / −"),
            ("F", "cadrer la sélection"),
            ("Ctrl+Z / Ctrl+Y", "annuler / rétablir"),
            ("Ctrl+D · Suppr", "dupliquer · supprimer"),
            ("Échap", "désélectionner"),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(k).font(medium(12.0)).color(T1));
                ui.label(RichText::new(what).font(regular(12.0)).color(T2));
            });
        }
    });
}

// ------------------------------------------------------------- tool column

fn rail(ctx: &egui::Context, st: &mut AppState, rect: Rect) {
    dock(ctx, "studio-rail", rect, B1, |ui| {
        ui.painter().line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, LINE));
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.add_space(8.0);
        let group = |ui: &mut Ui, title: &str| {
            let (r, _) = ui.allocate_exact_size(vec2(RAIL_W, 18.0), Sense::hover());
            ui.painter().text(r.center() + vec2(0.0, 2.0), Align2::CENTER_CENTER, title, semibold(8.5), T3);
        };
        let centred = |ui: &mut Ui, f: &mut dyn FnMut(&mut Ui)| {
            ui.horizontal(|ui| {
                ui.add_space((RAIL_W - 34.0) * 0.5);
                f(ui);
            });
        };
        let sel = st.tool == Tool::Select;
        centred(ui, &mut |ui| {
            let r = tool_btn(ui, Icon::Select, sel && st.pick_mode == PickMode::Object, "Sélection d'objets (Q ou 1)");
            ui::key_hint(ui, &r, "Q");
            if r.clicked() {
                st.set_pick_mode(PickMode::Object);
            }
        });
        centred(ui, &mut |ui| {
            let r = tool_btn(ui, Icon::Fillet, sel && st.pick_mode == PickMode::Edge, "Sélection d'arêtes (2) : fillets, chanfreins");
            ui::key_hint(ui, &r, "2");
            if r.clicked() {
                st.set_pick_mode(PickMode::Edge);
            }
        });
        group(ui, "FORMES");
        for k in PRIMS {
            centred(ui, &mut |ui| {
                let key = ui::prim_key(k).unwrap_or("");
                let r = tool_btn(ui, ui::prim_icon(k), st.tool == Tool::Place(k), &format!("{} ({key}) : glisser, ou taper les cotes", k.label()));
                ui::key_hint(ui, &r, key);
                if r.clicked() {
                    st.commit_placement();
                    st.tool = Tool::Place(k);
                }
            });
        }
        group(ui, "MODE");
        let shown = st.effective_mode();
        for m in MODES {
            centred(ui, &mut |ui| {
                let (r, resp) = ui.allocate_exact_size(vec2(34.0, 30.0), Sense::click());
                let c = mode_color(m);
                let on = shown == m;
                let p = ui.painter();
                if on {
                    p.rect(r, 7.0, tint(c, 0.16), Stroke::new(1.0, tint(c, 0.7)), StrokeKind::Inside);
                } else if resp.hovered() {
                    p.rect_filled(r, 7.0, B3);
                }
                ui::paint_mode_glyph(p, r.center(), m, if on { c } else { T2 }, 12.0);
                if resp.on_hover_text(if m == CombineMode::Add { "Add : ajoute la forme" } else { "Carve : creuse (Ctrl inverse pendant la création)" }).clicked() {
                    st.set_mode(m);
                }
            });
        }
        group(ui, "ÉDITION");
        let has = st.selection.is_some();
        let mirrored = st.selection.and_then(|id| st.scene.node(id)).map(|n| n.mirror.x).unwrap_or(false);
        centred(ui, &mut |ui| {
            if tool_btn(ui, Icon::Mirror, mirrored, "Symétrie X de la sélection").clicked() && has {
                if let Some(id) = st.selection {
                    st.edit("Symétrie", id, |n| n.mirror.x = !n.mirror.x);
                }
            }
        });
        centred(ui, &mut |ui| {
            if tool_btn(ui, Icon::Duplicate, false, "Dupliquer (Ctrl+D)").clicked() && has {
                st.duplicate_selected();
            }
        });
        centred(ui, &mut |ui| {
            if tool_btn(ui, Icon::Folder, false, "Grouper la sélection").clicked() && has {
                st.group_selected();
            }
        });
    });
}

// ------------------------------------------------------------- side panel

fn side_panel(ctx: &egui::Context, st: &mut AppState, rect: Rect) {
    dock(ctx, "studio-side", rect, B1, |ui| {
        ui.painter().line_segment([rect.left_top(), rect.left_bottom()], Stroke::new(1.0, LINE));
        ui.spacing_mut().item_spacing.y = 0.0;
        let w = rect.width() - 1.0;
        let rows = st.scene.rows();
        let n = st.scene.shape_count();
        // scene
        ui.add_space(6.0);
        section_head(ui, "SCÈNE", &format!("{n} forme{}", if n > 1 { "s" } else { "" }));
        let list_h = (rows.len().max(1) as f32 * 27.0 + 4.0).min(rect.height() * 0.3);
        egui::ScrollArea::vertical().id_salt("studio-scene").max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            if rows.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(RichText::new("Scène vide · R, C, S, A ou T pour créer").font(regular(12.0)).color(T2));
                });
            }
            for (id, depth) in rows.iter() {
                scene_row(ui, st, *id, *depth, w);
            }
        });
        ui.add_space(6.0);
        let (sep, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
        ui.painter().rect_filled(sep, 0.0, LINE);
        // properties
        let props_h = rect.height() - list_h - 6.0 - 26.0 - 7.0 - HIST_H;
        egui::ScrollArea::vertical().id_salt("studio-props").max_height(props_h).min_scrolled_height(props_h).auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(w - 26.0);
                ui.spacing_mut().item_spacing = vec2(6.0, 5.0);
                properties(ui, st);
            });
        });
        // history
        let (sep, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
        ui.painter().rect_filled(sep, 0.0, LINE);
        ui.add_space(6.0);
        section_head(ui, "HISTORIQUE", &format!("{} opération{}", rows.len(), if rows.len() > 1 { "s" } else { "" }));
        egui::ScrollArea::vertical().id_salt("studio-hist").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for (i, (id, _)) in rows.iter().enumerate() {
                history_row(ui, st, *id, i, w);
            }
        });
    });
}

fn row_frame(ui: &mut Ui, w: f32, selected: bool, hovered: bool, r: Rect) {
    let _ = w;
    let p = ui.painter();
    let r = r.shrink2(vec2(8.0, 0.0));
    if selected {
        p.rect(r, 6.0, accent_bg(), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
    } else if hovered {
        p.rect_filled(r, 6.0, B2);
    }
}

/// Mode badge of a row: « + » blue, « − » orange.
fn mode_badge(ui: &Ui, at: egui::Pos2, m: CombineMode, dim: f32) {
    let c = mode_color(m);
    let r = Rect::from_center_size(at, vec2(16.0, 16.0));
    ui.painter().rect_filled(r, 4.0, tint(c, 0.16 * dim));
    ui::paint_mode_glyph(ui.painter(), r.center(), m, tint(c, dim), 9.0);
}

fn scene_row(ui: &mut Ui, st: &mut AppState, id: NodeId, depth: usize, w: f32) {
    let Some(n) = st.scene.node(id).cloned() else { return };
    let (r, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
    let selected = st.is_selected(id);
    let dim = if n.visible { 1.0 } else { 0.45 };
    row_frame(ui, w, selected, resp.hovered(), r);
    let x0 = r.min.x + 16.0 + depth.saturating_sub(1) as f32 * 14.0;
    let mut toggled = false;
    if n.is_group() {
        let cr = Rect::from_min_size(pos2(x0, r.center().y - 5.0), vec2(10.0, 10.0));
        icons::paint(ui.painter(), cr, if n.expanded { Icon::Chevron } else { Icon::ChevronRight }, T2);
        toggled = ui.interact(cr.expand(5.0), Id::new(("s-exp", id)), Sense::click()).clicked();
        icons::paint(ui.painter(), Rect::from_min_size(pos2(x0 + 14.0, r.center().y - 7.0), vec2(14.0, 14.0)), Icon::Folder, tint(T2, dim));
    } else {
        mode_badge(ui, pos2(x0 + 8.0, r.center().y), n.combine.mode, dim);
    }
    let fg = if selected { accent_text() } else { tint(T1, dim) };
    let mut info = n.type_label();
    if n.mirror.any() {
        info += " · sym.";
    }
    let ig = ui.painter().layout_no_wrap(info, regular(11.5), if selected { accent_text() } else { tint(T3, dim.max(0.7)) });
    let ix = r.max.x - 36.0 - ig.size().x;
    let name_max = (ix - 8.0 - (x0 + 26.0)).max(20.0);
    let mut job = egui::text::LayoutJob::single_section(n.name.clone(), egui::TextFormat::simple(if selected { medium(12.5) } else { regular(12.5) }, fg));
    job.wrap = egui::text::TextWrapping { max_width: name_max, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = ui.painter().layout_job(job);
    let p = ui.painter();
    p.galley(pos2(x0 + 26.0, r.center().y - g.size().y * 0.5), g, fg);
    p.galley(pos2(ix, r.center().y - ig.size().y * 0.5), ig, T3);
    let er = Rect::from_center_size(pos2(r.max.x - 22.0, r.center().y), vec2(13.0, 13.0));
    icons::paint(p, er, if n.visible { Icon::Eye } else { Icon::EyeOff }, tint(T2, 0.8));
    let eye = ui.interact(er.expand(4.0), Id::new(("s-eye", id)), Sense::click());
    if eye.clicked() {
        st.toggle_visible(id);
    } else if toggled {
        if let Some(nm) = st.scene.node_mut(id) {
            nm.expanded = !nm.expanded;
        }
    } else if resp.double_clicked() {
        st.frame_selection();
    } else if resp.clicked() {
        st.commit_placement();
        st.tool = Tool::Select;
        if st.multi() || ui.input(|i| i.modifiers.shift) {
            st.toggle_in_selection(id);
        } else {
            st.select(Some(id));
        }
    }
}

/// The operations in evaluation order, numbered: a click selects one.
fn history_row(ui: &mut Ui, st: &mut AppState, id: NodeId, i: usize, w: f32) {
    let Some(n) = st.scene.node(id).cloned() else { return };
    let (r, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
    let selected = st.is_selected(id);
    row_frame(ui, w, selected, resp.hovered(), r);
    let p = ui.painter();
    p.text(pos2(r.min.x + 18.0, r.center().y), Align2::LEFT_CENTER, format!("{}", i + 1), medium(11.0), T3);
    if n.shape().is_some() {
        mode_badge(ui, pos2(r.min.x + 42.0, r.center().y), n.combine.mode, 1.0);
    }
    let p = ui.painter();
    let what = match n.shape() {
        Some(sh) => format!("{} · {}", sh.kind.label(), n.combine.mode.label()),
        None => n.type_label(),
    };
    p.text(pos2(r.min.x + 58.0, r.center().y), Align2::LEFT_CENTER, &n.name, regular(12.0), if selected { accent_text() } else { T1 });
    p.text(pos2(r.max.x - 16.0, r.center().y), Align2::RIGHT_CENTER, what, regular(11.0), T3);
    if resp.on_hover_text("Sélectionner cette opération").clicked() {
        st.commit_placement();
        st.tool = Tool::Select;
        st.select(Some(id));
    }
}

fn properties(ui: &mut Ui, st: &mut AppState) {
    let Some(id) = st.selection else {
        ui.label(RichText::new("Aucune sélection").font(semibold(13.0)).color(T1));
        ui.label(RichText::new("Clique une forme dans la vue ou dans la scène. Pour créer : une forme à gauche, puis glisse dans la vue ou tape ses cotes.").font(regular(12.0)).color(T2));
        sub_head(ui, "Nouvelles formes");
        ui.horizontal(|ui| {
            for m in MODES {
                if ui.radio(st.mode == m, m.label()).clicked() {
                    st.mode = m;
                }
            }
        });
        ui.checkbox(&mut st.sym_x, "Symétrie X (doubler les nouvelles formes)");
        sub_head(ui, "Matière par défaut");
        let cur = st.material;
        materials(ui, st, None, cur);
        return;
    };
    let Some(node) = st.scene.node(id).cloned() else { return };

    // name, type, mode, actions
    ui.horizontal(|ui| {
        let mut name = node.name.clone();
        let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(150.0).font(semibold(13.5)));
        if r.changed() {
            st.edit("Renommer", id, |n| n.name = name.clone());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            if small_btn(ui, Icon::Trash, false, "Supprimer (Suppr)").clicked() {
                st.delete_selected();
            }
            if small_btn(ui, Icon::Frame, false, "Cadrer (F)").clicked() {
                st.frame_selection();
            }
            if small_btn(ui, if node.visible { Icon::Eye } else { Icon::EyeOff }, false, "Masquer / afficher").clicked() {
                st.toggle_visible(id);
            }
            if small_btn(ui, Icon::Duplicate, false, "Dupliquer (Ctrl+D)").clicked() {
                st.duplicate_selected();
            }
        });
    });
    if st.selection.is_none() {
        return;
    }
    let what = match node.shape() {
        Some(sh) => format!("{} · {}", sh.kind.label(), node.combine.mode.label()),
        None => node.type_label(),
    };
    ui.label(RichText::new(what).font(regular(11.5)).color(T3));

    if let Some(sh) = node.shape().cloned() {
        sub_head(ui, "Forme");
        shape_params(ui, st, id, &sh);
    }
    if node.shape().is_some() || matches!(node.kind, NodeKind::Group { combined: true, .. }) {
        sub_head(ui, "Combinaison");
        let items = MODES.map(|m| (m, m.label(), mode_color(m)));
        if let Some(m) = ui::segmented(ui, &items, node.combine.mode, 28.0) {
            st.mode = m;
            st.edit("Mode", id, |n| n.combine.mode = m);
        }
        let mut b = node.combine.blend;
        let (ch, start) = spin(ui, "Raccord", &mut b, 0.05, 0.0..=12.0, " mm", 2);
        if ch {
            st.edit_live("Raccord", id, start, |n| n.combine.blend = b);
        }
        if node.combine.blend > 0.0 {
            ui.horizontal(|ui| {
                if ui.radio(node.combine.kind == BlendKind::Smooth, "Lisse").clicked() {
                    st.edit("Type de raccord", id, |n| n.combine.kind = BlendKind::Smooth);
                }
                if ui.radio(node.combine.kind == BlendKind::Chamfer, "Chanfrein").clicked() {
                    st.edit("Type de raccord", id, |n| n.combine.kind = BlendKind::Chamfer);
                }
            });
        }
    }
    if !st.edge_sel.is_empty() || node.shape().is_some() {
        sub_head(ui, "Arêtes");
        edges(ui, st);
    }
    sub_head(ui, "Position");
    position(ui, st, id, &node);
    sub_head(ui, "Symétrie");
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
    sub_head(ui, "Matière");
    materials(ui, st, Some(id), node.material);
}

/// X Y Z and rX rY rZ fields, three per row, coloured by axis.
fn position(ui: &mut Ui, st: &mut AppState, id: NodeId, node: &Node) {
    let mut pos = node.transform.pos;
    let (ex, ey, ez) = node.transform.rot.to_euler(EulerRot::XYZ);
    let mut rot = Vec3::new(ex.to_degrees(), ey.to_degrees(), ez.to_degrees());
    let fw = ((ui.available_width() - 3.0 * 20.0 - 5.0 * 4.0) / 3.0).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, (l, c)) in [("X", AXIS_X), ("Y", AXIS_Y), ("Z", AXIS_Z)].iter().enumerate() {
            ui.add_sized(vec2(16.0, 22.0), egui::Label::new(RichText::new(*l).font(semibold(11.5)).color(*c)));
            let resp = ui.add_sized(vec2(fw, 22.0), mm_drag(&mut pos[i], 1).suffix(""));
            let start = live_start(ui, &resp);
            if resp.changed() {
                st.edit_live("Position", id, start, |n| n.transform.pos = pos);
            }
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, (l, c)) in [("rX", AXIS_X), ("rY", AXIS_Y), ("rZ", AXIS_Z)].iter().enumerate() {
            ui.add_sized(vec2(16.0, 22.0), egui::Label::new(RichText::new(*l).font(semibold(11.0)).color(*c)));
            let resp = ui.add_sized(
                vec2(fw, 22.0),
                egui::DragValue::new(&mut rot[i]).speed(0.5).range(-360.0..=360.0).suffix("°").custom_formatter(|x, _| format!("{:.0}", x.round() + 0.0)),
            );
            let start = live_start(ui, &resp);
            if resp.changed() {
                let q = Quat::from_euler(EulerRot::XYZ, rot.x.to_radians(), rot.y.to_radians(), rot.z.to_radians());
                st.edit_live("Rotation", id, start, |n| n.transform.rot = q);
            }
        }
    });
}

fn materials(ui: &mut Ui, st: &mut AppState, target: Option<NodeId>, current: u32) {
    let mats = st.scene.materials.clone();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (i, m) in mats.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
            let on = current as usize == i;
            let c = Color32::from_rgb((m.color[0] * 255.0) as u8, (m.color[1] * 255.0) as u8, (m.color[2] * 255.0) as u8);
            let p = ui.painter();
            p.rect(r, 6.0, if on { accent_bg() } else { B0 }, Stroke::new(1.0, if on { accent() } else { LINE }), StrokeKind::Inside);
            p.rect(r.shrink(6.0), 3.0, c, Stroke::new(1.0, tint(Color32::WHITE, 0.15)), StrokeKind::Inside);
            if resp.on_hover_text(&m.name).clicked() {
                match target {
                    Some(id) => st.edit("Matière", id, |n| n.material = i as u32),
                    None => st.material = i as u32,
                }
            }
        }
    });
}

/// Dimensions as the user reads them (full width / height, radius).
fn shape_params(ui: &mut Ui, st: &mut AppState, id: NodeId, sh: &Shape) {
    let mut s = sh.clone();
    let mut any = false;
    let mut start = false;
    {
        let mut row = |ui: &mut Ui, label: &str, v: &mut f32, scale: f32, max: f32| {
            let mut shown = *v * scale;
            let (c, s0) = spin(ui, label, &mut shown, 0.1, 0.1..=max, " mm", 1);
            if c {
                *v = shown / scale;
                any = true;
                start |= s0;
            }
        };
        match sh.kind {
            PrimKind::Box => {
                row(ui, "Largeur", &mut s.size.x, 2.0, 400.0);
                row(ui, "Hauteur", &mut s.size.y, 2.0, 400.0);
                row(ui, "Profondeur", &mut s.size.z, 2.0, 400.0);
            }
            PrimKind::Cylinder | PrimKind::Prism => {
                row(ui, "Rayon", &mut s.size.x, 1.0, 200.0);
                row(ui, "Hauteur", &mut s.size.y, 2.0, 400.0);
            }
            PrimKind::Sphere => {
                row(ui, "Rayon X", &mut s.size.x, 1.0, 200.0);
                row(ui, "Rayon Y", &mut s.size.y, 1.0, 200.0);
                row(ui, "Rayon Z", &mut s.size.z, 1.0, 200.0);
            }
            PrimKind::Capsule => {
                row(ui, "Rayon", &mut s.size.x, 1.0, 200.0);
                row(ui, "Longueur", &mut s.size.y, 2.0, 400.0);
            }
            PrimKind::Torus => {
                row(ui, "Rayon", &mut s.size.x, 1.0, 200.0);
                row(ui, "Épaisseur", &mut s.size.z, 1.0, 80.0);
            }
            PrimKind::Cone => {
                row(ui, "Rayon bas", &mut s.size.x, 1.0, 200.0);
                row(ui, "Rayon haut", &mut s.size.z, 1.0, 200.0);
                row(ui, "Hauteur", &mut s.size.y, 2.0, 400.0);
            }
            PrimKind::Tube => {
                row(ui, "Rayon ext.", &mut s.size.x, 1.0, 200.0);
                row(ui, "Rayon int.", &mut s.size.z, 1.0, 200.0);
                row(ui, "Hauteur", &mut s.size.y, 2.0, 400.0);
            }
            PrimKind::Profile => {
                row(ui, "Épaisseur", &mut s.size.z, 2.0, 200.0);
            }
        }
    }
    if matches!(sh.kind, PrimKind::Box | PrimKind::Cylinder) {
        let max_round = match sh.kind {
            PrimKind::Box => s.size.min_element(),
            _ => s.size.x.min(s.size.y.max(0.1)),
        };
        let (r0, c0) = (s.round, s.chamfer);
        let (c, s0) = spin(ui, "Arrondi", &mut s.round, 0.05, 0.0..=max_round.max(0.1), " mm", 2);
        any |= c;
        start |= s0;
        let (c, s0) = spin(ui, "Chanfrein", &mut s.chamfer, 0.05, 0.0..=max_round.max(0.1), " mm", 2);
        any |= c;
        start |= s0;
        if s.round != r0 || s.chamfer != c0 {
            s.edges.clear();
        }
        if !sh.edges.is_empty() {
            ui.label(RichText::new("Arêtes réglées une par une : Arrondi / Chanfrein les remettent pareilles.").font(regular(11.5)).color(EDGE_SEL));
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

fn edges(ui: &mut Ui, st: &mut AppState) {
    let Some(e) = st.edge_sel.last().cloned() else {
        ui.label(RichText::new("Mode Arête (2) : survole une arête puis clique, Ctrl + clic pour en ajouter.").font(regular(12.0)).color(T2));
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
    let (ch, start) = spin(ui, if e.chamfer { "Chanfrein" } else { "Rayon" }, &mut r, 0.05, 0.0..=max, " mm", 2);
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

// ------------------------------------------------------------- status bar

fn status_bar(ctx: &egui::Context, st: &mut AppState, rect: Rect) {
    dock(ctx, "studio-status", rect, B1, |ui| {
        ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, LINE));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add_space(10.0);
            let key = |ui: &mut Ui, k: &str, what: &str| {
                let g = ui.painter().layout_no_wrap(k.to_string(), medium(10.5), T1);
                let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 10.0, 18.0), Sense::hover());
                ui.painter().rect(r, 4.0, B2, Stroke::new(1.0, LINE), StrokeKind::Inside);
                ui.painter().galley(r.center() - g.size() * 0.5, g, T1);
                ui.label(RichText::new(what).font(regular(11.5)).color(T2));
                ui.add_space(8.0);
            };
            match st.settings.navigation {
                Navigation::Maya => key(ui, "Alt", "+ gauche orbite · + molette pan · + droit zoom"),
                Navigation::Tripo => key(ui, "Glisser", "gauche orbite · droit pan · molette zoom"),
            }
            key(ui, "Clic droit", "marking menu");
            key(ui, "Espace", "commandes");
            key(ui, "F", "cadrer");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                ui.label(RichText::new(crate::version_title()).font(regular(11.0)).color(T3));
                ui.add_space(12.0);
                let hint = st.hint();
                let mut job = egui::text::LayoutJob::single_section(hint, egui::TextFormat::simple(regular(11.5), T2));
                job.wrap = egui::text::TextWrapping { max_width: (ui.available_width() - 20.0).max(60.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let g = ui.painter().layout_job(job);
                let (r, _) = ui.allocate_exact_size(vec2(g.size().x, 18.0), Sense::hover());
                ui.painter().galley(pos2(r.min.x, r.center().y - g.size().y * 0.5), g, T2);
            });
        });
    });
}

// --------------------------------------------------------- viewport chips

/// Over the 3D view: what a click picks and the helpers (top left), the view
/// and the display (top right).
fn view_chips(ctx: &egui::Context, st: &mut AppState, view: Rect) {
    egui::Area::new(Id::new("studio-chips-l")).order(Order::Foreground).fixed_pos(view.min + vec2(10.0, 10.0)).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let cur = if st.tool == Tool::Select { Some(st.pick_mode) } else { None };
            let items = [(Some(PickMode::Object), "Objet", "1"), (Some(PickMode::Edge), "Arête", "2")];
            if let Some(Some(m)) = ui::seg_inline(ui, "studio-pick", &items, cur) {
                st.set_pick_mode(m);
            }
            if ui::chip(ui, Some(Icon::Magnet), "Snap", st.snap, "Snap : grille, sommets et arêtes").clicked() {
                st.snap = !st.snap;
            }
            if ui::chip(ui, Some(Icon::Mirror), "Sym. X", st.sym_x, "Symétrie X des nouvelles formes").clicked() {
                st.sym_x = !st.sym_x;
            }
        });
    });
    egui::Area::new(Id::new("studio-chips-r")).order(Order::Foreground).pivot(Align2::RIGHT_TOP).fixed_pos(pos2(view.max.x - 10.0, view.min.y + 10.0)).show(ctx, |ui| {
        studio_style(ui.style_mut());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let cur = st.camera.preset_of();
            let name = presets().iter().find(|(p, _)| *p == cur).map(|(_, l)| *l).unwrap_or("Perspective");
            ui.menu_button(RichText::new(name).font(medium(12.0)).color(T1), |ui| {
                for (p, l) in presets() {
                    if ui.radio(cur == p, l).clicked() {
                        st.camera.preset(p);
                        st.view_changed();
                    }
                }
                ui.separator();
                if ui.button("Cadrer tout").clicked() {
                    let (lo, hi) = st.scene.bounds();
                    st.frame_smooth(lo, hi);
                }
            });
            if ui::chip(ui, Some(Icon::Grid), "Grille", st.grid, "Grille au sol").clicked() {
                st.grid = !st.grid;
                st.view_changed();
            }
        });
    });
}

// ------------------------------------------------------------ sizes panel

/// Studio: while a creation tool is armed, its dimensions can be typed.
/// Tab goes to the next field, Entrée creates the shape at the centre of the
/// view (on the face or the grid there), Échap leaves the tool.
fn sizes_panel(ctx: &egui::Context, st: &mut AppState, view: Rect) {
    let Tool::Place(kind) = st.tool else { return };
    if st.placement.is_some() {
        return;
    }
    let key = Id::new(("studio-sizes", kind.label()));
    let mut d: [f32; 3] = ctx.data(|m| m.get_temp(key)).unwrap_or(match kind {
        PrimKind::Box => [20.0, 20.0, 10.0],
        PrimKind::Cylinder => [8.0, 16.0, 0.0],
        PrimKind::Sphere => [8.0, 0.0, 0.0],
        PrimKind::Capsule => [5.0, 20.0, 0.0],
        PrimKind::Torus => [10.0, 3.0, 0.0],
        _ => [10.0, 10.0, 10.0],
    });
    let fields: &[&str] = match kind {
        PrimKind::Box => &["Largeur", "Profondeur", "Hauteur"],
        PrimKind::Cylinder => &["Rayon", "Hauteur"],
        PrimKind::Sphere => &["Rayon"],
        PrimKind::Capsule => &["Rayon", "Longueur"],
        PrimKind::Torus => &["Rayon", "Épaisseur"],
        _ => &[],
    };
    let mut create = false;
    egui::Area::new(Id::new("studio-sizes")).order(Order::Foreground).fixed_pos(view.min + vec2(10.0, 50.0)).show(ctx, |ui| {
        studio_style(ui.style_mut());
        ui::island().inner_margin(egui::Margin::same(12)).show(ui, |ui| {
            ui.set_width(232.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.horizontal(|ui| {
                icons::paint(ui.painter(), Rect::from_min_size(ui.cursor().min + vec2(0.0, 2.0), vec2(15.0, 15.0)), ui::prim_icon(kind), T1);
                ui.add_space(20.0);
                ui.label(RichText::new(format!("Nouveau {}", kind.label().to_lowercase())).font(semibold(13.0)).color(T1));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let m = st.effective_mode();
                    let c = mode_color(m);
                    let (r, resp) = ui.allocate_exact_size(vec2(26.0, 22.0), Sense::click());
                    ui.painter().rect(r, 6.0, tint(c, 0.16), Stroke::new(1.0, tint(c, 0.7)), StrokeKind::Inside);
                    ui::paint_mode_glyph(ui.painter(), r.center(), m, c, 10.0);
                    if resp.on_hover_text("Add ⇄ Carve").clicked() {
                        st.set_mode(if m == CombineMode::Add { CombineMode::Carve } else { CombineMode::Add });
                    }
                });
            });
            for (i, label) in fields.iter().enumerate() {
                ui.horizontal(|ui| {
                    let (lr, _) = ui.allocate_exact_size(vec2(76.0, 24.0), Sense::hover());
                    ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, *label, regular(12.0), T2);
                    // a text field: typing replaces the value (all selected on focus)
                    let fid = key.with(("field", i));
                    let mut text: String = ctx.data(|m| m.get_temp(fid)).unwrap_or_else(|| format!("{}", d[i]).replace('.', ","));
                    let resp = ui.add(egui::TextEdit::singleline(&mut text).id(fid).desired_width(ui.available_width() - 30.0).horizontal_align(egui::Align::RIGHT).font(medium(12.5)));
                    ui.label(RichText::new("mm").font(regular(11.5)).color(T3));
                    let first = i == 0 && ctx.data(|m| m.get_temp::<bool>(key.with("focused"))).is_none();
                    if first {
                        // first time the panel shows for this tool: the first field takes the keys
                        resp.request_focus();
                        ctx.data_mut(|m| m.insert_temp(key.with("focused"), true));
                    }
                    if first || resp.gained_focus() {
                        if let Some(mut s) = egui::TextEdit::load_state(ctx, fid) {
                            let n = text.chars().count();
                            s.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(n))));
                            s.store(ctx, fid);
                        }
                    }
                    if let Ok(v) = text.trim().replace(',', ".").parse::<f32>() {
                        d[i] = v.clamp(0.5, 400.0);
                    }
                    ctx.data_mut(|m| m.insert_temp(fid, text));
                });
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Tab · Entrée crée · Échap").font(regular(11.0)).color(T3));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if text_btn(ui, None, "Créer", true).clicked() {
                        create = true;
                    }
                });
            });
        });
    });
    ctx.data_mut(|m| m.insert_temp(key, d));
    if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
        create = true;
    }
    if create {
        let at = st.principal();
        st.place_sized(at, kind, d);
        ctx.data_mut(|m| {
            m.remove::<bool>(key.with("focused"));
            for i in 0..3 {
                m.remove::<String>(key.with(("field", i)));
            }
        });
        ctx.memory_mut(|m| m.request_focus(Id::NULL));
    }
}

// --------------------------------------------------------- command palette

#[derive(Clone, Copy)]
enum Cmd {
    Place(PrimKind),
    Mode(CombineMode),
    Pick(PickMode),
    Duplicate,
    Delete,
    Group,
    MirrorSel,
    Frame,
    FrameAll,
    View(ViewPreset),
    Snap,
    SymX,
    Grid,
    Undo,
    Redo,
    Save,
    Open,
    Export,
    Settings,
    Display,
    Etabli,
}

fn commands() -> Vec<(String, &'static str, Cmd)> {
    let mut v: Vec<(String, &'static str, Cmd)> = vec![];
    for k in PRIMS {
        v.push((format!("Créer : {}", k.label()), ui::prim_key(k).unwrap_or(""), Cmd::Place(k)));
    }
    v.extend([
        ("Mode Add (nouvelles formes)".to_string(), "", Cmd::Mode(CombineMode::Add)),
        ("Mode Carve (nouvelles formes)".to_string(), "", Cmd::Mode(CombineMode::Carve)),
        ("Sélection d'objets".to_string(), "Q", Cmd::Pick(PickMode::Object)),
        ("Sélection d'arêtes (fillets)".to_string(), "2", Cmd::Pick(PickMode::Edge)),
        ("Dupliquer".to_string(), "Ctrl+D", Cmd::Duplicate),
        ("Supprimer".to_string(), "Suppr", Cmd::Delete),
        ("Grouper la sélection".to_string(), "", Cmd::Group),
        ("Symétrie X de la sélection".to_string(), "", Cmd::MirrorSel),
        ("Cadrer la sélection".to_string(), "F", Cmd::Frame),
        ("Cadrer toute la scène".to_string(), "", Cmd::FrameAll),
        ("Vue perspective".to_string(), "", Cmd::View(ViewPreset::Perspective)),
        ("Vue de face".to_string(), "", Cmd::View(ViewPreset::Front)),
        ("Vue de droite".to_string(), "", Cmd::View(ViewPreset::Right)),
        ("Vue de dessus".to_string(), "", Cmd::View(ViewPreset::Top)),
        ("Snap (activer / couper)".to_string(), "", Cmd::Snap),
        ("Symétrie X des nouvelles formes".to_string(), "", Cmd::SymX),
        ("Grille (afficher / cacher)".to_string(), "", Cmd::Grid),
        ("Annuler".to_string(), "Ctrl+Z", Cmd::Undo),
        ("Rétablir".to_string(), "Ctrl+Y", Cmd::Redo),
        ("Enregistrer".to_string(), "Ctrl+S", Cmd::Save),
        ("Ouvrir · Nouveau…".to_string(), "", Cmd::Open),
        ("Exporter le mesh…".to_string(), "", Cmd::Export),
        ("Réglages…".to_string(), "", Cmd::Settings),
        ("Affichage : lumière, ombres…".to_string(), "", Cmd::Display),
        ("Passer à l'interface Établi".to_string(), "", Cmd::Etabli),
    ]);
    v
}

fn run(st: &mut AppState, c: Cmd) {
    match c {
        Cmd::Place(k) => {
            st.commit_placement();
            st.tool = Tool::Place(k);
        }
        Cmd::Mode(m) => st.mode = m,
        Cmd::Pick(m) => st.set_pick_mode(m),
        Cmd::Duplicate => st.duplicate_selected(),
        Cmd::Delete => st.delete_selected(),
        Cmd::Group => st.group_selected(),
        Cmd::MirrorSel => {
            if let Some(id) = st.selection {
                st.edit("Symétrie", id, |n| n.mirror.x = !n.mirror.x);
            }
        }
        Cmd::Frame => st.frame_selection(),
        Cmd::FrameAll => {
            let (lo, hi) = st.scene.bounds();
            st.frame_smooth(lo, hi);
        }
        Cmd::View(p) => {
            st.camera.preset(p);
            st.view_changed();
        }
        Cmd::Snap => st.snap = !st.snap,
        Cmd::SymX => st.sym_x = !st.sym_x,
        Cmd::Grid => {
            st.grid = !st.grid;
            st.view_changed();
        }
        Cmd::Undo => st.undo(),
        Cmd::Redo => st.redo(),
        Cmd::Save => st.save(),
        Cmd::Open => st.files_open = true,
        Cmd::Export => st.export_open = true,
        Cmd::Settings => st.settings_open = true,
        Cmd::Display => st.display_open = true,
        Cmd::Etabli => st.set_ui_mode(UiMode::Atelier),
    }
}

/// Lower case without accents: « créer » matches « creer ».
fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'à' | 'â' => 'a',
            'î' | 'ï' => 'i',
            'ô' => 'o',
            'ù' | 'û' => 'u',
            'ç' => 'c',
            _ => c,
        })
        .collect()
}

#[derive(Clone, Default)]
struct Palette {
    open: bool,
    query: String,
    sel: usize,
    fresh: bool,
}

fn open_palette(ctx: &egui::Context) {
    ctx.data_mut(|m| m.insert_temp(Id::new("studio-palette"), Palette { open: true, query: String::new(), sel: 0, fresh: true }));
}

fn palette(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let id = Id::new("studio-palette");
    let mut pal: Palette = ctx.data(|m| m.get_temp(id)).unwrap_or_default();
    if !pal.open {
        return;
    }
    if ui::dim(ctx, screen, "studio-palette-dim") {
        pal.open = false;
    }
    let all = commands();
    let q = fold(pal.query.trim());
    let words: Vec<&str> = q.split_whitespace().collect();
    let hits: Vec<&(String, &'static str, Cmd)> = all.iter().filter(|(l, _, _)| {
        let f = fold(l);
        words.iter().all(|w| f.contains(w))
    }).collect();
    let (up, down, enter, esc) = ctx.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
    if down && !hits.is_empty() {
        pal.sel = (pal.sel + 1).min(hits.len() - 1);
    }
    if up {
        pal.sel = pal.sel.saturating_sub(1);
    }
    pal.sel = pal.sel.min(hits.len().saturating_sub(1));
    let mut chosen: Option<Cmd> = None;
    egui::Area::new(Id::new("studio-palette-box")).order(Order::Tooltip).pivot(Align2::CENTER_TOP).fixed_pos(pos2(screen.center().x, screen.min.y + 90.0)).show(ctx, |ui| {
        studio_style(ui.style_mut());
        ui::island().inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.set_width(520.0);
            let r = ui.add(egui::TextEdit::singleline(&mut pal.query).hint_text("Tape une commande : cylindre, carve, vue de dessus, exporter…").font(regular(14.0)).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)));
            if pal.fresh {
                r.request_focus();
                pal.fresh = false;
            }
            if r.changed() {
                pal.sel = 0;
            }
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                if hits.is_empty() {
                    ui.label(RichText::new("Aucune commande ne correspond.").font(regular(12.5)).color(T2));
                }
                for (i, (label, key, cmd)) in hits.iter().enumerate() {
                    let (rr, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
                    let on = i == pal.sel;
                    let p = ui.painter();
                    if on {
                        p.rect(rr, 6.0, accent_bg(), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
                    } else if resp.hovered() {
                        p.rect_filled(rr, 6.0, B2);
                    }
                    p.text(rr.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, label, regular(12.5), if on { accent_text() } else { T1 });
                    if !key.is_empty() {
                        p.text(rr.right_center() - vec2(10.0, 0.0), Align2::RIGHT_CENTER, *key, medium(11.0), T3);
                    }
                    if on && (up || down) {
                        resp.scroll_to_me(None);
                    }
                    if resp.clicked() {
                        chosen = Some(*cmd);
                    }
                }
            });
        });
    });
    if enter {
        if let Some((_, _, c)) = hits.get(pal.sel) {
            chosen = Some(*c);
        }
    }
    if esc {
        pal.open = false;
    }
    if let Some(c) = chosen {
        pal.open = false;
        run(st, c);
    }
    ctx.data_mut(|m| m.insert_temp(id, pal));
}
