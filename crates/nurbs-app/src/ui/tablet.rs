//! The iPad's layout, from the mock-up of 21 nov. (Romain: "Parfait pour
//! la maquette, tu peux mettre à jour la version iPad avec la maquette
//! dédiée"). The same app and tools as the PC, laid out for the fingers and
//! the Pencil:
//!
//! - top left the document (its name, Open / New / Save, undo, redo);
//!   top centre the component modes and ADD / CARVE (FILLET / CHAMFER in
//!   Edge and Vertex modes; the SDF tab's operation and blend); top right
//!   the tabs (Forme, SDF), the view menu, Frame all and the settings;
//! - the scene list in a sidebar that folds, an element's operations
//!   under its row with their switches, its position and rotation folded at
//!   the bottom;
//! - the tools in a dock on the other side (Transform, Create, Modify, the
//!   material ball), its labels under the icons;
//! - an action bar over what is selected or picked, with labelled buttons;
//! - the history at the bottom, larger cells;
//! - the operation window with − / + steps and a keypad for the values;
//! - the Touch & Pencil sheet (the "?" button) with its settings.
//!
//! Left-handed (settings) swaps the sides: the dock on the left, the scene
//! list and Shift / Ctrl on the right.

use super::*;
use crate::touch::{BAR_EDGE, KEY_D, KEY_GAP};

/// The margin to the screen's edges, the top islands' height, where the
/// sidebar and the dock begin, their widths, the history's height, the
/// help button.
const E: f32 = 16.0;
const BAR_H: f32 = 62.0;
const T_TOP: f32 = E + BAR_H + 14.0;
const SIDE_W: f32 = 316.0;
const DOCK_W: f32 = 92.0;
const HELP_D: f32 = 60.0;

/// The mock-up's island: graphite, a thin border, rounder, a soft shadow.
fn island_t() -> egui::Frame {
    egui::Frame::new()
        .fill(tint(PANEL(), 0.96))
        .stroke(Stroke::new(1.0, PANEL_BORDER()))
        .corner_radius(egui::CornerRadius::same(18))
        .inner_margin(Margin::same(8))
        .shadow(egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(90) })
}

/// A pop-over (a menu, a fly-out, the gestures sheet): opaque.
pub(super) fn pop_t() -> egui::Frame {
    island_t().fill(PANEL().to_opaque())
}

/// A pop-over's flag (open or not), kept by egui between frames.
pub(super) fn flag(ctx: &egui::Context, name: &str) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::new(("t-flag", name)))).unwrap_or(false)
}

pub(super) fn set_flag(ctx: &egui::Context, name: &str, on: bool) {
    ctx.data_mut(|d| d.insert_temp(Id::new(("t-flag", name)), on));
}

/// A press outside these rects closes a pop-over.
pub(super) fn pressed_outside(ctx: &egui::Context, keep: &[Rect]) -> bool {
    ctx.input(|i| i.pointer.any_pressed()) && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| !keep.iter().any(|r| r.contains(p)))
}

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    let flip = st.settings.left_handed;
    let side = st.panel_visible;
    // the view, between the sidebar and the dock
    let (l, r) = if flip { (E + DOCK_W + E, if side { screen.width() - E - SIDE_W - E } else { screen.width() - E }) } else { (if side { E + SIDE_W + E } else { E }, screen.width() - E - DOCK_W - E) };
    st.view_rect = [screen.min.x + l, T_TOP, screen.min.x + r, screen.max.y - E];
    doc_island(ctx, st, screen);
    modes_island(ctx, st, screen);
    view_island(ctx, st, screen);
    let dock_x = if flip { screen.min.x + E } else { screen.max.x - E - DOCK_W };
    dock(ctx, st, screen, dock_x);
    // the sidebar on the side of Shift and Ctrl
    let side_x = if flip { screen.max.x - E - SIDE_W } else { screen.min.x + E };
    if side {
        sidebar(ctx, st, screen, side_x);
    } else {
        side_button(ctx, st, if flip { screen.max.x - E - 52.0 } else { screen.min.x + E });
    }
    hint_pill(ctx, st);
    history_strip(ctx, st, screen);
    action_bar(ctx, st);
    group_bar(ctx, st);
    align_window(ctx, st);
    op_window(ctx, st);
    materials_pop(ctx, st, dock_x, flip);
    curves_flyout(ctx, st, dock_x, flip);
    more_flyout(ctx, st, dock_x, flip);
    view_menu(ctx, st);
    settings_pop(ctx, st);
    help_button(ctx, st, screen, flip);
    gesture_sheet(ctx, st, screen);
    files_dialog(ctx, st, screen);
    toast(ctx, st, screen);
    keypad(ctx);
    shortcuts(ctx, st);
}

// ------------------------------------------------------------ the top

/// A button of a segmented row (the modes, ADD / CARVE, the tabs): its
/// icon over its label, or beside it (`row`); lit in `on_fill`.
fn seg_btn(ui: &mut Ui, size: egui::Vec2, icon: Option<Icon>, label: &str, on: bool, on_fill: Color32, row: bool) -> Response {
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(r, 11.0, on_fill);
    } else if resp.is_pointer_button_down_on() || resp.hovered() {
        p.rect_filled(r, 11.0, RAISED());
    }
    let fg = if on { contrast_on(on_fill) } else { pick(Color32::from_rgb(0xB8, 0xBE, 0xC6), Color32::from_rgb(0x4A, 0x50, 0x58)) };
    if row {
        let g = p.layout_no_wrap(label.to_string(), semibold(13.0), fg);
        let w = g.size().x + if icon.is_some() { 24.0 } else { 0.0 };
        let x0 = r.center().x - w * 0.5;
        if let Some(i) = icon {
            icons::paint(p, Rect::from_center_size(pos2(x0 + 8.0, r.center().y), vec2(16.0, 16.0)), i, fg);
        }
        p.galley(pos2(x0 + if icon.is_some() { 24.0 } else { 0.0 }, r.center().y - g.size().y * 0.5), g, fg);
    } else {
        if let Some(i) = icon {
            icons::paint(p, Rect::from_center_size(pos2(r.center().x, r.center().y - 8.0), vec2(18.0, 18.0)), i, fg);
        }
        p.text(pos2(r.center().x, r.center().y + 12.0), Align2::CENTER_CENTER, label, medium(11.0), fg);
    }
    resp
}

/// The text on a filled button: dark on a light fill, light on a dark one.
pub(super) fn contrast_on(fill: Color32) -> Color32 {
    let l = 0.299 * fill.r() as f32 + 0.587 * fill.g() as f32 + 0.114 * fill.b() as f32;
    if l > 150.0 { Color32::from_rgb(0x0F, 0x11, 0x14) } else { Color32::from_rgb(0xF2, 0xF3, 0xF5) }
}

/// The well round a segmented row.
fn well(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new().fill(WELL()).stroke(Stroke::new(1.0, DIVIDER())).corner_radius(egui::CornerRadius::same(14)).inner_margin(Margin::same(3)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            add(ui);
        });
    });
}

fn sep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(9.0, 40.0), Sense::hover());
    ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(1.0, 30.0)), 0.0, DIVIDER());
}

/// Top left: the document (a tap: Open, New, Save), undo, redo.
fn doc_island(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    // above the sidebar or the dock, whichever side it is on
    let at = pos2(screen.min.x + E, screen.min.y + E);
    egui::Area::new(Id::new("t-doc")).order(Order::Middle).fixed_pos(at).show(ctx, |ui| {
        island_t().show(ui, |ui| {
            ui.set_height(BAR_H - 16.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let n = st.doc.features.iter().filter(|f| f.is_solid()).count();
                let (r, resp) = ui.allocate_exact_size(vec2(196.0, 46.0), Sense::click());
                let p = ui.painter();
                if resp.is_pointer_button_down_on() || st.files_open {
                    p.rect_filled(r, 12.0, RAISED());
                }
                let lr = Rect::from_center_size(pos2(r.min.x + 22.0, r.center().y), vec2(34.0, 34.0));
                paint_logo(ui, lr);
                let p = ui.painter();
                let mut job = egui::text::LayoutJob::single_section(st.doc_name.clone(), egui::TextFormat::simple(semibold(16.0), TEXT_STRONG()));
                job.wrap = egui::text::TextWrapping { max_width: r.max.x - 28.0 - (lr.max.x + 10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let g = p.layout_job(job);
                p.galley(pos2(lr.max.x + 10.0, r.center().y - 2.0 - g.size().y), g, TEXT_STRONG());
                p.text(pos2(lr.max.x + 10.0, r.center().y + 1.0), Align2::LEFT_TOP, format!("{n} shape{}", if n == 1 { "" } else { "s" }), regular(13.0), TEXT2());
                icons::paint(p, Rect::from_center_size(pos2(r.max.x - 12.0, r.center().y), vec2(14.0, 14.0)), Icon::Chevron, TEXT2());
                if resp.clicked() {
                    st.files_open = !st.files_open;
                }
                sep(ui);
                if icon_btn(ui, Icon::Undo, false, "Undo (two-finger tap)", 44.0).clicked() {
                    st.undo();
                }
                if icon_btn(ui, Icon::Redo, false, "Redo (three-finger tap)", 44.0).clicked() {
                    st.redo();
                }
            });
        });
    });
}

/// Top centre: the component modes, ADD / CARVE (FILLET / CHAMFER in Edge
/// and Vertex modes), Snap. The SDF tab: Add / Carve / Intersect, the blend.
fn modes_island(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let cx = screen.center().x;
    egui::Area::new(Id::new("t-modes")).order(Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(pos2(cx, screen.min.y + E)).show(ctx, |ui| {
        island_t().show(ui, |ui| {
            ui.set_height(BAR_H - 16.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                crate::tab_anim::enter(ui, vec2(0.0, -6.0), |ui| match st.workspace {
                    Workspace::Sdf => sdf_modes(ui, st),
                    _ => forme_modes(ui, st),
                });
                sep(ui);
                let snap = st.settings.snap;
                let (r, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
                ui.painter().rect_filled(r, 12.0, if snap { RAISED() } else { Color32::TRANSPARENT });
                icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(20.0, 20.0)), Icon::Magnet, if snap { TEXT_STRONG() } else { TEXT2() });
                if resp.clicked() {
                    st.settings.snap = !snap;
                    st.save_settings();
                    st.toast(if snap { "Snap off: free moves" } else { "Snap on: the grid and the part's points" }, false);
                }
            });
        });
    });
}

fn forme_modes(ui: &mut Ui, st: &mut AppState) {
    well(ui, |ui| {
        for (i, m) in SelMode::ALL.into_iter().enumerate() {
            let icon = [Icon::ModeVertex, Icon::ModeEdge, Icon::ModeFace, Icon::ModeObject][i];
            let on = st.sel_mode == m;
            if seg_btn(ui, vec2(64.0, 46.0), Some(icon), m.label(), on, accent(), false).clicked() {
                st.set_sel_mode(m);
            }
        }
    });
    sep(ui);
    if st.tool == Tool::Select && matches!(st.sel_mode, SelMode::Edge | SelMode::Vertex) {
        // what a drag on an edge or a sharp corner makes
        well(ui, |ui| {
            for (c, name) in [(false, "FILLET"), (true, "CHAMFER")] {
                if seg_btn(ui, vec2(94.0, 46.0), Some(Icon::Fillet), name, st.round_chamfer == c, accent(), true).clicked() {
                    st.round_chamfer = c;
                }
            }
        });
    } else {
        let shown = shown_op(st);
        well(ui, |ui| {
            for op in [Op::Add, Op::Carve] {
                let on = shown == op;
                let label = if op == Op::Add { "ADD" } else { "CARVE" };
                let (r, resp) = ui.allocate_exact_size(vec2(86.0, 46.0), Sense::click());
                let p = ui.painter();
                let fill = if op == Op::Add { accent() } else { carve() };
                if on {
                    p.rect_filled(r, 11.0, fill);
                } else if resp.is_pointer_button_down_on() {
                    p.rect_filled(r, 11.0, RAISED());
                }
                let fg = if on { contrast_on(fill) } else { TEXT2() };
                paint_op_glyph(p, pos2(r.min.x + 22.0, r.center().y), op, fg, 12.0);
                p.text(pos2(r.min.x + 36.0, r.center().y), Align2::LEFT_CENTER, label, semibold(13.0), fg);
                if resp.clicked() {
                    set_op_from_ui(st, op);
                }
            }
        });
    }
}

fn sdf_modes(ui: &mut Ui, st: &mut AppState) {
    use forme_core::CombineMode;
    let picked = st.sdf_picked().and_then(|(f, n)| crate::sdf::scene_of(f).and_then(|s| s.node(n)).map(|nd| (nd.combine.mode, nd.combine.blend)));
    let (mode, blend) = picked.unwrap_or((st.sdf.mode, st.sdf.blend));
    well(ui, |ui| {
        for (m, icon, label) in [(CombineMode::Add, Icon::BoolUnion, "ADD"), (CombineMode::Carve, Icon::BoolSubtract, "CARVE"), (CombineMode::Intersect, Icon::BoolIntersect, "INTERSECT")] {
            if seg_btn(ui, vec2(if m == CombineMode::Intersect { 120.0 } else { 92.0 }, 46.0), Some(icon), label, mode == m, if m == CombineMode::Carve { carve() } else { accent() }, true).clicked() {
                st.sdf.mode = m;
                if picked.is_some() {
                    st.sdf_edit_node(&format!("Make it {}", m.label()), |n| n.combine.mode = m);
                }
            }
        }
    });
    sep(ui);
    // the blend: − value +
    ui.label(egui::RichText::new("BLEND").font(mono(11.0)).color(LABEL()));
    let mut b = blend;
    let mut changed = false;
    if icon_btn(ui, Icon::Minus, false, "Blend 0.5 mm less", 40.0).clicked() {
        b = (b - 0.5).max(0.0);
        changed = true;
    }
    if value_field(ui, &mut b, 0.0..=30.0, 0.1, "mm", 92.0).changed() {
        changed = true;
    }
    if icon_btn(ui, Icon::Plus, false, "Blend 0.5 mm more", 40.0).clicked() {
        b = (b + 0.5).min(30.0);
        changed = true;
    }
    if changed && (b - blend).abs() > 1e-4 {
        st.sdf.blend = b;
        if picked.is_some() {
            st.sdf_edit_node("Blend", |n| n.combine.blend = b);
        }
    }
}

/// Top right: the tabs (Forme, SDF), the view menu, Frame all, settings.
fn view_island(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let x = if st.settings.left_handed { screen.max.x - E } else { screen.max.x - E };
    egui::Area::new(Id::new("t-view")).order(Order::Middle).pivot(Align2::RIGHT_TOP).fixed_pos(pos2(x, screen.min.y + E)).show(ctx, |ui| {
        island_t().show(ui, |ui| {
            ui.set_height(BAR_H - 16.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                // the tabs: each in its own colour when it is the current one
                well(ui, |ui| {
                    for w in st.workspaces() {
                        if seg_btn(ui, vec2(70.0, 46.0), Some(w.icon()), w.label(), st.workspace == w, w.swatch(), false).clicked() && st.workspace != w {
                            st.set_workspace(w);
                        }
                    }
                });
                sep(ui);
                let name = match st.camera.preset_of() {
                    ViewPreset::Perspective => "3D",
                    ViewPreset::Front => "Front",
                    ViewPreset::Right => "Right",
                    ViewPreset::Top => "Top",
                };
                let (r, resp) = ui.allocate_exact_size(vec2(70.0, 44.0), Sense::click());
                let open = flag(ctx, "view");
                if open || resp.is_pointer_button_down_on() {
                    ui.painter().rect_filled(r, 12.0, RAISED());
                }
                ui.painter().text(pos2(r.min.x + 14.0, r.center().y), Align2::LEFT_CENTER, name, semibold(15.0), TEXT_STRONG());
                icons::paint(ui.painter(), Rect::from_center_size(pos2(r.max.x - 16.0, r.center().y), vec2(13.0, 13.0)), Icon::Chevron, TEXT2());
                ctx.data_mut(|d| d.insert_temp(Id::new("t-view-btn"), r));
                if resp.clicked() {
                    set_flag(ctx, "view", !open);
                    set_flag(ctx, "settings", false);
                }
                if icon_btn(ui, Icon::Frame, false, "Frame all", 44.0).clicked() {
                    st.frame_all();
                }
                let r = icon_btn(ui, Icon::Sliders, flag(ctx, "settings"), "Settings", 44.0);
                ctx.data_mut(|d| d.insert_temp(Id::new("t-settings-btn"), r.rect));
                if r.clicked() {
                    let open = flag(ctx, "settings");
                    set_flag(ctx, "settings", !open);
                    set_flag(ctx, "view", false);
                }
            });
        });
    });
}

/// A row of a pop-over menu: an icon, a label, a tick or a value on the
/// right; 48 points tall.
pub(super) fn menu_row(ui: &mut Ui, icon: Option<Icon>, label: &str, on: bool) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
    let p = ui.painter();
    if resp.is_pointer_button_down_on() {
        p.rect_filled(r, 10.0, RAISED());
    }
    let mut x = r.min.x + 12.0;
    if let Some(i) = icon {
        icons::paint(p, Rect::from_center_size(pos2(x + 10.0, r.center().y), vec2(20.0, 20.0)), i, TEXT());
        x += 32.0;
    }
    p.text(pos2(x, r.center().y), Align2::LEFT_CENTER, label, medium(15.0), TEXT());
    if on {
        let c = pos2(r.max.x - 22.0, r.center().y);
        p.line_segment([c + vec2(-6.0, 0.0), c + vec2(-2.0, 4.5)], Stroke::new(2.0, TEXT_STRONG()));
        p.line_segment([c + vec2(-2.0, 4.5), c + vec2(6.0, -5.0)], Stroke::new(2.0, TEXT_STRONG()));
    }
    resp
}

/// The view menu under "3D": the presets, X-ray, Isolate, the grid.
pub(super) fn view_menu(ctx: &egui::Context, st: &mut AppState) {
    if !flag(ctx, "view") {
        return;
    }
    let Some(btn) = ctx.data(|d| d.get_temp::<Rect>(Id::new("t-view-btn"))) else { return };
    let at = pos2(btn.max.x + 60.0, btn.max.y + 14.0);
    let area = egui::Area::new(Id::new("t-view-menu")).order(Order::Foreground).pivot(Align2::RIGHT_TOP).fixed_pos(at).show(ctx, |ui| {
        pop_t().inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.set_width(240.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let cur = st.camera.preset_of();
            for (p, name) in [(ViewPreset::Perspective, "3D"), (ViewPreset::Front, "Front"), (ViewPreset::Right, "Right"), (ViewPreset::Top, "Top")] {
                if menu_row(ui, None, name, cur == p).clicked() {
                    st.stop_flight();
                    st.camera.preset(p);
                    set_flag(ctx, "view", false);
                }
            }
            ui.add_space(4.0);
            divider(ui);
            ui.add_space(4.0);
            if menu_row(ui, Some(Icon::Xray), "X-ray", st.xray).clicked() {
                st.xray = !st.xray;
            }
            if menu_row(ui, Some(Icon::Isolate), "Isolate the selection", st.isolate.is_some()).clicked() {
                st.toggle_isolate();
            }
            if menu_row(ui, Some(Icon::Grid), "Ground grid", st.settings.grid).clicked() {
                st.settings.grid = !st.settings.grid;
                st.save_settings();
            }
        });
    });
    if pressed_outside(ctx, &[area.response.rect, btn]) {
        set_flag(ctx, "view", false);
    }
}

/// The settings pop-over: the look, the snapping, the iPad's own (the
/// hand, Pencil only, the hold time) and the Touch & Pencil sheet.
pub(super) fn settings_pop(ctx: &egui::Context, st: &mut AppState) {
    if !flag(ctx, "settings") {
        return;
    }
    let Some(btn) = ctx.data(|d| d.get_temp::<Rect>(Id::new("t-settings-btn"))) else { return };
    let at = pos2(btn.max.x + 8.0, btn.max.y + 14.0);
    let area = egui::Area::new(Id::new("t-settings")).order(Order::Foreground).pivot(Align2::RIGHT_TOP).fixed_pos(at).show(ctx, |ui| {
        pop_t().inner_margin(Margin::same(14)).show(ui, |ui| {
            ui.set_width(320.0);
            ui.label(egui::RichText::new("Settings").font(semibold(17.0)).color(TEXT_STRONG()));
            ui.add_space(4.0);
            let mut changed = false;
            for (label, v) in [
                ("Light theme", &mut st.settings.light as *mut bool),
                ("Studio interface", &mut st.settings.studio as *mut bool),
                ("Snap to the part's points", &mut st.settings.points as *mut bool),
                ("Hint line at the top", &mut st.settings.help as *mut bool),
                ("Tab animation", &mut st.settings.tab_anim as *mut bool),
                ("Left-handed", &mut st.settings.left_handed as *mut bool),
                ("Pencil only (a finger never picks)", &mut st.settings.pencil_only as *mut bool),
                ("Show frame times", &mut st.settings.show_perf as *mut bool),
            ] {
                // SAFETY: each pointer is a distinct field of st.settings,
                // alive for this loop, and nothing else borrows it meanwhile
                let v = unsafe { &mut *v };
                if toggle_row(ui, label, *v) {
                    *v = !*v;
                    changed = true;
                }
            }
            let mut t = st.settings.hold_time as f32;
            if slider_row(ui, "Hold time", &mut t, 0.25..=1.0, 0.05, "s").0 {
                st.settings.hold_time = t as f64;
                changed = true;
            }
            let mut res = st.settings.sdf_res as f32 + 1.0;
            if slider_row(ui, "SDF resolution", &mut res, 1.0..=4.0, 1.0, "").0 {
                st.settings.sdf_res = (res.round() as i32 - 1).clamp(0, 3) as u8;
                changed = true;
            }
            let mut step = st.settings.step as f32;
            if slider_row(ui, "Grid step", &mut step, 0.1..=10.0, 0.1, "mm").0 {
                st.settings.step = step.max(0.1) as f64;
                changed = true;
            }
            if changed {
                st.save_settings();
            }
            ui.add_space(6.0);
            let w = ui.available_width();
            if secondary_btn(ui, "Touch & Pencil gestures", w).clicked() {
                set_flag(ctx, "sheet", true);
                set_flag(ctx, "settings", false);
            }
        });
    });
    // the keypad of the grid step is outside the pop-over: it stays open
    let kp = ctx.memory(|m| m.area_rect(Id::new("keypad-area"))).filter(|_| ctx.data(|d| d.get_temp::<Keypad>(Id::new("keypad")).is_some()));
    let mut keep = vec![area.response.rect, btn];
    keep.extend(kp);
    if pressed_outside(ctx, &keep) {
        set_flag(ctx, "settings", false);
    }
}

// ------------------------------------------------------------ the dock

/// A tool of the dock: its icon over its label (74 × 54), white when on.
fn tool_btn(ui: &mut Ui, icon: Icon, label: &str, on: bool) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(DOCK_W - 16.0, 48.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(r, 13.0, accent());
    } else if resp.is_pointer_button_down_on() {
        p.rect_filled(r, 13.0, RAISED());
    }
    let fg = if on { on_accent() } else { pick(Color32::from_rgb(0xC3, 0xC8, 0xCF), Color32::from_rgb(0x3A, 0x40, 0x48)) };
    icons::paint(p, Rect::from_center_size(pos2(r.center().x, r.center().y - 7.0), vec2(19.0, 19.0)), icon, fg);
    p.text(pos2(r.center().x, r.center().y + 14.0), Align2::CENTER_CENTER, label, medium(11.0), fg);
    resp
}

/// A tool of half the dock's width (Q W E R).
fn small_tool_btn(ui: &mut Ui, icon: Icon, label: &str, on: bool) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2((DOCK_W - 18.0) * 0.5, 48.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(r, 11.0, accent());
    } else if resp.is_pointer_button_down_on() {
        p.rect_filled(r, 11.0, RAISED());
    }
    let fg = if on { on_accent() } else { pick(Color32::from_rgb(0xC3, 0xC8, 0xCF), Color32::from_rgb(0x3A, 0x40, 0x48)) };
    icons::paint(p, Rect::from_center_size(pos2(r.center().x, r.center().y - 7.0), vec2(18.0, 18.0)), icon, fg);
    p.text(pos2(r.center().x, r.center().y + 14.0), Align2::CENTER_CENTER, label, medium(9.5), fg);
    resp
}

fn dock_section(ui: &mut Ui, title: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(DOCK_W - 16.0, 20.0), Sense::hover());
    ui.painter().text(r.center() + vec2(0.0, 2.0), Align2::CENTER_CENTER, title, mono(9.5), LABEL());
}

/// The tools, on the side across from the scene list: Transform (Q W E R),
/// then the tab's own (Forme: Create and Modify; SDF: its five shapes),
/// the material ball at the bottom.
fn dock(ctx: &egui::Context, st: &mut AppState, screen: Rect, x: f32) {
    let top = T_TOP;
    let bottom = screen.max.y - E - HELP_D - 12.0;
    egui::Area::new(Id::new("t-dock")).order(Order::Middle).fixed_pos(pos2(x, top)).show(ctx, |ui| {
        island_t().inner_margin(Margin::symmetric(8, 8)).show(ui, |ui| {
            ui.set_width(DOCK_W - 16.0);
            let ball_h = 74.0;
            let room = (bottom - top - 16.0 - ball_h).max(120.0);
            egui::ScrollArea::vertical().id_salt("t-dock-scroll").max_height(room).min_scrolled_height(room).auto_shrink([false, true]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                dock_section(ui, "TRANSFORM");
                // Q W E R: two by two, so the tab's tools fit under them
                let modes = [(GizmoMode::Select, Icon::Select, "Select"), (GizmoMode::Move, Icon::Move, "Move"), (GizmoMode::Rotate, Icon::Rotate, "Rotate"), (GizmoMode::Scale, Icon::Scale, "Scale")];
                for row in modes.chunks(2) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (g, icon, label) in row {
                            if small_tool_btn(ui, *icon, label, st.tool == Tool::Select && st.gizmo == *g).clicked() {
                                st.select_tool();
                                st.set_gizmo(*g);
                            }
                        }
                    });
                }
                crate::tab_anim::enter(ui, vec2(14.0, 0.0), |ui| match st.workspace {
                    Workspace::Sdf => sdf_tools(ui, st),
                    _ => forme_tools(ctx, ui, st),
                });
            });
            ui.add_space(4.0);
            divider(ui);
            ui.add_space(4.0);
            // the material ball: its pop-over (the six balls, their settings)
            let cur = current_material(st).unwrap_or(st.material);
            let (r, resp) = ui.allocate_exact_size(vec2(DOCK_W - 16.0, ball_h - 12.0), Sense::click());
            let open = flag(ctx, "mats");
            if open || resp.is_pointer_button_down_on() {
                ui.painter().rect_filled(r, 13.0, RAISED());
            }
            material_ball(ui.painter(), pos2(r.center().x, r.center().y - 8.0), 14.0, &st.look(cur));
            ui.painter().text(pos2(r.center().x, r.center().y + 19.0), Align2::CENTER_CENTER, materials::get(cur).name, medium(11.0), TEXT());
            ctx.data_mut(|d| d.insert_temp(Id::new("t-ball"), r));
            if resp.clicked() {
                set_flag(ctx, "mats", !open);
            }
        });
    });
}

fn forme_tools(ctx: &egui::Context, ui: &mut Ui, st: &mut AppState) {
    dock_section(ui, "CREATE");
    for (k, label) in [(PrimKind::Box, "Cube"), (PrimKind::Cylinder, "Cylinder"), (PrimKind::Profile, "Sketch"), (PrimKind::Path, "Path")] {
        if tool_btn(ui, prim_icon(k), label, st.tool == Tool::Create(k)).clicked() {
            st.arm(k);
        }
    }
    // 3ds Max's shapes, the Surface and the Slot: a fly-out beside the dock
    let more_shapes = matches!(st.tool, Tool::Curve(_) | Tool::Slot | Tool::Create(PrimKind::Surface));
    let r = tool_btn(ui, Icon::CurveRect, "Shapes", more_shapes || flag(ctx, "curves"));
    ctx.data_mut(|d| d.insert_temp(Id::new("t-curves-btn"), r.rect));
    if r.clicked() {
        let open = flag(ctx, "curves");
        set_flag(ctx, "curves", !open);
        set_flag(ctx, "more", false);
    }
    if tool_btn(ui, Icon::Sphere, "Forms", st.tool == Tool::Form).clicked() {
        st.arm_form();
    }
    dock_section(ui, "MODIFY");
    for (k, icon, label) in [(PushKind::Inset, Icon::Inset, "Inset"), (PushKind::Extrude, Icon::Extrude, "Extrude")] {
        if tool_btn(ui, icon, label, st.tool == Tool::Push(k)).clicked() {
            st.arm_push(k, None);
        }
    }
    for (t, icon, label) in [(Tool::Knife, Icon::Knife, "Knife"), (Tool::Point, Icon::EdgePoint, "Point")] {
        if tool_btn(ui, icon, label, st.tool == t).clicked() {
            st.arm_knife(t);
        }
    }
    // Shell, Mirror, Array, Align: a fly-out
    let r = tool_btn(ui, Icon::Array, "More", st.tool == Tool::Align || flag(ctx, "more"));
    ctx.data_mut(|d| d.insert_temp(Id::new("t-more-btn"), r.rect));
    if r.clicked() {
        let open = flag(ctx, "more");
        set_flag(ctx, "more", !open);
        set_flag(ctx, "curves", false);
    }
}

fn sdf_tools(ui: &mut Ui, st: &mut AppState) {
    use crate::sdf::{prim_icon, prim_label, SDF_PRIMS};
    dock_section(ui, "SHAPES");
    for k in SDF_PRIMS {
        if tool_btn(ui, prim_icon(k), prim_label(k), false).clicked() {
            st.sdf_add(k);
        }
    }
}

/// A row of tools beside a dock button (`btn`: its rect's key), a thin
/// line before the items marked; the index of the one tapped.
fn flyout(ctx: &egui::Context, name: &str, btn: &str, dock_x: f32, flip: bool, items: &[(Icon, &str, bool, bool)]) -> Option<usize> {
    let Some(btn) = ctx.data(|d| d.get_temp::<Rect>(Id::new(btn))) else { return None };
    let (at, pivot) = if flip { (pos2(dock_x + DOCK_W + 10.0, btn.center().y), Align2::LEFT_CENTER) } else { (pos2(dock_x - 10.0, btn.center().y), Align2::RIGHT_CENTER) };
    let mut hit = None;
    let area = egui::Area::new(Id::new(("t-fly", name))).order(Order::Foreground).pivot(pivot).fixed_pos(at).show(ctx, |ui| {
        pop_t().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, (icon, label, on, line)) in items.iter().enumerate() {
                    if *line {
                        let (r, _) = ui.allocate_exact_size(vec2(9.0, 56.0), Sense::hover());
                        ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(1.0, 36.0)), 0.0, DIVIDER());
                    }
                    let (r, resp) = ui.allocate_exact_size(vec2(80.0, 56.0), Sense::click());
                    let p = ui.painter();
                    if *on {
                        p.rect_filled(r, 12.0, accent());
                    } else if resp.is_pointer_button_down_on() {
                        p.rect_filled(r, 12.0, RAISED());
                    }
                    let fg = if *on { on_accent() } else { TEXT() };
                    icons::paint(p, Rect::from_center_size(pos2(r.center().x, r.center().y - 8.0), vec2(20.0, 20.0)), *icon, fg);
                    p.text(pos2(r.center().x, r.center().y + 15.0), Align2::CENTER_CENTER, *label, medium(11.0), fg);
                    if resp.clicked() {
                        hit = Some(i);
                    }
                }
            });
        });
    });
    if hit.is_some() || pressed_outside(ctx, &[area.response.rect, btn]) {
        set_flag(ctx, name, false);
    }
    hit
}

/// The 3ds Max shapes, the Surface and the Slot beside the dock's Shapes.
fn curves_flyout(ctx: &egui::Context, st: &mut AppState, dock_x: f32, flip: bool) {
    if !flag(ctx, "curves") || st.workspace != Workspace::Forme {
        return;
    }
    let kinds = [(CurveKind::Rectangle, Icon::CurveRect, "Rectangle"), (CurveKind::Circle, Icon::CurveCircle, "Circle"), (CurveKind::Ellipse, Icon::CurveEllipse, "Ellipse"), (CurveKind::Polygon, Icon::CurvePolygon, "Polygon"), (CurveKind::Star, Icon::CurveStar, "Star")];
    let mut items: Vec<(Icon, &str, bool, bool)> = kinds.iter().map(|(k, i, l)| (*i, *l, st.tool == Tool::Curve(*k), false)).collect();
    items.push((Icon::Path, "Surface", st.tool == Tool::Create(PrimKind::Surface), true));
    items.push((Icon::Scan, "Slot", st.tool == Tool::Slot, false));
    match flyout(ctx, "curves", "t-curves-btn", dock_x, flip, &items) {
        Some(i) if i < kinds.len() => st.arm_curve(kinds[i].0),
        Some(5) => st.arm(PrimKind::Surface),
        Some(6) => st.arm_slot(),
        _ => {}
    }
}

/// Shell, Mirror, Array, Align beside the dock's More.
fn more_flyout(ctx: &egui::Context, st: &mut AppState, dock_x: f32, flip: bool) {
    if !flag(ctx, "more") || st.workspace != Workspace::Forme {
        return;
    }
    let items = [(Icon::Shell, "Shell", false, false), (Icon::Mirror, "Mirror", false, false), (Icon::Array, "Array", false, false), (Icon::Align, "Align", st.tool == Tool::Align, false)];
    match flyout(ctx, "more", "t-more-btn", dock_x, flip, &items) {
        Some(0) => {
            st.add_shell();
        }
        Some(1) => {
            st.add_mirror(0);
        }
        Some(2) => {
            st.add_array();
        }
        Some(3) => {
            st.begin_align();
        }
        _ => {}
    }
}

/// The six balls and the current one's settings, beside the dock's ball.
fn materials_pop(ctx: &egui::Context, st: &mut AppState, dock_x: f32, flip: bool) {
    if !flag(ctx, "mats") {
        return;
    }
    let Some(ball) = ctx.data(|d| d.get_temp::<Rect>(Id::new("t-ball"))) else { return };
    let (at, pivot) = if flip { (pos2(dock_x + DOCK_W + 10.0, ball.max.y), Align2::LEFT_BOTTOM) } else { (pos2(dock_x - 10.0, ball.max.y), Align2::RIGHT_BOTTOM) };
    materials_island_at(ctx, st, at, pivot, 300.0);
}

// ------------------------------------------------------------ the sidebar

/// The sidebar folded: one button to bring it back.
fn side_button(ctx: &egui::Context, st: &mut AppState, x: f32) {
    egui::Area::new(Id::new("t-side-btn")).order(Order::Middle).fixed_pos(pos2(x, T_TOP)).show(ctx, |ui| {
        island_t().inner_margin(Margin::same(4)).show(ui, |ui| {
            if icon_btn(ui, Icon::Sidebar, false, "Show the scene list", 44.0).clicked() {
                st.panel_visible = true;
            }
        });
    });
}

/// The scene list (the outliner, its rows the mock-up's), and at the bottom
/// what is selected: its position and rotation, folded.
fn sidebar(ctx: &egui::Context, st: &mut AppState, screen: Rect, x: f32) {
    let top = T_TOP;
    let bottom = screen.max.y - BAR_EDGE - 2.0 * KEY_D - KEY_GAP - 16.0;
    let total = (bottom - top - 24.0).max(200.0);
    let open = flag(ctx, "props");
    let props_h = if open { (total * 0.5).min(440.0) } else { 0.0 };
    egui::Area::new(Id::new("t-side")).order(Order::Middle).fixed_pos(pos2(x, top)).show(ctx, |ui| {
        island_t().inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(SIDE_W - 24.0);
            tree(ui, st, total - 58.0 - props_h);
            ui.add_space(4.0);
            divider(ui);
            // the selection's settings: folded under one row
            let (title, summary) = props_title(st);
            let (r, resp) = ui.allocate_exact_size(vec2(SIDE_W - 24.0, 48.0), Sense::click());
            let p = ui.painter();
            if resp.is_pointer_button_down_on() {
                p.rect_filled(r, 10.0, RAISED());
            }
            icons::paint(p, Rect::from_center_size(pos2(r.min.x + 10.0, r.center().y), vec2(12.0, 12.0)), if open { Icon::Chevron } else { Icon::ChevronRight }, TEXT2());
            p.text(pos2(r.min.x + 26.0, r.center().y), Align2::LEFT_CENTER, title, medium(15.0), TEXT());
            p.text(pos2(r.max.x - 6.0, r.center().y), Align2::RIGHT_CENTER, summary, regular(13.0), TEXT2());
            if resp.clicked() {
                set_flag(ctx, "props", !open);
            }
            if open {
                egui::ScrollArea::vertical().id_salt("t-props").max_height(props_h).min_scrolled_height(props_h).auto_shrink([false, true]).show(ui, |ui| {
                    ui.set_width(SIDE_W - 24.0);
                    selection_props(ui, st);
                });
            }
        });
    });
}

/// The bottom row of the sidebar: what it holds, and a short summary.
fn props_title(st: &AppState) -> (&'static str, String) {
    let n = |v: f64| {
        let r = (v * 10.0).round() / 10.0;
        if r.fract() == 0.0 { format!("{}", r as i64) } else { fmt_mm(r as f32) }
    };
    if st.selected_group().is_some() {
        return ("Group transform", String::new());
    }
    if let Some(f) = st.single() {
        if f.is_solid() {
            return ("Position and rotation", format!("{} · {} · {}", n(f.origin.x), n(f.origin.y), n(f.origin.z)));
        }
        return ("Properties", f.name.clone());
    }
    if !st.selection.is_empty() {
        return ("Selection", format!("{} shapes", st.selection.len()));
    }
    ("New shapes", String::new())
}

// ------------------------------------------------------------ the view

/// The hint at the top of the view: the tool and what the Pencil and the
/// fingers do there now (the PC's help box, its keys left out).
fn hint_pill(ctx: &egui::Context, st: &mut AppState) {
    if !st.settings.help || flag(ctx, "sheet") {
        return;
    }
    let parts = touch_hint(st);
    let vr = st.view_rect;
    let flip = st.settings.left_handed;
    let x = if !st.panel_visible && !flip { vr[0] + 64.0 } else { vr[0] + 8.0 };
    egui::Area::new(Id::new("t-hint")).order(Order::Middle).interactable(false).fixed_pos(pos2(x, T_TOP)).show(ctx, |ui| {
        egui::Frame::new()
            .fill(tint(PANEL(), 0.92))
            .stroke(Stroke::new(1.0, PANEL_BORDER()))
            .corner_radius(egui::CornerRadius::same(22))
            .inner_margin(Margin::symmetric(16, 0))
            .show(ui, |ui| {
                ui.set_height(44.0);
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.label(egui::RichText::new(help_title(st)).font(medium(14.0)).color(TEXT_STRONG()));
                    for p in parts {
                        ui.label(egui::RichText::new("·").font(regular(14.0)).color(LABEL()));
                        ui.label(egui::RichText::new(p).font(regular(14.0)).color(TEXT2()));
                    }
                });
            });
    });
}

/// The PC's hint said for the iPad: the parts the Pencil and the fingers
/// can do (no keys), "click" said "tap", at most three.
pub(super) fn touch_hint(st: &AppState) -> Vec<String> {
    let text = crate::viewport::hint(st);
    let mut out = vec![];
    for part in text.split(" · ") {
        let part = part.trim();
        let key = part.split_once(": ").map(|(k, _)| k).unwrap_or(part);
        // "Shift + drag", "Ctrl + click" are the round buttons; "Ctrl +
        // Shift + D", "Alt + M" need a keyboard
        let tokens: Vec<&str> = key.split(" + ").map(str::trim).collect();
        let (last, mods) = tokens.split_last().map(|(l, m)| (*l, m)).unwrap_or((key, &[]));
        let gesture = ["Click", "Drag", "Double-click", "Press", "Move", "Release", "Arrow", "Dot", "First point", "Away", "Up", "Down", "Centre", "Cubes", "Rings", "Square", "Grip"];
        let ok = gesture.iter().any(|w| last.to_lowercase().starts_with(&w.to_lowercase())) && mods.iter().all(|m| *m == "Shift" || *m == "Ctrl");
        let keyboard = ["Esc", "Enter", "Del", "Tab", "Backspace", "D held", "Alt"].iter().any(|w| key.contains(w));
        if !ok || keyboard || part.len() > 64 {
            continue;
        }
        let t = part.replace("Double-click", "Double-tap").replace("double-click", "double-tap").replace("Right click", "Hold").replace("right click", "hold").replace("Click", "Tap").replace("click", "tap");
        out.push(t);
        if out.len() == 3 {
            break;
        }
    }
    out
}

/// Where the action bar sits: above the box of what is selected or picked
/// (its bottom centre), below the hint.
pub(super) fn bar_at(st: &AppState) -> Option<egui::Pos2> {
    if st.tool != Tool::Select || st.creation.is_some() || st.drag.is_some() || st.marking.is_open() || st.align.is_some() || st.push.is_some() {
        return None;
    }
    let comps = st.selection.is_empty() && (!st.edge_sel.is_empty() || !st.face_sel.is_empty() || !st.vert_sel.is_empty());
    if !comps && context_bar_at(st).is_none() {
        return None;
    }
    let (lo, hi) = if comps {
        st.selection_bounds().map(|(a, b)| (a.as_dvec3(), b.as_dvec3()))?
    } else {
        st.doc.features.iter().filter(|f| st.selection.contains(&f.id)).map(|f| f.bounds()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))?
    };
    let view = st.view();
    let (mut top, mut x0, mut x1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY);
    for i in 0..8 {
        let c = DVec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
        if let Some(p) = view.project(c.as_vec3()) {
            top = top.min(p[1]);
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
        }
    }
    if !top.is_finite() {
        return None;
    }
    let vr = st.view_rect;
    let room = if st.shown_group().is_some() { 150.0 } else { 120.0 };
    Some(pos2(((x0 + x1) * 0.5).clamp(vr[0] + 220.0, vr[2] - 220.0), (top - 24.0).clamp(vr[1] + room, vr[3] - 160.0)))
}

/// One labelled button of the action bar (an icon over its label).
fn act_btn(ui: &mut Ui, icon: Option<Icon>, label: &str, color: Color32, on: bool) -> (Response, Rect) {
    let g = ui.painter().layout_no_wrap(label.to_string(), medium(12.0), color);
    let w = (g.size().x + 20.0).max(76.0);
    let (r, resp) = ui.allocate_exact_size(vec2(w, 52.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(r, 12.0, accent());
    } else if resp.is_pointer_button_down_on() {
        p.rect_filled(r, 12.0, RAISED());
    }
    let fg = if on { on_accent() } else { color };
    if let Some(i) = icon {
        icons::paint(p, Rect::from_center_size(pos2(r.center().x, r.center().y - 9.0), vec2(19.0, 19.0)), i, fg);
    }
    p.text(pos2(r.center().x, r.center().y + 14.0), Align2::CENTER_CENTER, label, medium(12.0), fg);
    (resp, r)
}

fn act_sep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(9.0, 52.0), Sense::hover());
    ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(1.0, 34.0)), 0.0, DIVIDER());
}

/// The actions of what is selected or picked, in a bar above it, labelled
/// (the mock-up): a shape's Duplicate, Hide, Make Carve, Frame, Material,
/// Delete; picked edges' Fillet, Chamfer, Select loop, Deselect; picked
/// faces' Extrude, Inset, Shell, Deselect; an SDF shape's.
fn action_bar(ctx: &egui::Context, st: &mut AppState) {
    ctx.data_mut(|d| d.remove::<Rect>(Id::new("t-actions-rect")));
    let Some(mut at) = bar_at(st) else { return };
    // off the operation window (it stays where it took its place; the bar
    // follows the selection): beside it, else under it
    let size = ctx.memory(|m| m.area_rect(Id::new("t-actions"))).map(|r| r.size()).unwrap_or(vec2(420.0, 62.0));
    let op = ctx.memory(|m| m.areas().visible_layer_ids().contains(&egui::LayerId::new(Order::Foreground, Id::new("op-pop"))).then(|| m.area_rect(Id::new("op-pop"))).flatten());
    if let Some(op) = op {
        let vr = st.view_rect;
        let bar = |at: egui::Pos2| Rect::from_min_size(pos2(at.x - size.x * 0.5, at.y - size.y), size);
        if bar(at).intersects(op.expand(8.0)) {
            let left = pos2(op.min.x - 12.0 - size.x * 0.5, at.y);
            let right = pos2(op.max.x + 12.0 + size.x * 0.5, at.y);
            if left.x - size.x * 0.5 >= vr[0] + 8.0 {
                at = left;
            } else if right.x + size.x * 0.5 <= vr[2] - 8.0 {
                at = right;
            } else {
                at.y = (op.max.y + 12.0 + size.y).min(vr[3] - 90.0);
            }
        }
    }
    let shown = egui::Area::new(Id::new("t-actions")).order(Order::Middle).pivot(Align2::CENTER_BOTTOM).fixed_pos(at).show(ctx, |ui| {
        island_t().inner_margin(Margin::same(5)).corner_radius(egui::CornerRadius::same(16)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let count = |ui: &mut Ui, text: String, c: Color32| {
                    let g = ui.painter().layout_no_wrap(text, mono(12.5), c);
                    let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 24.0, 52.0), Sense::hover());
                    ui.painter().galley(pos2(r.min.x + 12.0, r.center().y - g.size().y * 0.5), g, c);
                };
                if st.selection.is_empty() {
                    let deselect = |ui: &mut Ui, st: &mut AppState| {
                        act_sep(ui);
                        if act_btn(ui, Some(Icon::Close), "Deselect", TEXT(), false).0.clicked() {
                            st.edge_sel.clear();
                            st.face_sel.clear();
                            st.vert_sel.clear();
                        }
                    };
                    if !st.edge_sel.is_empty() {
                        let n = st.edge_sel.len();
                        count(ui, format!("{n} edge{}", if n > 1 { "s" } else { "" }), EDGE_SEL);
                        for (c, label) in [(false, "Fillet"), (true, "Chamfer")] {
                            if act_btn(ui, Some(Icon::Fillet), label, TEXT(), false).0.clicked() {
                                st.round_chamfer = c;
                                st.set_round(1.0, c, true);
                                st.end_live();
                                st.edge_sel.clear();
                            }
                        }
                        if act_btn(ui, Some(Icon::Loop), "Select loop", TEXT(), false).0.clicked() {
                            if let Some(i) = st.edge_sel.first().and_then(|r| st.model_edge_of(r)) {
                                st.pick_linked_edges(i, true);
                            }
                        }
                        deselect(ui, st);
                    } else if !st.face_sel.is_empty() {
                        let n = st.face_sel.len();
                        count(ui, format!("{n} face{}", if n > 1 { "s" } else { "" }), FACE_SEL);
                        if act_btn(ui, Some(Icon::Extrude), "Extrude", TEXT(), false).0.clicked() {
                            st.arm_push(PushKind::Extrude, None);
                        }
                        if act_btn(ui, Some(Icon::Inset), "Inset", TEXT(), false).0.clicked() {
                            st.arm_push(PushKind::Inset, None);
                        }
                        if act_btn(ui, Some(Icon::Shell), "Shell", TEXT(), false).0.clicked() {
                            st.add_shell();
                        }
                        if act_btn(ui, Some(Icon::Frame), "Frame", TEXT(), false).0.clicked() {
                            st.frame_current();
                        }
                        deselect(ui, st);
                    } else {
                        let n = st.vert_sel.len();
                        count(ui, format!("{n} vert{}", if n > 1 { "ices" } else { "ex" }), TEXT2());
                        if act_btn(ui, Some(Icon::Frame), "Frame", TEXT(), false).0.clicked() {
                            st.frame_current();
                        }
                        deselect(ui, st);
                    }
                    return;
                }
                // an SDF shape picked in its object
                if st.sdf_comp() && st.sdf_picked().is_some() {
                    if act_btn(ui, Some(Icon::Duplicate), "Duplicate", TEXT(), false).0.clicked() {
                        st.sdf_duplicate();
                    }
                    if act_btn(ui, Some(Icon::Frame), "Frame", TEXT(), false).0.clicked() {
                        st.frame_current();
                    }
                    material_act(ctx, ui, st);
                    act_sep(ui);
                    if act_btn(ui, Some(Icon::Trash), "Delete", carve_light(), false).0.clicked() {
                        st.sdf_delete();
                    }
                    return;
                }
                let solids: Vec<FeatureId> = st.selection.iter().copied().filter(|id| st.doc.feature(*id).map(|f| f.is_solid()).unwrap_or(false)).collect();
                if !solids.is_empty() {
                    if act_btn(ui, Some(Icon::Duplicate), "Duplicate", TEXT(), false).0.clicked() {
                        st.duplicate_selected();
                    }
                    let shown = solids.iter().any(|id| st.doc.feature(*id).is_some_and(|f| f.visible));
                    if act_btn(ui, Some(if shown { Icon::EyeOff } else { Icon::Eye }), if shown { "Hide" } else { "Show" }, TEXT(), false).0.clicked() {
                        for id in &solids {
                            if st.doc.feature(*id).is_some_and(|f| f.visible == shown) {
                                st.toggle_visible(*id);
                            }
                        }
                    }
                    let solid_shapes = solids.iter().any(|id| st.doc.feature(*id).is_some_and(|f| !f.is_path() && !f.is_mesh()));
                    if solid_shapes {
                        let op = shown_op(st);
                        let other = op.other();
                        let label = if other == Op::Carve { "Make Carve" } else { "Make Add" };
                        let (resp, r) = act_btn(ui, None, label, TEXT(), false);
                        paint_op_glyph(ui.painter(), pos2(r.center().x, r.center().y - 9.0), other, op_color(other), 12.0);
                        if resp.clicked() {
                            set_op_from_ui(st, other);
                        }
                    }
                    if st.can_group() && st.selection.len() > 1 && act_btn(ui, Some(Icon::Group), "Group", TEXT(), false).0.clicked() {
                        st.group_selection();
                    }
                }
                if act_btn(ui, Some(Icon::Frame), "Frame", TEXT(), false).0.clicked() {
                    st.frame_selection();
                }
                if !solids.is_empty() {
                    material_act(ctx, ui, st);
                }
                act_sep(ui);
                if act_btn(ui, Some(Icon::Trash), "Delete", carve_light(), false).0.clicked() {
                    st.delete_selected();
                }
            });
        });
    });
    // the operation window keeps off it (`op_place`)
    ctx.data_mut(|d| d.insert_temp(Id::new("t-actions-rect"), shown.response.rect));
}

/// The action bar's Material: the current ball, opens the materials.
fn material_act(ctx: &egui::Context, ui: &mut Ui, st: &mut AppState) {
    let cur = current_material(st).unwrap_or(st.material);
    let open = flag(ctx, "mats");
    let (resp, r) = act_btn(ui, None, "Material", TEXT(), false);
    material_ball(ui.painter(), pos2(r.center().x, r.center().y - 9.0), 8.0, &st.look(cur));
    if resp.clicked() {
        set_flag(ctx, "mats", !open);
    }
}

// ------------------------------------------------------------ help

/// The round "?" at the bottom corner: the Touch & Pencil sheet.
fn help_button(ctx: &egui::Context, st: &mut AppState, screen: Rect, flip: bool) {
    let c = if flip { pos2(screen.min.x + E + HELP_D * 0.5, screen.max.y - E - HELP_D * 0.5) } else { pos2(screen.max.x - E - HELP_D * 0.5, screen.max.y - E - HELP_D * 0.5) };
    let _ = st;
    egui::Area::new(Id::new("t-help")).order(Order::Middle).fixed_pos(c - vec2(HELP_D, HELP_D) * 0.5).show(ctx, |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(HELP_D, HELP_D), Sense::click());
        let p = ui.painter();
        p.circle_filled(r.center() + vec2(0.0, 2.0), HELP_D * 0.5 + 1.0, Color32::from_black_alpha(70));
        p.circle(r.center(), HELP_D * 0.5, if resp.is_pointer_button_down_on() { RAISED() } else { tint(PANEL(), 0.96) }, Stroke::new(1.0, PANEL_BORDER()));
        icons::paint(p, Rect::from_center_size(r.center(), vec2(24.0, 24.0)), Icon::Help, TEXT());
        if resp.clicked() {
            let open = flag(ctx, "sheet");
            set_flag(ctx, "sheet", !open);
        }
    });
}

/// A gesture's row in the sheet: its picture, its name, what it does.
fn gesture_row(ui: &mut Ui, icon: Icon, name: &str, what: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        let (r, _) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::hover());
        ui.painter().rect_filled(r, 12.0, RAISED());
        icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(26.0, 26.0)), icon, TEXT());
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(4.0);
            ui.label(egui::RichText::new(name).font(semibold(15.0)).color(TEXT_STRONG()));
            ui.add(egui::Label::new(egui::RichText::new(what).font(regular(13.0)).color(TEXT2())).wrap());
        });
    });
    ui.add_space(8.0);
}

fn bullet(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (r, _) = ui.allocate_exact_size(vec2(6.0, 20.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 2.2, LABEL());
        ui.label(egui::RichText::new(text).font(regular(14.0)).color(TEXT()));
    });
}

fn col_title(ui: &mut Ui, t: &str) {
    ui.label(egui::RichText::new(t).font(mono(11.0)).color(LABEL()));
    ui.add_space(8.0);
}

/// The Touch & Pencil sheet (the mock-up's): the fingers, the Pencil,
/// Shift and Ctrl, then the settings (left-handed, Pencil only, the hold
/// time).
pub(super) fn gesture_sheet(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    if !flag(ctx, "sheet") {
        return;
    }
    if dim(ctx, screen, "t-sheet-dim") {
        set_flag(ctx, "sheet", false);
        return;
    }
    let w = (screen.width() - 2.0 * 90.0).min(1000.0);
    let hold = format!("{:.2} s", st.settings.hold_time).replace(".00", "").replace('.', ",");
    egui::Area::new(Id::new("t-sheet")).order(Order::Foreground).pivot(Align2::CENTER_CENTER).fixed_pos(screen.center()).show(ctx, |ui| {
        pop_t().corner_radius(egui::CornerRadius::same(22)).inner_margin(Margin::same(0)).show(ui, |ui| {
            ui.set_width(w);
            // the header
            egui::Frame::new().inner_margin(Margin { left: 28, right: 20, top: 20, bottom: 16 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("Touch & Pencil").font(semibold(22.0)).color(TEXT_STRONG()));
                        ui.label(egui::RichText::new("The Pencil does the work, the fingers move the view. Shift and Ctrl change what the Pencil does.").font(regular(14.0)).color(TEXT2()));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (r, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
                        ui.painter().circle_filled(r.center(), 22.0, RAISED());
                        icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(18.0, 18.0)), Icon::Close, TEXT());
                        if resp.clicked() {
                            set_flag(ctx, "sheet", false);
                        }
                    });
                });
            });
            divider(ui);
            let col_w = (w - 2.0 * 28.0 - 2.0 * 28.0) / 3.0;
            egui::Frame::new().inner_margin(Margin::symmetric(28, 20)).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 28.0;
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        col_title(ui, "FINGERS · IN THE VIEW");
                        gesture_row(ui, Icon::Orbit, "Drag one finger", "Orbit");
                        gesture_row(ui, Icon::Pan, "Drag two fingers", "Pan");
                        gesture_row(ui, Icon::Pinch, "Pinch", "Zoom");
                        gesture_row(ui, Icon::Tap, "Tap", if st.settings.pencil_only { "Nothing (Pencil only is on)" } else { "Pick what is under it" });
                        gesture_row(ui, Icon::Hold, &format!("Hold still {hold}"), "Marking menu, flick to an item; while a sketch or path is drawn: close it");
                        gesture_row(ui, Icon::TwoTap, "Two-finger tap", "Undo");
                        gesture_row(ui, Icon::ThreeTap, "Three-finger tap", "Redo");
                        gesture_row(ui, Icon::DoubleTap, "Double-tap", "On a shape: frame it. On empty space: frame all");
                    });
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        col_title(ui, "APPLE PENCIL");
                        gesture_row(ui, Icon::Pencil, "Tap, drag", "The tool: pick, draw a shape, a sketch, a box round components");
                        gesture_row(ui, Icon::Fillet, "Drag on an edge", "Fillet, its size grows with the drag");
                        gesture_row(ui, Icon::Marquee, "Drag on empty space", "Box selection in Vertex, Edge and Face modes");
                        gesture_row(ui, Icon::Sketch, "Sketch or Path", "Tap the first point to close it; a finger held still ends a path open");
                        gesture_row(ui, Icon::Palm, "Palm on the glass", "Ignored while the Pencil is down");
                        gesture_row(ui, Icon::Sliders, "A value", "Tap it: the keypad. − and +: one step. Drag the slider");
                    });
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        col_title(ui, "SHIFT AND CTRL");
                        for (k, name, what) in [("Ctrl", "Hold with the thumb", "On while held, the Pencil works with it"), ("Shift", "Tap", "Stays on until tapped again (a dot shows it)")] {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 14.0;
                                let (r, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
                                let on = k == "Shift";
                                ui.painter().circle(r.center(), 28.0, if on { Color32::from_rgb(0xF2, 0xF3, 0xF5) } else { tint(PANEL(), 0.96) }, Stroke::new(1.0, PANEL_BORDER()));
                                ui.painter().text(r.center(), Align2::CENTER_CENTER, k, semibold(14.0), if on { Color32::from_rgb(0x0F, 0x11, 0x14) } else { TEXT() });
                                ui.vertical(|ui| {
                                    ui.add_space(8.0);
                                    ui.label(egui::RichText::new(name).font(semibold(15.0)).color(TEXT_STRONG()));
                                    ui.add(egui::Label::new(egui::RichText::new(what).font(regular(13.0)).color(TEXT2())).wrap());
                                });
                            });
                            ui.add_space(6.0);
                        }
                        divider(ui);
                        ui.add_space(8.0);
                        col_title(ui, "SHIFT");
                        for t in ["Add to the pick", "Fillet tension instead of size", "Drag a picked face: Extrude", "Drag a picked edge: pull it out", "Sketch, Path: sides at 0°, 45°, 90°"] {
                            bullet(ui, t);
                        }
                        ui.add_space(6.0);
                        col_title(ui, "CTRL");
                        for t in ["Take away from the pick", "Chamfer instead of fillet", "Drag a picked face: Inset", "No snapping, grid only", "Sketch: drag a point to round it"] {
                            bullet(ui, t);
                        }
                    });
                });
            });
            divider(ui);
            // the settings
            egui::Frame::new().inner_margin(Margin::symmetric(28, 14)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 18.0;
                    let mut changed = false;
                    let third = (w - 56.0 - 2.0 * 18.0) / 3.0;
                    for (name, what, v) in [
                        ("Left-handed", "Shift / Ctrl and the dock swap sides", &mut st.settings.left_handed as *mut bool),
                        ("Pencil only", "A finger tap never picks", &mut st.settings.pencil_only as *mut bool),
                    ] {
                        // SAFETY: two distinct fields of st.settings, nothing
                        // else borrows them meanwhile
                        let v = unsafe { &mut *v };
                        ui.allocate_ui(vec2(third, 56.0), |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.set_width(third - 64.0);
                                    ui.label(egui::RichText::new(name).font(semibold(15.0)).color(TEXT_STRONG()));
                                    ui.label(egui::RichText::new(what).font(regular(13.0)).color(TEXT2()));
                                });
                                let (r, resp) = ui.allocate_exact_size(vec2(52.0, 32.0), Sense::click());
                                switch(ui.painter(), r, *v, 1.0);
                                if resp.clicked() {
                                    *v = !*v;
                                    changed = true;
                                }
                            });
                        });
                    }
                    ui.allocate_ui(vec2(third, 56.0), |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.set_width(third - 170.0);
                                ui.label(egui::RichText::new("Hold time").font(semibold(15.0)).color(TEXT_STRONG()));
                                ui.label(egui::RichText::new("Before the marking menu").font(regular(13.0)).color(TEXT2()));
                            });
                            let mut t = st.settings.hold_time as f32;
                            let mut ch = false;
                            if icon_btn(ui, Icon::Minus, false, "Shorter", 40.0).clicked() {
                                t = (t - 0.05).max(0.25);
                                ch = true;
                            }
                            if value_field(ui, &mut t, 0.25..=1.0, 0.05, "s", 76.0).changed() {
                                ch = true;
                            }
                            if icon_btn(ui, Icon::Plus, false, "Longer", 40.0).clicked() {
                                t = (t + 0.05).min(1.0);
                                ch = true;
                            }
                            if ch {
                                st.settings.hold_time = ((t as f64) * 100.0).round() / 100.0;
                                changed = true;
                            }
                        });
                    });
                    if changed {
                        st.save_settings();
                    }
                });
            });
        });
    });
}
