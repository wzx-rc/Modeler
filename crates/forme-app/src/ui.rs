//! Atelier interface: viewport first, floating panels (top bar, tool rail with
//! the modes, properties + shape tree on the right, history strip at the
//! bottom), plus the dialogs and toasts shared with the Studio interface.

use crate::camera::ViewPreset;
use crate::files;
use crate::icons::{self, Icon};
use crate::settings::UiMode;
use crate::state::*;
use crate::theme::*;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Frame, Id, Margin, Order, Rect, Response, Sense, Stroke, StrokeKind, Ui};
use forme_core::*;
use glam::{EulerRot, Quat, Vec3};

const RIGHT_W: f32 = 276.0;

/// Desktop layout: compact rows for the mouse (the iPad keeps 44 pt targets).
const DESK: bool = !cfg!(target_os = "ios");

pub(crate) fn panel() -> Frame {
    Frame::new().fill(PANEL).stroke(Stroke::new(1.0, PANEL_BORDER)).corner_radius(CornerRadius::same(16)).inner_margin(Margin::same(8))
}

pub(crate) fn mode_color(m: CombineMode) -> Color32 {
    match m {
        CombineMode::Add => ACCENT,
        CombineMode::Carve => CARVE,
        _ => NEUTRAL_MODE,
    }
}

/// Mode glyphs drawn as vectors (Inter has no ∩).
pub fn paint_mode_glyph(p: &egui::Painter, c: egui::Pos2, m: CombineMode, color: Color32, size: f32) {
    let s = size * 0.5;
    let st = Stroke::new((size * 0.13).max(1.4), color);
    match m {
        CombineMode::Add => {
            p.line_segment([c - vec2(s, 0.0), c + vec2(s, 0.0)], st);
            p.line_segment([c - vec2(0.0, s), c + vec2(0.0, s)], st);
        }
        CombineMode::Carve => {
            p.line_segment([c - vec2(s, 0.0), c + vec2(s, 0.0)], st);
        }
        CombineMode::Intersect | CombineMode::Tongue | CombineMode::Groove => {
            let n = 12;
            let up = m != CombineMode::Groove;
            let mut pts: Vec<egui::Pos2> = vec![];
            for i in 0..=n {
                let a = std::f32::consts::PI * i as f32 / n as f32;
                let y = if up { -a.sin() } else { a.sin() };
                pts.push(c + vec2(-a.cos() * s * 0.75, y * s * 0.7 + if up { s * 0.35 } else { -s * 0.35 }));
            }
            if m == CombineMode::Intersect {
                pts.insert(0, c + vec2(-s * 0.75, s * 0.9));
                pts.push(c + vec2(s * 0.75, s * 0.9));
            } else {
                let base = if up { s * 0.35 } else { -s * 0.35 };
                pts.insert(0, c + vec2(-s, base));
                pts.push(c + vec2(s, base));
            }
            p.add(egui::Shape::line(pts, st));
        }
    }
}

pub(crate) fn prim_icon(k: PrimKind) -> Icon {
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

// ----------------------------------------------------------------- widgets

pub fn icon_btn(ui: &mut Ui, icon: Icon, selected: bool, tip: &str, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let p = ui.painter();
    if selected {
        p.rect(rect.shrink(0.75), 10.0, ACCENT_BG, Stroke::new(1.5, ACCENT), StrokeKind::Inside);
    } else if resp.hovered() || resp.is_pointer_button_down_on() {
        p.rect_filled(rect, 10.0, RAISED);
    }
    let c = if selected { ACCENT_TEXT } else { TEXT_ICON };
    let isz = if size < 36.0 { 16.0 } else { 20.0 };
    icons::paint(p, Rect::from_center_size(rect.center(), vec2(isz, isz)), icon, c);
    resp.on_hover_text(tip)
}

pub(crate) fn toggle_chip(ui: &mut Ui, icon: Icon, label: &str, on: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), medium(13.0), TEXT);
    let w = galley.size().x + 24.0 + 22.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
    let p = ui.painter();
    let (bg, border, fg) = if on { (ACCENT_BG, ACCENT_BORDER, ACCENT_TEXT) } else { (Color32::TRANSPARENT, DIVIDER, TEXT_ICON) };
    p.rect(rect.shrink(0.5), 10.0, bg, Stroke::new(1.0, border), StrokeKind::Inside);
    icons::paint(p, Rect::from_min_size(rect.min + vec2(12.0, 14.0), vec2(16.0, 16.0)), icon, fg);
    p.text(rect.min + vec2(34.0, 22.0), Align2::LEFT_CENTER, label, medium(13.0), fg);
    resp
}

pub(crate) fn primary_btn(ui: &mut Ui, icon: Option<Icon>, label: &str, width: Option<f32>) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), semibold(14.0), ON_ACCENT);
    let w = width.unwrap_or(galley.size().x + 32.0 + if icon.is_some() { 24.0 } else { 0.0 });
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
    let p = ui.painter();
    let fill = if resp.is_pointer_button_down_on() { ACCENT_LIGHT } else { ACCENT };
    p.rect_filled(rect, 10.0, fill);
    let tw = galley.size().x + if icon.is_some() { 24.0 } else { 0.0 };
    let x0 = rect.center().x - tw * 0.5;
    if let Some(i) = icon {
        icons::paint(p, Rect::from_min_size(pos2(x0, rect.center().y - 8.0), vec2(16.0, 16.0)), i, ON_ACCENT);
    }
    p.galley(pos2(x0 + if icon.is_some() { 24.0 } else { 0.0 }, rect.center().y - galley.size().y * 0.5), galley, ON_ACCENT);
    resp
}

pub(crate) fn secondary_btn(ui: &mut Ui, label: &str, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 44.0), Sense::click());
    let p = ui.painter();
    let fill = if resp.hovered() { RAISED } else { Color32::TRANSPARENT };
    p.rect(rect.shrink(0.5), 10.0, fill, Stroke::new(1.0, DIVIDER), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, label, medium(14.0), LABEL);
    resp
}

/// Segmented control. Returns the picked value.
pub(crate) fn segmented<T: PartialEq + Copy>(ui: &mut Ui, items: &[(T, &str, Color32)], current: T, height: f32) -> Option<T> {
    let height = if DESK { height.min(28.0) } else { height };
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    ui.painter().rect_filled(rect, 11.0, WELL);
    let n = items.len() as f32;
    let inner = rect.shrink(3.0);
    let w = (inner.width() - 3.0 * (n - 1.0)) / n;
    let mut picked = None;
    for (i, (v, label, color)) in items.iter().enumerate() {
        let r = Rect::from_min_size(inner.min + vec2(i as f32 * (w + 3.0), 0.0), vec2(w, inner.height()));
        let resp = ui.interact(r, ui.id().with(("seg", i, label)), Sense::click());
        let on = *v == current;
        let p = ui.painter();
        if on {
            p.rect(r, 8.0, tint(*color, 0.16), Stroke::new(1.5, *color), StrokeKind::Inside);
        } else if resp.hovered() {
            p.rect_filled(r, 8.0, RAISED);
        }
        p.text(r.center(), Align2::CENTER_CENTER, *label, semibold(if DESK { 11.5 } else { 12.0 }), if on { TEXT_STRONG } else { Color32::from_rgb(0xAE, 0xB3, 0xBA) });
        if resp.clicked() {
            picked = Some(*v);
        }
    }
    picked
}

pub(crate) fn section(ui: &mut Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(title).font(semibold(12.0)).color(TEXT2));
}

fn nice_ceil(x: f32) -> f32 {
    if x <= 0.0 {
        return 1.0;
    }
    let e = 10f32.powf(x.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if m * e >= x {
            return m * e;
        }
    }
    10.0 * e
}

/// Touch-friendly slider (44 pt tall hit area). Sizes use an adaptive range so the
/// knob sits mid-track whatever the scale; the range is frozen while dragging.
fn touch_slider(ui: &mut Ui, id: Id, v: &mut f32, min: f32, max: f32, adaptive: bool, step: f32, width: f32) -> (bool, bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, if DESK { 22.0 } else { 34.0 }), Sense::click_and_drag());
    let resp = ui.interact(rect.expand2(vec2(0.0, 5.0)), id, Sense::click_and_drag());
    let key = id.with("frozen-max");
    let soft = if adaptive { nice_ceil((v.abs() * 2.0).max(max)) } else { max };
    let mut hi = soft;
    if resp.drag_started() || resp.is_pointer_button_down_on() && !resp.dragged() {
        ui.data_mut(|d| d.insert_temp(key, soft));
    }
    if resp.dragged() || resp.drag_stopped() {
        hi = ui.data(|d| d.get_temp::<f32>(key)).unwrap_or(soft);
    }
    let rail = Rect::from_center_size(rect.center(), vec2(width - if DESK { 12.0 } else { 20.0 }, if DESK { 3.0 } else { 4.0 }));
    let mut changed = false;
    let started = resp.drag_started() || resp.clicked();
    if resp.dragged() || resp.clicked() || resp.drag_started() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.x - rail.min.x) / rail.width()).clamp(0.0, 1.0);
            let mut nv = min + t * (hi - min);
            if step > 0.0 {
                nv = (nv / step).round() * step;
            }
            if (nv - *v).abs() > 1e-6 {
                *v = nv;
                changed = true;
            }
        }
    }
    let t = ((*v - min) / (hi - min)).clamp(0.0, 1.0);
    let kx = rail.min.x + t * rail.width();
    let p = ui.painter();
    p.rect_filled(rail, 2.0, Color32::from_rgb(0x3A, 0x3D, 0x44));
    let zero = if min < 0.0 { rail.min.x + (-min / (hi - min)) * rail.width() } else { rail.min.x };
    p.rect_filled(Rect::from_x_y_ranges(zero.min(kx)..=zero.max(kx), rail.y_range()), 2.0, ACCENT);
    let k = pos2(kx, rail.center().y);
    let active = resp.dragged() || resp.is_pointer_button_down_on();
    let (kr, ka) = if DESK { (6.5, 7.5) } else { (9.5, 11.0) };
    p.circle_filled(k + vec2(0.0, 1.0), kr + 1.0, tint(Color32::BLACK, 0.35));
    p.circle(k, if active { ka } else { kr }, Color32::from_rgb(0xF2, 0xF3, 0xF5), Stroke::new(if active { 2.5 } else { 0.0 }, ACCENT));
    (changed, started && changed || resp.drag_started())
}

/// Number field in mm / ° / ×, French decimals, typed or dragged, fixed width
/// (the panel never changes width with the value).
fn value_field(ui: &mut Ui, v: &mut f32, range: std::ops::RangeInclusive<f32>, step: f64, suffix: &str, width: f32) -> egui::Response {
    let sfx = if suffix.is_empty() { String::new() } else { format!(" {suffix}") };
    let parse_sfx = suffix.to_string();
    let field = egui::DragValue::new(v)
        .speed(step)
        .range(range)
        .custom_formatter(move |x, _| format!("{}{}", fmt_mm(x as f32), sfx))
        .custom_parser(move |s| s.trim().trim_end_matches(parse_sfx.as_str()).trim().replace(',', ".").parse::<f64>().ok());
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = WELL;
        w.inactive.bg_stroke = Stroke::new(1.0, DIVIDER);
        w.hovered.weak_bg_fill = RAISED;
        w.inactive.fg_stroke.color = TEXT_STRONG;
        w.inactive.corner_radius = CornerRadius::same(6);
        w.hovered.corner_radius = CornerRadius::same(6);
        w.active.corner_radius = CornerRadius::same(6);
        ui.style_mut().override_font_id = Some(regular(12.5));
        // a DragValue is as wide as its text, at least interact_size: pin both
        ui.spacing_mut().interact_size = vec2(width, 22.0);
        ui.spacing_mut().button_padding = vec2(4.0, 1.0);
        ui.add(field)
    })
    .inner
}

/// label · slider · value. Returns (changed, gesture started).
fn slider_row(ui: &mut Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, step: f64, suffix: &str) -> (bool, bool) {
    let mut out = (false, false);
    let id = ui.make_persistent_id(("slider", label));
    if DESK {
        // desktop: compact row, the value is a field you can type into
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.set_height(26.0);
            let total = ui.available_width();
            let (lw, vw) = (74.0, 68.0);
            let (lr, _) = ui.allocate_exact_size(vec2(lw, 22.0), Sense::hover());
            ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, label, regular(12.5), LABEL);
            let sw = (total - lw - vw - 12.0).max(40.0);
            let (min, max) = (*range.start(), *range.end());
            let adaptive = suffix == "mm" && min >= 0.0 && max > 20.0;
            let hint = if adaptive { 10.0 } else { max };
            out = touch_slider(ui, id, v, min, hint.min(max), adaptive, step as f32, sw);
            let r = value_field(ui, v, range.clone(), step, suffix, vw);
            if r.changed() {
                out = (true, out.1 || r.drag_started() || !r.dragged());
            }
            *v = v.clamp(min, max);
        });
        return out;
    }
    ui.horizontal(|ui| {
        ui.set_height(34.0);
        let (lr, _) = ui.allocate_exact_size(vec2(84.0, 30.0), Sense::hover());
        ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, label, regular(13.0), LABEL);
        let sw = (ui.available_width() - 70.0).max(40.0);
        let (min, max) = (*range.start(), *range.end());
        let adaptive = suffix == "mm" && min >= 0.0 && max > 20.0;
        let hint = if adaptive { 10.0 } else { max };
        out = touch_slider(ui, id, v, min, hint.min(max), adaptive, step as f32, sw);
        *v = v.clamp(min, max);
        let (vr, _) = ui.allocate_exact_size(vec2(62.0, 30.0), Sense::hover());
        let txt = if suffix.is_empty() { format!("{}", fmt_mm(*v)) } else { format!("{} {}", fmt_mm(*v), suffix) };
        ui.painter().text(vr.right_center(), Align2::RIGHT_CENTER, txt, regular(13.0), TEXT_STRONG);
    });
    out
}

fn stepper(ui: &mut Ui, label: &str, value: u32, min: u32, max: u32) -> Option<u32> {
    let mut out = None;
    ui.horizontal(|ui| {
        ui.set_height(44.0);
        let (lr, _) = ui.allocate_exact_size(vec2(ui.available_width() - 136.0, 40.0), Sense::hover());
        ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, label, regular(13.0), LABEL);
        let btn = |ui: &mut Ui, t: &str| {
            let (r, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
            ui.painter().rect_filled(r, 10.0, if resp.hovered() { Color32::from_rgb(0x33, 0x37, 0x3E) } else { RAISED });
            ui.painter().text(r.center(), Align2::CENTER_CENTER, t, regular(18.0), TEXT);
            resp.clicked()
        };
        if btn(ui, "−") && value > min {
            out = Some(value - 1);
        }
        let (vr, _) = ui.allocate_exact_size(vec2(36.0, 44.0), Sense::hover());
        ui.painter().text(vr.center(), Align2::CENTER_CENTER, format!("×{value}"), semibold(13.0), TEXT);
        if btn(ui, "+") && value < max {
            out = Some(value + 1);
        }
    });
    out
}

pub(crate) fn toggle_row(ui: &mut Ui, label: &str, on: bool) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        // desktop: a small switch on a 28 pt row; iPad: 44 pt touch target
        let (row, tw, th, kr) = if DESK { (28.0, 34.0, 18.0, 7.0) } else { (44.0, 52.0, 32.0, 13.0) };
        ui.set_height(row);
        let (lr, _) = ui.allocate_exact_size(vec2(ui.available_width() - tw - ui.spacing().item_spacing.x, row - 4.0), Sense::hover());
        ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, label, regular(if DESK { 12.5 } else { 13.0 }), LABEL);
        let (r, resp) = ui.allocate_exact_size(vec2(tw, th), Sense::click());
        let p = ui.painter();
        p.rect_filled(r, th * 0.5, if on { ACCENT } else { RAISED });
        let knob = if on { r.right_center() - vec2(th * 0.5, 0.0) } else { r.left_center() + vec2(th * 0.5, 0.0) };
        p.circle_filled(knob, kr, if on { ON_ACCENT } else { TEXT2 });
        clicked = resp.clicked();
    });
    clicked
}

pub(crate) fn divider(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, PANEL_BORDER);
}

// ------------------------------------------------------------------ layout

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    st.screen = [screen.width(), screen.height()];
    // free view: between the rail and the right column, above the history strip
    let right = if st.panel_visible { screen.width() - 308.0 } else { screen.width() - 12.0 };
    // the Bake panel floats over the view: it does not move the camera (Romain)
    st.set_view_rect([84.0, 72.0, right, screen.height() - 84.0]);
    top_bar(ctx, st, screen);
    if st.bake_mode() {
        // BAKE: no tools, no properties, no history to edit; the Bake panel only
        hud(ctx, st);
        bake_side_panel(ctx, st, screen);
        dialogs(ctx, st, screen);
        shortcuts(ctx, st);
        return;
    }
    rail(ctx, st);
    history_strip(ctx, st, screen);
    if cfg!(target_os = "ios") {
        // touch only: a keyboard has Alt and Ctrl
        thumb_button(ctx, st, screen);
        ctrl_button(ctx, st, screen);
    }
    hud(ctx, st);
    right_column(ctx, st, screen);
    dialogs(ctx, st, screen);
    shortcuts(ctx, st);
}

/// Toast and modal dialogs (both interfaces).
pub(crate) fn dialogs(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    toast(ctx, st, screen);
    if st.export_open {
        export_dialog(ctx, st, screen);
    }
    if st.bake.job.is_some() {
        ctx.request_repaint();
    }
    if st.files_open {
        files_dialog(ctx, st, screen);
    }
}

pub(crate) fn shortcuts(ctx: &egui::Context, st: &mut AppState) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let (undo, redo, del, dup, esc, enter, alt, f, save) = ctx.input(|i| {
        let cmd = i.modifiers.command;
        (
            cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
            cmd && (i.key_pressed(egui::Key::Y) || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
            i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
            cmd && i.key_pressed(egui::Key::D),
            i.key_pressed(egui::Key::Escape),
            i.key_pressed(egui::Key::Enter),
            i.modifiers.alt,
            i.key_pressed(egui::Key::F) && !cmd,
            cmd && i.key_pressed(egui::Key::S),
        )
    });
    if save {
        st.save();
    }
    if st.bake_mode() {
        // BAKE: nothing edits the model; Esc goes back to BUILD, F frames
        if esc && st.display_open {
            st.display_open = false;
        } else if esc && st.settings_open {
            st.settings_open = false;
        } else if esc {
            st.set_bake_mode(false);
        }
        if f {
            st.frame_selection();
        }
        let _ = (undo, redo, del, dup, enter, alt);
        st.key_ctrl = false;
        return;
    }
    if undo {
        st.undo();
    }
    if redo {
        st.redo();
    }
    if del {
        if let Some(e) = st.edge_sel.last().cloned() {
            // on edges, Delete removes their fillet (the shapes stay)
            st.set_fillet(0.0, e.chamfer, true);
        } else {
            st.delete_selected();
        }
    }
    if dup {
        st.duplicate_selected();
    }
    if esc && st.display_open {
        st.display_open = false;
    } else if esc && st.settings_open {
        st.settings_open = false;
    } else if esc {
        if !st.edge_sel.is_empty() {
            st.clear_edge();
        }
        st.commit_placement();
        st.tool = Tool::Select;
    }
    if enter {
        st.commit_placement();
    }
    if f {
        st.frame_selection();
    }
    let (k1, k2) = ctx.input(|i| (!i.modifiers.command && i.key_pressed(egui::Key::Num1), !i.modifiers.command && i.key_pressed(egui::Key::Num2)));
    if k1 {
        st.set_pick_mode(PickMode::Object);
    }
    // tools: Q selection, R cube, C cylinder (never while typing: guarded above)
    let (kq, kr, kc) = ctx.input(|i| {
        let free = !i.modifiers.command && !i.modifiers.alt;
        (free && i.key_pressed(egui::Key::Q), free && i.key_pressed(egui::Key::R), free && i.key_pressed(egui::Key::C))
    });
    if kq {
        // Q = Select tool in object mode (also leaves the edge mode, like 1)
        st.set_pick_mode(PickMode::Object);
    }
    for (pressed, k) in [(kr, PrimKind::Box), (kc, PrimKind::Cylinder)] {
        if pressed {
            st.commit_placement();
            st.tool = Tool::Place(k);
        }
    }
    if k2 {
        st.set_pick_mode(PickMode::Edge);
    }
    // Alt is the Maya navigation (viewport.rs); Ctrl / Cmd = the CTRL button
    let _ = alt;
    st.key_ctrl = ctx.input(|i| i.modifiers.ctrl || i.modifiers.mac_cmd);
}

fn top_bar(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let bar = Rect::from_min_size(screen.min + vec2(12.0, 12.0), vec2(screen.width() - 24.0, 56.0));
    egui::Area::new(Id::new("topbar-bg")).order(Order::Middle).fixed_pos(bar.min).interactable(false).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(bar.size(), Sense::hover());
        ui.painter().rect(r, 16.0, PANEL, Stroke::new(1.0, PANEL_BORDER), StrokeKind::Inside);
    });
    egui::Area::new(Id::new("topbar-left")).order(Order::Foreground).fixed_pos(bar.min + vec2(12.0, 6.0)).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (r, _) = ui.allocate_exact_size(vec2(30.0, 44.0), Sense::hover());
            let logo = Rect::from_center_size(r.center(), vec2(30.0, 30.0));
            ui.painter().rect_filled(logo, 9.0, ACCENT);
            icons::paint(ui.painter(), Rect::from_center_size(logo.center(), vec2(18.0, 18.0)), Icon::Cube, ON_ACCENT);
            ui.label(egui::RichText::new("Forme").font(semibold(16.0)).color(TEXT));
            let (sep, _) = ui.allocate_exact_size(vec2(1.0, 24.0), Sense::hover());
            ui.painter().rect_filled(sep, 0.0, DIVIDER);
            let name = st.doc_name.clone();
            let g = ui.painter().layout_no_wrap(name.clone(), medium(14.0), TEXT);
            let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 34.0, 44.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, 10.0, RAISED);
            }
            ui.painter().galley(pos2(r.min.x + 8.0, r.center().y - g.size().y * 0.5), g, TEXT);
            icons::paint(ui.painter(), Rect::from_min_size(pos2(r.max.x - 22.0, r.center().y - 7.0), vec2(14.0, 14.0)), Icon::Chevron, TEXT2);
            if resp.clicked() {
                st.files_open = !st.files_open;
            }
            ui.add_space(6.0);
            if !st.bake_mode() {
                if icon_btn(ui, Icon::Undo, false, "Annuler (tap à 2 doigts)", 44.0).clicked() {
                    st.undo();
                }
                if icon_btn(ui, Icon::Redo, false, "Rétablir (tap à 3 doigts)", 44.0).clicked() {
                    st.redo();
                }
            }
            let r = icon_btn(ui, Icon::Light, st.display_open, "Affichage : lumière, ombres, anticrénelage", 44.0);
            light_toggle(st, &r);
            if icon_btn(ui, Icon::Settings, st.settings_open, "Réglages (interface Atelier ou Studio)", 44.0).clicked() {
                st.settings_open = !st.settings_open;
            }
        });
    });
    egui::Area::new(Id::new("topbar-right"))
        .order(Order::Foreground)
        .anchor(Align2::RIGHT_TOP, vec2(-20.0, 18.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                // view presets
                let cur = st.camera.preset_of();
                let presets = [
                    (ViewPreset::Perspective, "3D"),
                    (ViewPreset::Front, "Face"),
                    (ViewPreset::Right, "Droite"),
                    (ViewPreset::Top, "Dessus"),
                ];
                let (well, _) = ui.allocate_exact_size(vec2(4.0 * 58.0 + 6.0, 44.0), Sense::hover());
                ui.painter().rect_filled(well, 12.0, WELL);
                for (i, (p, l)) in presets.iter().enumerate() {
                    let r = Rect::from_min_size(well.min + vec2(3.0 + i as f32 * 58.0, 3.0), vec2(58.0, 38.0));
                    let resp = ui.interact(r, Id::new(("preset", i)), Sense::click());
                    let on = cur == *p;
                    if on {
                        ui.painter().rect_filled(r, 9.0, Color32::from_rgb(0x2D, 0x30, 0x36));
                    }
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, *l, if on { semibold(13.0) } else { medium(13.0) }, if on { TEXT_STRONG } else { Color32::from_rgb(0xAE, 0xB3, 0xBA) });
                    if resp.clicked() {
                        st.camera.preset(*p);
                        st.view_changed();
                    }
                }
                ui.add_space(4.0);
                if !st.bake_mode() {
                    if toggle_chip(ui, Icon::Mirror, "Sym. X", st.sym_x).clicked() {
                        st.sym_x = !st.sym_x;
                        st.toast(if st.sym_x { "Symétrie X : les nouvelles formes sont doublées" } else { "Symétrie X désactivée" });
                    }
                    if toggle_chip(ui, Icon::Magnet, "Snap", st.snap).clicked() {
                        st.snap = !st.snap;
                    }
                }
                if toggle_chip(ui, Icon::Grid, "Grille", st.grid).clicked() {
                    st.grid = !st.grid;
                    st.view_changed();
                }
                let (sep, _) = ui.allocate_exact_size(vec2(9.0, 24.0), Sense::hover());
                ui.painter().rect_filled(Rect::from_center_size(sep.center(), vec2(1.0, 24.0)), 0.0, DIVIDER);
                // the Bake is set aside (Romain, 10 nov.): pure SDF modelling
                if primary_btn(ui, Some(Icon::Export), "Exporter", None).clicked() {
                    st.export_open = !st.export_open;
                }
            });
        });
}

/// BUILD | BAKE switch (both interfaces). BAKE freezes the model: only the
/// camera, the diagnostics, the options and the Bake.
#[allow(dead_code)] // the Bake is set aside for now
pub(crate) fn mode_switch(ui: &mut Ui, st: &mut AppState, height: f32) {
    let baking = st.bake.job.is_some();
    let items = [(false, "BUILD"), (true, if baking { "BAKE…" } else { "BAKE" })];
    let w = 66.0;
    let (well, _) = ui.allocate_exact_size(vec2(2.0 * w + 6.0, height), Sense::hover());
    ui.painter().rect_filled(well, 10.0, WELL);
    let cur = st.bake_mode();
    for (i, (bake, label)) in items.iter().enumerate() {
        let r = Rect::from_min_size(well.min + vec2(3.0 + i as f32 * w, 3.0), vec2(w, height - 6.0));
        let resp = ui.interact(r, Id::new(("mode-switch", i)), Sense::click());
        let on = cur == *bake;
        let c = if *bake { CARVE } else { ACCENT };
        let p = ui.painter();
        if on {
            p.rect(r, 8.0, tint(c, 0.18), Stroke::new(1.5, c), StrokeKind::Inside);
        } else if resp.hovered() {
            p.rect_filled(r, 8.0, RAISED);
        }
        p.text(r.center(), Align2::CENTER_CENTER, *label, semibold(12.5), if on { TEXT_STRONG } else { TEXT2 });
        let tip = if *bake { "Mode BAKE : modèle figé, diagnostic et Bake (Échap pour revenir)" } else { "Mode BUILD : modélisation" };
        if resp.on_hover_text(tip).clicked() {
            st.set_bake_mode(*bake);
        }
    }
}

/// Keyboard shortcut of a primitive tool (Q select, R cube, C cylinder).
pub(crate) fn prim_key(k: PrimKind) -> Option<&'static str> {
    match k {
        PrimKind::Box => Some("R"),
        PrimKind::Cylinder => Some("C"),
        _ => None,
    }
}

/// The shortcut letter, small, in the bottom-right corner of a tool button.
pub(crate) fn key_hint(ui: &Ui, r: &Response, key: &str) {
    if !DESK {
        return;
    }
    let at = r.rect.right_bottom() - vec2(4.0, 3.0);
    ui.painter().text(at, Align2::RIGHT_BOTTOM, key, medium(9.0), Color32::from_rgb(0x8A, 0x8F, 0x97));
}

/// Small notched button under the primitives: opens the creation popover.
pub(crate) fn creation_btn(ui: &mut Ui, st: &mut AppState, width: f32, below: bool) {
    let (r, resp) = ui.allocate_exact_size(vec2(width, 14.0), Sense::click());
    let p = ui.painter();
    let on = st.creation_open;
    let c = if on { ACCENT } else if resp.hovered() { TEXT } else { Color32::from_rgb(0x7C, 0x81, 0x89) };
    // a notch: three small dots on a short bar
    let mid = r.center();
    p.rect_filled(Rect::from_center_size(mid, vec2(22.0, 10.0)), 5.0, if on { ACCENT_BG } else if resp.hovered() { RAISED } else { Color32::TRANSPARENT });
    for i in -1..=1 {
        p.circle_filled(mid + vec2(i as f32 * 5.0, 0.0), 1.4, c);
    }
    let resp = resp.on_hover_text("Options de création");
    if resp.clicked() {
        st.creation_open = !st.creation_open;
        st.creation_anchor = if below { [r.left() - 20.0, r.bottom() + 14.0] } else { [r.right() + 10.0, r.top() - 6.0] };
    }
}

/// Ephemeral popover with the creation settings (closes on a click outside).
pub fn creation_popover(ctx: &egui::Context, st: &mut AppState) {
    if !st.creation_open {
        return;
    }
    let at = pos2(st.creation_anchor[0], st.creation_anchor[1]);
    let resp = egui::Area::new(Id::new("creation-popover")).order(Order::Foreground).fade_in(false).fixed_pos(at).show(ctx, |ui| {
        panel().inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(250.0);
            ui.label(egui::RichText::new("Création").font(semibold(13.0)).color(TEXT));
            use crate::settings::Creation;
            let items = [(Creation::Interactive, "Interactive", ACCENT), (Creation::Direct, "Directe", ACCENT)];
            if let Some(c) = segmented(ui, &items, st.settings.creation, 30.0) {
                st.settings.creation = c;
                st.save_settings();
            }
            let blurb = match st.settings.creation {
                Creation::Interactive => "Glisse la base, relâche, puis la hauteur : vers le haut +, vers le bas −. Ctrl inverse.",
                Creation::Direct => "Un clic pose la forme à sa taille par défaut sur la face ou la grille sous le curseur ; ensuite ses poignées la dimensionnent.",
            };
            ui.add(egui::Label::new(egui::RichText::new(blurb).font(regular(11.5)).color(TEXT2)).wrap());
        });
    });
    if ctx.input(|i| i.pointer.any_pressed()) {
        if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
            let button = Rect::from_center_size(pos2(st.creation_anchor[0], st.creation_anchor[1]), vec2(140.0, 60.0));
            if !resp.response.rect.contains(p) && !button.contains(p) {
                st.creation_open = false;
            }
        }
    }
}

fn rail(ctx: &egui::Context, st: &mut AppState) {
    egui::Area::new(Id::new("rail")).order(Order::Middle).fixed_pos(pos2(12.0, 80.0)).show(ctx, |ui| {
        panel().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.set_width(44.0);
            // selection, like 3ds Max: object (1) or edges (2)
            let sel = st.tool == Tool::Select;
            let r = icon_btn(ui, Icon::Select, sel && st.pick_mode == PickMode::Object, "Sélection d'objets (Q ou 1)", 44.0);
            key_hint(ui, &r, "Q");
            if r.clicked() {
                st.set_pick_mode(PickMode::Object);
            }
            let r = icon_btn(ui, Icon::Fillet, sel && st.pick_mode == PickMode::Edge, "Sélection d'arêtes (2) : fillets, chanfreins", 44.0);
            key_hint(ui, &r, "2");
            if r.clicked() {
                st.set_pick_mode(PickMode::Edge);
            }
            divider(ui);
            for k in PRIMS {
                let tip = match prim_key(k) {
                    Some(key) => format!("{} ({key})", k.label()),
                    None => k.label().to_string(),
                };
                let r = icon_btn(ui, prim_icon(k), st.tool == Tool::Place(k), &tip, 44.0);
                if let Some(key) = prim_key(k) {
                    key_hint(ui, &r, key);
                }
                if r.clicked() {
                    st.commit_placement();
                    st.tool = Tool::Place(k);
                }
            }
            if DESK {
                creation_btn(ui, st, 44.0, false);
            }
            divider(ui);
            // mode of the next shapes (and of the selected one)
            let shown = shown_mode(st);
            for m in MODES {
                if mode_btn(ui, m, shown == m).clicked() {
                    st.set_mode(m);
                }
            }
        });
    });
}

/// Mode shown as current: while placing, the placement mode; with the Select
/// tool, the selected shape's mode.
fn shown_mode(st: &AppState) -> CombineMode {
    let current = st.effective_mode();
    let target = if st.tool == Tool::Select {
        st.selection.and_then(|id| st.scene.node(id)).filter(|n| n.shape().is_some()).map(|n| n.combine.mode)
    } else {
        None
    };
    if st.carve_held {
        current
    } else {
        target.unwrap_or(current)
    }
}

fn mode_btn(ui: &mut Ui, m: CombineMode, on: bool) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(44.0, 38.0), Sense::click());
    let c = mode_color(m);
    let p = ui.painter();
    if on {
        p.rect(r.shrink(0.75), 10.0, tint(c, 0.16), Stroke::new(1.5, c), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(r, 10.0, RAISED);
    }
    paint_mode_glyph(p, r.center(), m, if on { c } else { TEXT_ICON }, 14.0);
    let tip = if m == CombineMode::Add { "Add : ajoute la forme" } else { "Carve : creuse" };
    resp.on_hover_text(tip)
}

/// The operations in evaluation order, as a strip of chips: the history the
/// Bake replays. A chip selects its operation.
fn history_strip(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let rows = st.scene.rows();
    let cx = st.principal()[0];
    let max_w = (st.view_rect[2] - st.view_rect[0] - 24.0).max(320.0);
    egui::Area::new(Id::new("history")).order(Order::Middle).pivot(Align2::CENTER_BOTTOM).fixed_pos(pos2(cx, screen.max.y - 14.0)).show(ctx, |ui| {
        panel().inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(egui::RichText::new("Historique").font(semibold(13.0)).color(TEXT));
                    let n = rows.len();
                    let sub = match n {
                        0 => "vide".to_string(),
                        1 => "1 opération".to_string(),
                        _ => format!("{n} opérations"),
                    };
                    ui.label(egui::RichText::new(sub).font(regular(11.5)).color(TEXT2));
                });
                let (sep, _) = ui.allocate_exact_size(vec2(1.0, 36.0), Sense::hover());
                ui.painter().rect_filled(sep, 0.0, DIVIDER);
                if rows.is_empty() {
                    ui.label(egui::RichText::new("Choisis Cube ou Cylindre à gauche, puis glisse dans la vue").font(regular(13.0)).color(TEXT2));
                    return;
                }
                egui::ScrollArea::horizontal().id_salt("history-scroll").max_width(max_w - 220.0).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for (i, (id, depth)) in rows.iter().enumerate() {
                            if i > 0 {
                                let (lr, _) = ui.allocate_exact_size(vec2(12.0, 44.0), Sense::hover());
                                ui.painter().rect_filled(Rect::from_center_size(lr.center(), vec2(10.0, 2.0)), 1.0, DIVIDER);
                            }
                            if history_chip(ui, st, *id, *depth).clicked() {
                                st.commit_placement();
                                st.tool = Tool::Select;
                                st.select(Some(*id));
                            }
                        }
                    });
                });
                let (er, _) = ui.allocate_exact_size(vec2(40.0, 44.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(Rect::from_center_size(pos2(er.min.x + 4.0, er.center().y), vec2(3.0, 30.0)), 2.0, ACCENT);
                p.text(pos2(er.min.x + 12.0, er.center().y), Align2::LEFT_CENTER, "Fin", regular(12.0), TEXT2);
            });
        });
    });
}

fn history_chip(ui: &mut Ui, st: &AppState, id: NodeId, depth: usize) -> Response {
    let Some(n) = st.scene.node(id) else {
        return ui.allocate_exact_size(vec2(0.0, 0.0), Sense::hover()).1;
    };
    let title = n.name.clone();
    let mut sub = if n.shape().is_some() { format!("{} · {}", n.type_label(), n.combine.mode.label()) } else { n.type_label() };
    if n.mirror.any() {
        sub += " · sym.";
    }
    if n.repeat.count() > 1 {
        sub += &format!(" ×{}", n.repeat.count());
    }
    let tg = ui.painter().layout_no_wrap(title, semibold(12.5), TEXT);
    let sg = ui.painter().layout_no_wrap(sub, regular(11.0), TEXT2);
    let w = 8.0 + depth.saturating_sub(1) as f32 * 6.0 + 28.0 + 8.0 + tg.size().x.max(sg.size().x) + 12.0;
    let (r, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
    let selected = st.is_selected(id);
    let p = ui.painter();
    if selected {
        p.rect(r, 10.0, ACCENT_BG, Stroke::new(1.0, ACCENT_BORDER), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(r, 10.0, RAISED);
    }
    let dim = if n.visible { 1.0 } else { 0.45 };
    let x0 = r.min.x + 8.0 + depth.saturating_sub(1) as f32 * 6.0;
    let ir = Rect::from_min_size(pos2(x0, r.center().y - 13.0), vec2(26.0, 26.0));
    p.rect_filled(ir, 7.0, RAISED);
    let icon = match n.shape() {
        Some(sh) => prim_icon(sh.kind),
        None => Icon::Folder,
    };
    icons::paint(p, ir.shrink(5.0), icon, tint(TEXT_ICON, dim));
    if n.shape().is_some() {
        let c = mode_color(n.combine.mode);
        p.circle_filled(ir.right_bottom() - vec2(1.0, 1.0), 5.5, PANEL);
        p.circle_filled(ir.right_bottom() - vec2(1.0, 1.0), 4.0, c);
    }
    let tx = ir.max.x + 8.0;
    p.galley(pos2(tx, r.center().y - tg.size().y - 0.5), tg, if selected { ACCENT_TEXT } else { tint(TEXT, dim) });
    p.galley(pos2(tx, r.center().y + 1.5), sg, TEXT2);
    resp.on_hover_text("Sélectionner cette opération")
}

/// CTRL button area (points), just above the thumb button. Held, taps add to
/// the selection (edges with the Select tool).
pub fn ctrl_rect(screen_w: f32, screen_h: f32) -> Rect {
    // next to the thumb button, clear of the tool column
    let t = thumb_rect(screen_w, screen_h);
    Rect::from_min_size(t.min + vec2(t.width() + 10.0, 0.0), vec2(84.0, 84.0))
}

fn ctrl_button(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let cr = ctrl_rect(screen.width(), screen.height());
    egui::Area::new(Id::new("ctrl")).order(Order::Middle).fixed_pos(cr.min).show(ctx, |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::click_and_drag());
        if resp.is_pointer_button_down_on() {
            st.ctrl_held = true;
        } else if resp.drag_stopped() || resp.clicked() {
            st.ctrl_held = false;
        }
        let held = st.multi();
        let p = ui.painter();
        let c = r.center();
        let sub = if st.tool == Tool::Select { "+ sélection" } else { "combiner" };
        if held {
            p.circle_filled(c, 50.0, tint(EDGE_SEL, 0.22));
            p.circle(c, 42.0, EDGE_SEL, Stroke::new(2.0, Color32::from_rgb(0xFF, 0xE4, 0xA6)));
            let dark = Color32::from_rgb(0x2A, 0x20, 0x08);
            p.text(c + vec2(0.0, -6.0), Align2::CENTER_CENTER, "CTRL", semibold(19.0), dark);
            p.text(c + vec2(0.0, 15.0), Align2::CENTER_CENTER, sub, semibold(11.0), dark);
        } else {
            p.circle(c, 42.0, Color32::from_rgb(0x24, 0x23, 0x1E), Stroke::new(2.0, EDGE_SEL_DIM.gamma_multiply(1.6)));
            p.text(c + vec2(0.0, -6.0), Align2::CENTER_CENTER, "CTRL", semibold(19.0), EDGE_SEL);
            p.text(c + vec2(0.0, 15.0), Align2::CENTER_CENTER, sub, regular(11.0), Color32::from_rgb(0xB9, 0xB2, 0xA0));
        }
    });
}

/// Thumb button area (points). On iPad the platform layer handles this touch
/// itself so that the Pencil, as the second touch, still drives the viewport.
pub fn thumb_rect(screen_w: f32, screen_h: f32) -> Rect {
    let _ = screen_w;
    Rect::from_min_size(pos2(12.0, screen_h - 102.0), vec2(84.0, 84.0))
}

fn thumb_button(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let tr = thumb_rect(screen.width(), screen.height());
    egui::Area::new(Id::new("thumb")).order(Order::Middle).fixed_pos(tr.min).show(ctx, |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::click_and_drag());
        if resp.is_pointer_button_down_on() {
            st.carve_held = true;
        } else if resp.drag_stopped() || resp.clicked() {
            st.carve_held = false;
        }
        let held = st.carve_held;
        let p = ui.painter();
        let c = r.center();
        if held {
            p.circle_filled(c, 50.0, tint(CARVE, 0.22));
            p.circle(c, 42.0, CARVE, Stroke::new(2.0, Color32::from_rgb(0xFF, 0xC7, 0x9C)));
            let dark = Color32::from_rgb(0x2A, 0x16, 0x08);
            p.text(c + vec2(0.0, -16.0), Align2::CENTER_CENTER, "−", semibold(22.0), dark);
            p.text(c + vec2(0.0, 4.0), Align2::CENTER_CENTER, "Carve", semibold(13.0), dark);
            p.text(c + vec2(0.0, 20.0), Align2::CENTER_CENTER, "maintenu", semibold(11.0), dark);
        } else {
            p.circle(c, 42.0, Color32::from_rgb(0x26, 0x23, 0x1F), Stroke::new(2.0, CARVE));
            p.text(c + vec2(0.0, -16.0), Align2::CENTER_CENTER, "−", semibold(22.0), CARVE_LIGHT);
            p.text(c + vec2(0.0, 4.0), Align2::CENTER_CENTER, "Carve", semibold(13.0), CARVE_LIGHT);
            p.text(c + vec2(0.0, 20.0), Align2::CENTER_CENTER, "maintenir", regular(11.0), Color32::from_rgb(0xB9, 0xAF, 0xA6));
        }
    });
}

pub(crate) fn hud(ctx: &egui::Context, st: &mut AppState) {
    let cx = st.principal()[0];
    let text = st.hint();
    let color = match &st.placement {
        Some(p) => mode_color(p.mode),
        None => match st.tool {
            Tool::Place(_) => mode_color(st.effective_mode()),
            _ => st.selection.and_then(|id| st.scene.node(id)).map(|n| mode_color(n.combine.mode)).unwrap_or(TEXT2),
        },
    };
    egui::Area::new(Id::new("hud")).order(Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(pos2(cx, 82.0)).interactable(false).show(ctx, |ui| {
        let g = ui.painter().layout_no_wrap(text, regular(13.0), LABEL);
        let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 44.0, 32.0), Sense::hover());
        let p = ui.painter();
        p.rect(r, 16.0, tint(PANEL, 0.92), Stroke::new(1.0, PANEL_BORDER), StrokeKind::Inside);
        p.circle_filled(pos2(r.min.x + 18.0, r.center().y), 4.0, color);
        p.galley(pos2(r.min.x + 30.0, r.center().y - g.size().y * 0.5), g, LABEL);
    });
}

pub(crate) fn toast(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let Some(t) = &st.toast else { return };
    let text = t.text.clone();
    let warn = t.warn;
    let cx = st.principal()[0];
    egui::Area::new(Id::new("toast")).order(Order::Tooltip).pivot(Align2::CENTER_BOTTOM).fixed_pos(pos2(cx, screen.max.y - 92.0)).interactable(false).show(ctx, |ui| {
        let g = ui.painter().layout_no_wrap(text, medium(13.0), TEXT);
        let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 32.0, 36.0), Sense::hover());
        ui.painter().rect(r, 18.0, Color32::from_rgb(0x2A, 0x2D, 0x33), Stroke::new(1.0, if warn { CARVE } else { ACCENT_BORDER }), StrokeKind::Inside);
        ui.painter().galley(pos2(r.min.x + 16.0, r.center().y - g.size().y * 0.5), g, TEXT);
    });
}

// ------------------------------------------------------------ right column

fn right_column(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let x = screen.max.x - 12.0 - RIGHT_W;
    let show_tree = st.show_tree;
    let tree_h = if show_tree { 268.0 } else { 0.0 };
    let props_max = screen.height() - 80.0 - 12.0 - if show_tree { tree_h + 12.0 } else { 0.0 };
    egui::Area::new(Id::new("props")).order(Order::Middle).fixed_pos(pos2(x, 80.0)).show(ctx, |ui| {
        panel().inner_margin(Margin::same(14)).show(ui, |ui| {
            ui.set_width(RIGHT_W - 28.0);
            egui::ScrollArea::vertical().id_salt("props-scroll").max_height(props_max - 28.0).min_scrolled_height(props_max - 28.0).auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width(RIGHT_W - 28.0);
                if st.selection.and_then(|id| st.scene.node(id)).is_some() {
                    properties(ui, st);
                } else {
                    scene_panel(ui, st);
                }
            });
        });
    });
    if show_tree {
        egui::Area::new(Id::new("tree")).order(Order::Middle).fixed_pos(pos2(x, screen.max.y - 12.0 - tree_h)).show(ctx, |ui| {
            panel().inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                ui.set_width(RIGHT_W - 20.0);
                ui.set_height(tree_h - 16.0);
                tree(ui, st, tree_h - 16.0);
            });
        });
    }
}

fn properties(ui: &mut Ui, st: &mut AppState) {
    let id = st.selection.unwrap();
    let node = st.scene.node(id).cloned().unwrap();

    // header
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(egui::RichText::new("Sélection").font(regular(12.0)).color(TEXT2));
            ui.horizontal(|ui| {
                if st.renaming {
                    let mut name = node.name.clone();
                    let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(120.0).font(semibold(15.0)));
                    if r.changed() {
                        st.edit("Renommer", id, |n| n.name = name.clone());
                    }
                    if r.lost_focus() {
                        st.renaming = false;
                    }
                } else {
                    let r = ui.add(egui::Label::new(egui::RichText::new(&node.name).font(semibold(if DESK { 15.0 } else { 17.0 })).color(TEXT)).sense(Sense::click()).truncate());
                    if r.double_clicked() {
                        st.renaming = true;
                    }
                }
                chip_label(ui, &node.type_label(), RAISED, LABEL);
                if node.shape().is_some() || matches!(node.kind, NodeKind::Group { combined: true, .. }) {
                    let c = mode_color(node.combine.mode);
                    chip_label(ui, node.combine.mode.label(), tint(c, 0.16), c);
                }
            });
        });
    });
    let ib = if DESK { 30.0 } else { 44.0 };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if icon_btn(ui, Icon::Duplicate, false, "Dupliquer", ib).clicked() {
            st.duplicate_selected();
        }
        if icon_btn(ui, if node.visible { Icon::Eye } else { Icon::EyeOff }, false, "Masquer", ib).clicked() {
            st.toggle_visible(id);
        }
        if icon_btn(ui, Icon::Frame, false, "Cadrer", ib).clicked() {
            st.frame_selection();
        }
        if icon_btn(ui, Icon::Trash, false, "Supprimer", ib).clicked() {
            st.delete_selected();
            return;
        }
    });
    if st.selection.is_none() {
        return;
    }
    divider(ui);
    if !st.edge_sel.is_empty() {
        edge_card(ui, st);
        divider(ui);
    }

    if let Some(sh) = node.shape().cloned() {
        section(ui, "Forme");
        shape_rows(ui, st, id, &sh);
    } else if let NodeKind::Group { combined, .. } = node.kind {
        section(ui, "Groupe");
        if toggle_row(ui, "Fusionner les enfants", combined) {
            st.edit("Groupe", id, |n| {
                if let NodeKind::Group { combined, .. } = &mut n.kind {
                    *combined = !*combined;
                }
            });
        }
        let mut s = node.transform.scale;
        let (ch, start) = slider_row(ui, "Échelle", &mut s, 0.2..=4.0, 0.05, "×");
        if ch {
            st.edit_live("Échelle", id, start, |n| n.transform.scale = s);
        }
    }

    if node.shape().is_some() || matches!(node.kind, NodeKind::Group { combined: true, .. }) {
        section(ui, "Combinaison");
        let items = MODES.map(|m| (m, m.label(), mode_color(m)));
        if let Some(m) = segmented(ui, &items, node.combine.mode, 40.0) {
            st.mode = m;
            st.edit("Mode", id, |n| n.combine.mode = m);
        }
        let mut b = node.combine.blend;
        let (ch, start) = slider_row(ui, "Blend", &mut b, 0.0..=12.0, 0.1, "mm");
        if ch {
            st.edit_live("Blend", id, start, |n| n.combine.blend = b);
        }
        if node.combine.blend > 0.0 {
            let kinds = [(BlendKind::Smooth, "Lisse", ACCENT), (BlendKind::Chamfer, "Chanfrein", ACCENT)];
            if let Some(k) = segmented(ui, &kinds, node.combine.kind, 36.0) {
                st.edit("Type de blend", id, |n| n.combine.kind = k);
            }
        }
    }

    symmetry_row(ui, st, id, &node);
    position_rows(ui, st, id, &node);

    section(ui, "Matière");
    material_palette(ui, st, Some(id), node.material);
}

fn symmetry_row(ui: &mut Ui, st: &mut AppState, id: NodeId, node: &Node) {
    ui.horizontal(|ui| {
        let (row, bw, bh) = if DESK { (28.0, 40.0, 24.0) } else { (40.0, 52.0, 36.0) };
        ui.set_height(row);
        let (lr, _) = ui.allocate_exact_size(vec2(if DESK { 74.0 } else { 84.0 }, bh), Sense::hover());
        ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, "Symétrie", regular(if DESK { 12.5 } else { 13.0 }), LABEL);
        let m = node.mirror;
        let items = [(0, "X", m.x), (1, "Y", m.y), (2, "Z", m.z)];
        for (i, l, on) in items {
            let (r, resp) = ui.allocate_exact_size(vec2(bw, bh), Sense::click());
            let p = ui.painter();
            p.rect(r, 8.0, if on { ACCENT_BG } else { WELL }, Stroke::new(1.0, if on { ACCENT } else { DIVIDER }), StrokeKind::Inside);
            p.text(r.center(), Align2::CENTER_CENTER, l, semibold(12.5), if on { ACCENT_TEXT } else { TEXT2 });
            if resp.clicked() {
                st.edit("Symétrie", id, |n| match i {
                    0 => n.mirror.x = !n.mirror.x,
                    1 => n.mirror.y = !n.mirror.y,
                    _ => n.mirror.z = !n.mirror.z,
                });
            }
        }
    });
}

fn position_rows(ui: &mut Ui, st: &mut AppState, id: NodeId, node: &Node) {
    section(ui, "Position et rotation");
    let mut pos = node.transform.pos;
    let (ex, ey, ez) = node.transform.rot.to_euler(EulerRot::XYZ);
    let mut rot = Vec3::new(ex.to_degrees(), ey.to_degrees(), ez.to_degrees());
    let mut changed_pos = false;
    let mut changed_rot = false;
    // three fixed-width fields per row: the panel never changes width
    let field_w = ((ui.available_width() - 3.0 * 22.0 - 5.0 * 4.0) / 3.0).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, (l, c)) in [("X", AXIS_X), ("Y", AXIS_Y), ("Z", AXIS_Z)].iter().enumerate() {
            let (lr, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
            ui.painter().text(lr.center(), Align2::CENTER_CENTER, *l, semibold(11.5), *c);
            changed_pos |= value_field(ui, &mut pos[i], -10000.0..=10000.0, 0.25, "", field_w).changed();
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, (l, c)) in [("rX", AXIS_X), ("rY", AXIS_Y), ("rZ", AXIS_Z)].iter().enumerate() {
            let (lr, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
            ui.painter().text(lr.center(), Align2::CENTER_CENTER, *l, semibold(11.5), *c);
            changed_rot |= value_field(ui, &mut rot[i], -360.0..=360.0, 1.0, "°", field_w).changed();
        }
    });
    if changed_pos {
        st.edit("Position", id, |n| n.transform.pos = pos);
    }
    if changed_rot {
        let q = Quat::from_euler(EulerRot::XYZ, rot.x.to_radians(), rot.y.to_radians(), rot.z.to_radians());
        st.edit("Rotation", id, |n| n.transform.rot = q);
    }
}

/// The selected edge: fillet size and kind, straight on the parameter that
/// makes it (blend of a junction, rounding of a shape's edges).
fn edge_card(ui: &mut Ui, st: &mut AppState) {
    let Some(e) = st.edge_sel.last().cloned() else { return };
    let n = st.edge_sel.len();
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 5.0, EDGE_SEL);
        let title = if n > 1 { format!("{n} arêtes sélectionnées") } else { "Arête sélectionnée".to_string() };
        ui.label(egui::RichText::new(title).font(semibold(14.0)).color(TEXT));
    });
    let what = if n > 1 {
        let mixed = st.edge_sel.iter().any(|x| (x.radius - e.radius).abs() > 1e-4 || x.chamfer != e.chamfer);
        format!("Même réglage pour toutes{} · CTRL + tap pour en ajouter ou retirer", if mixed { " (valeurs différentes)" } else { "" })
    } else {
        match e.kind {
            forme_core::edges::EdgeKind::Junction { .. } => format!("{} · jonction entre deux formes", e.label),
            forme_core::edges::EdgeKind::Own { .. } => format!("{} · maintiens CTRL pour en ajouter", e.label),
        }
    };
    ui.label(egui::RichText::new(what).font(regular(12.0)).color(TEXT2));
    ui.add_space(4.0);
    let kinds = [(false, "Arrondi", EDGE_SEL), (true, "Chanfrein", EDGE_SEL)];
    if let Some(c) = segmented(ui, &kinds, e.chamfer, 36.0) {
        st.set_fillet(e.radius, c, true);
    }
    let max = st.fillet_max().max(0.1);
    let mut r = e.radius;
    let label = if e.chamfer { "Chanfrein" } else { "Fillet" };
    let (changed, start) = slider_row(ui, label, &mut r, 0.0..=max, 0.1, "mm");
    if changed {
        st.set_fillet(r, e.chamfer, start);
    }
    ui.horizontal(|ui| {
        if secondary_btn(ui, "Arête vive", 120.0).clicked() {
            st.set_fillet(0.0, e.chamfer, true);
        }
        if secondary_btn(ui, "Désélectionner", ui.available_width()).clicked() {
            st.clear_edge();
        }
    });
}

pub(crate) fn chip_label(ui: &mut Ui, text: &str, bg: Color32, fg: Color32) {
    let g = ui.painter().layout_no_wrap(text.to_string(), medium(11.5), fg);
    let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 14.0, 20.0), Sense::hover());
    ui.painter().rect_filled(r, 6.0, bg);
    ui.painter().galley(pos2(r.min.x + 7.0, r.center().y - g.size().y * 0.5), g, fg);
}

pub(crate) fn material_palette(ui: &mut Ui, st: &mut AppState, target: Option<NodeId>, current: u32) {
    let mats = st.scene.materials.clone();
    let cols = 3;
    egui::Grid::new(("mats", target)).spacing(vec2(6.0, 6.0)).show(ui, |ui| {
        for (i, m) in mats.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2((RIGHT_W - 28.0 - 12.0) / 3.0, 40.0), Sense::click());
            let on = current as usize == i;
            let p = ui.painter();
            p.rect(r, 9.0, if on { ACCENT_BG } else { WELL }, Stroke::new(1.0, if on { ACCENT } else { Color32::TRANSPARENT }), StrokeKind::Inside);
            let c = Color32::from_rgb((m.color[0] * 255.0) as u8, (m.color[1] * 255.0) as u8, (m.color[2] * 255.0) as u8);
            p.rect(Rect::from_min_size(r.min + vec2(8.0, 11.0), vec2(18.0, 18.0)), 5.0, c, Stroke::new(1.0, tint(Color32::WHITE, 0.15)), StrokeKind::Inside);
            let name = m.name.split(' ').next().unwrap_or("").to_string();
            let fg = if on { ACCENT_TEXT } else { LABEL };
            let mut job = egui::text::LayoutJob::single_section(name, egui::text::TextFormat::simple(regular(12.0), fg));
            job.wrap = egui::text::TextWrapping { max_width: r.width() - 38.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            let g = p.layout_job(job);
            p.galley(pos2(r.min.x + 32.0, r.center().y - g.size().y * 0.5), g, fg);
            if resp.clicked() {
                match target {
                    Some(id) => st.edit("Matière", id, |n| n.material = i as u32),
                    None => st.material = i as u32,
                }
            }
            if (i + 1) % cols == 0 {
                ui.end_row();
            }
        }
    });
}

fn shape_rows(ui: &mut Ui, st: &mut AppState, id: NodeId, sh: &Shape) {
    let mut s = sh.clone();
    let mut any = false;
    let mut start = false;
    let mut row = |ui: &mut Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, step: f64, scale: f32, suffix: &str| {
        let mut shown = *v * scale;
        let (c, st0) = slider_row(ui, label, &mut shown, range, step, suffix);
        if c {
            *v = shown / scale;
            any = true;
            start |= st0;
        }
    };
    match sh.kind {
        PrimKind::Box => {
            row(ui, "Largeur", &mut s.size.x, 0.5..=200.0, 0.5, 2.0, "mm");
            row(ui, "Hauteur", &mut s.size.y, 0.5..=200.0, 0.5, 2.0, "mm");
            row(ui, "Profondeur", &mut s.size.z, 0.5..=200.0, 0.5, 2.0, "mm");
        }
        PrimKind::Cylinder | PrimKind::Prism => {
            row(ui, "Rayon", &mut s.size.x, 0.2..=100.0, 0.1, 1.0, "mm");
            row(ui, "Hauteur", &mut s.size.y, 0.2..=200.0, 0.5, 2.0, "mm");
        }
        PrimKind::Sphere => {
            row(ui, "Rayon X", &mut s.size.x, 0.2..=100.0, 0.1, 1.0, "mm");
            row(ui, "Rayon Y", &mut s.size.y, 0.2..=100.0, 0.1, 1.0, "mm");
            row(ui, "Rayon Z", &mut s.size.z, 0.2..=100.0, 0.1, 1.0, "mm");
        }
        PrimKind::Capsule => {
            row(ui, "Rayon", &mut s.size.x, 0.2..=100.0, 0.1, 1.0, "mm");
            row(ui, "Longueur", &mut s.size.y, 0.0..=200.0, 0.5, 2.0, "mm");
        }
        PrimKind::Torus => {
            row(ui, "Rayon", &mut s.size.x, 0.5..=100.0, 0.1, 1.0, "mm");
            row(ui, "Épaisseur", &mut s.size.z, 0.2..=40.0, 0.1, 1.0, "mm");
        }
        PrimKind::Cone => {
            row(ui, "Rayon bas", &mut s.size.x, 0.0..=100.0, 0.1, 1.0, "mm");
            row(ui, "Rayon haut", &mut s.size.z, 0.0..=100.0, 0.1, 1.0, "mm");
            row(ui, "Hauteur", &mut s.size.y, 0.2..=200.0, 0.5, 2.0, "mm");
        }
        PrimKind::Tube => {
            row(ui, "Rayon ext.", &mut s.size.x, 0.5..=100.0, 0.1, 1.0, "mm");
            row(ui, "Rayon int.", &mut s.size.z, 0.0..=100.0, 0.1, 1.0, "mm");
            row(ui, "Hauteur", &mut s.size.y, 0.2..=200.0, 0.5, 2.0, "mm");
        }
        PrimKind::Profile => {
            row(ui, "Épaisseur", &mut s.size.z, 0.2..=100.0, 0.5, 2.0, "mm");
        }
    }
    let max_round = match sh.kind {
        PrimKind::Box => s.size.min_element(),
        PrimKind::Profile => s.size.z,
        _ => s.size.x.min(s.size.y.max(0.1)),
    };
    if !sh.edges.is_empty() {
        ui.label(egui::RichText::new("Fillets réglés arête par arête · Rondeur / Chanfrein les remettent tous pareils").font(regular(11.5)).color(EDGE_SEL));
    }
    let (r0, c0) = (s.round, s.chamfer);
    if !matches!(sh.kind, PrimKind::Sphere | PrimKind::Capsule | PrimKind::Torus) {
        row(ui, "Rondeur", &mut s.round, 0.0..=max_round.max(0.1), 0.1, 1.0, "mm");
    }
    if matches!(sh.kind, PrimKind::Box | PrimKind::Cylinder) {
        row(ui, "Chanfrein", &mut s.chamfer, 0.0..=max_round.max(0.1), 0.1, 1.0, "mm");
    }
    if s.round != r0 || s.chamfer != c0 {
        s.edges.clear();
    }
    if sh.kind == PrimKind::Prism {
        if let Some(n) = stepper(ui, "Côtés", sh.sides, 3, 24) {
            s.sides = n;
            any = true;
            start = true;
        }
    }
    if sh.kind == PrimKind::Profile {
        if let Some(prof) = &sh.profile {
            let modes = [(ProfileMode::Extrude, "Extrusion", ACCENT), (ProfileMode::Revolve, "Révolution", ACCENT)];
            if let Some(m) = segmented(ui, &modes, prof.mode, 40.0) {
                if let Some(p) = s.profile.as_mut() {
                    p.mode = m;
                }
                any = true;
                start = true;
            }
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

fn scene_panel(ui: &mut Ui, st: &mut AppState) {
    ui.label(egui::RichText::new("Scène").font(regular(12.0)).color(TEXT2));
    ui.label(egui::RichText::new(st.doc_name.clone()).font(semibold(17.0)).color(TEXT));
    ui.label(egui::RichText::new(format!("{} formes · touche une forme pour l'éditer", st.scene.shape_count())).font(regular(12.0)).color(TEXT2));
    ui.add_space(4.0);
    divider(ui);
    section(ui, "Nouvelles formes");
    let items = MODES.map(|m| (m, m.label(), mode_color(m)));
    if let Some(m) = segmented(ui, &items, st.mode, 40.0) {
        st.mode = m;
    }
    section(ui, "Matière par défaut");
    let cur = st.material;
    material_palette(ui, st, None, cur);
    section(ui, "Document");
    let w = RIGHT_W - 28.0;
    if secondary_btn(ui, "Cadrer toute la scène", w).clicked() {
        st.frame_all();
    }
    if secondary_btn(ui, "Ouvrir · Nouveau…", w).clicked() {
        st.files_open = true;
    }
    if primary_btn(ui, None, "Enregistrer", Some(w)).clicked() {
        st.save();
    }
    section(ui, "Affichage");
    if toggle_row(ui, "Arbre de formes", st.show_tree) {
        st.show_tree = !st.show_tree;
    }
}

fn tree(ui: &mut Ui, st: &mut AppState, height: f32) {
    ui.horizontal(|ui| {
        ui.set_height(36.0);
        let (lr, _) = ui.allocate_exact_size(vec2(ui.available_width() - 3.0 * 38.0, 36.0), Sense::hover());
        ui.painter().text(lr.left_center() + vec2(4.0, 0.0), Align2::LEFT_CENTER, "Arbre de formes", semibold(13.0), TEXT);
        ui.spacing_mut().item_spacing.x = 2.0;
        if icon_btn(ui, Icon::Folder, false, "Grouper la sélection", 36.0).clicked() {
            st.group_selected();
        }
        if icon_btn(ui, Icon::Up, false, "Monter (évalué plus tôt)", 36.0).clicked() {
            if let Some(id) = st.selection {
                st.move_in_parent(id, -1);
            }
        }
        if icon_btn(ui, Icon::Down, false, "Descendre (évalué plus tard)", 36.0).clicked() {
            if let Some(id) = st.selection {
                st.move_in_parent(id, 1);
            }
        }
    });
    let rows = st.scene.rows();
    egui::ScrollArea::vertical().max_height(height - 40.0).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (id, depth) in rows {
            let Some(n) = st.scene.node(id).cloned() else { continue };
            let w = ui.available_width();
            let (r, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
            let selected = st.is_selected(id);
            let p = ui.painter();
            if selected {
                p.rect(r, 8.0, ACCENT_BG, Stroke::new(1.0, ACCENT_BORDER), StrokeKind::Inside);
            } else if resp.hovered() {
                p.rect_filled(r, 8.0, RAISED);
            }
            let x0 = r.min.x + 6.0 + depth as f32 * 16.0;
            let dim = if n.visible { 1.0 } else { 0.4 };
            let mut toggled = false;
            if n.is_group() {
                let cr = Rect::from_min_size(pos2(x0, r.center().y - 7.0), vec2(14.0, 14.0));
                icons::paint(p, cr, if n.expanded { Icon::Chevron } else { Icon::ChevronRight }, TEXT2);
                let cresp = ui.interact(cr.expand(8.0), Id::new(("exp", id)), Sense::click());
                if cresp.clicked() {
                    toggled = true;
                }
                let p = ui.painter();
                icons::paint(p, Rect::from_min_size(pos2(x0 + 20.0, r.center().y - 8.0), vec2(16.0, 16.0)), Icon::Folder, tint(TEXT2, dim));
            } else {
                let c = mode_color(n.combine.mode);
                let br = Rect::from_min_size(pos2(x0 + 20.0, r.center().y - 8.0), vec2(16.0, 16.0));
                p.rect_filled(br, 4.0, tint(c, 0.16 * dim));
                paint_mode_glyph(p, br.center(), n.combine.mode, tint(c, dim), 9.0);
            }
            let p = ui.painter();
            let name_color = if selected { TEXT_STRONG } else { tint(TEXT, dim) };
            let mut info = n.type_label();
            if n.repeat.count() > 1 {
                info += &format!(" ×{}", n.repeat.count());
            }
            if n.mirror.any() {
                info += " · sym.";
            }
            let info_g = p.layout_no_wrap(info, regular(12.0), if selected { Color32::from_rgb(0xC3, 0xD9, 0xF2) } else { tint(TEXT2, dim) });
            let info_x = r.max.x - 40.0 - info_g.size().x;
            p.galley(pos2(info_x, r.center().y - info_g.size().y * 0.5), info_g, Color32::WHITE);
            // the name gives way to the type (ellipsis)
            let max_w = (info_x - 8.0 - (x0 + 44.0)).max(20.0);
            let mut job = egui::text::LayoutJob::single_section(n.name.clone(), egui::TextFormat::simple(if selected { semibold(13.0) } else { regular(13.0) }, name_color));
            job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            let name_g = ui.painter().layout_job(job);
            let p = ui.painter();
            p.galley(pos2(x0 + 44.0, r.center().y - name_g.size().y * 0.5), name_g, name_color);
            let er = Rect::from_center_size(pos2(r.max.x - 18.0, r.center().y), vec2(16.0, 16.0));
            icons::paint(p, er, if n.visible { Icon::Eye } else { Icon::EyeOff }, tint(TEXT2, 0.8));
            let eresp = ui.interact(er.expand(10.0), Id::new(("eye", id)), Sense::click());
            if eresp.clicked() {
                st.toggle_visible(id);
            } else if toggled {
                if let Some(nm) = st.scene.node_mut(id) {
                    nm.expanded = !nm.expanded;
                }
            } else if resp.clicked() {
                st.tool = Tool::Select;
                if st.multi() || ui.input(|i| i.modifiers.shift) {
                    st.toggle_in_selection(id);
                } else {
                    st.select(Some(id));
                }
            }
        }
    });
}

// ----------------------------------------------------------------- dialogs

pub(crate) fn dim(ctx: &egui::Context, screen: Rect, id: &str) -> bool {
    let mut close = false;
    egui::Area::new(Id::new(id)).order(Order::Foreground).fade_in(false).fixed_pos(screen.min).show(ctx, |ui| {
        let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
        ui.painter().rect_filled(r, 0.0, tint(Color32::BLACK, 0.45));
        close = resp.clicked();
    });
    close
}

pub(crate) fn export_dialog(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    if dim(ctx, screen, "export-dim") && st.export_job.is_none() {
        st.export_open = false;
        return;
    }
    egui::Area::new(Id::new("export")).order(Order::Tooltip).pivot(Align2::CENTER_CENTER).fixed_pos(screen.center()).show(ctx, |ui| {
        panel().inner_margin(Margin::same(20)).show(ui, |ui| {
            ui.set_width(380.0);
            ui.label(egui::RichText::new("Exporter le mesh").font(semibold(18.0)).color(TEXT));
            ui.label(
                egui::RichText::new("Maillage à arêtes vives (dual contouring). Les matières partent en couleurs de vertex pour le masque ID de Substance Painter.")
                    .font(regular(12.5))
                    .color(TEXT2),
            );
            ui.add_space(6.0);
            section(ui, "Résolution");
            let res = [
                (ExportRes::Draft, ExportRes::Draft.label(), ACCENT),
                (ExportRes::Normal, ExportRes::Normal.label(), ACCENT),
                (ExportRes::Fine, ExportRes::Fine.label(), ACCENT),
            ];
            if let Some(r) = segmented(ui, &res, st.export_res, 40.0) {
                st.export_res = r;
            }
            let (lo, hi) = st.scene.bounds();
            let cell = (hi - lo).max_element() / st.export_res.cells();
            ui.label(egui::RichText::new(format!("Cellule : {} mm", fmt_mm(cell.max(0.05)))).font(regular(12.0)).color(TEXT2));
            section(ui, "Formats");
            if toggle_row(ui, "glTF binaire (.glb)", st.export_glb) {
                st.export_glb = !st.export_glb;
            }
            if toggle_row(ui, "OBJ avec couleurs de vertex", st.export_obj) {
                st.export_obj = !st.export_obj;
            }
            ui.label(egui::RichText::new(format!("Dossier : {}", files::documents_dir().display())).font(regular(11.5)).color(TEXT2));
            ui.add_space(8.0);
            if let Some((_, t0)) = &st.export_job {
                let el = st.now - t0;
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(18.0).color(ACCENT));
                    ui.label(egui::RichText::new(format!("Maillage en cours… {:.0} s", el)).color(LABEL));
                });
                ctx.request_repaint();
            } else {
                ui.horizontal(|ui| {
                    if secondary_btn(ui, "Annuler", 120.0).clicked() {
                        st.export_open = false;
                    }
                    if primary_btn(ui, Some(Icon::Export), "Exporter", Some(250.0)).clicked() {
                        st.start_export();
                    }
                });
            }
        });
    });
}

fn stat_row(ui: &mut Ui, label: &str, value: String, color: Color32) {
    ui.horizontal(|ui| {
        ui.set_height(22.0);
        ui.label(egui::RichText::new(label).font(regular(13.0)).color(TEXT2));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).font(medium(13.0)).color(color));
        });
    });
}

/// Atelier: the Bake panel docked on the left, next to the rail.
pub(crate) const BAKE_PANEL_W: f32 = 340.0;

fn bake_side_panel(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let rect = Rect::from_min_max(pos2(92.0, 72.0), pos2(92.0 + BAKE_PANEL_W, screen.height() - 84.0));
    egui::Area::new(Id::new("bake-panel")).order(Order::Middle).fixed_pos(rect.min).show(ctx, |ui| {
        panel().inner_margin(Margin::same(16)).show(ui, |ui| {
            ui.set_width(rect.width() - 32.0);
            ui.set_height(rect.height() - 32.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Bake").font(semibold(18.0)).color(TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_btn(ui, Icon::ChevronLeft, false, "Fermer le panneau Bake", 32.0).clicked() {
                        st.set_bake_mode(false);
                    }
                });
            });
            egui::ScrollArea::vertical().id_salt("bake-panel-scroll").auto_shrink([false, false]).show(ui, |ui| {
                bake_body(ui, st, false);
            });
        });
    });
}

/// One on/off option: a switch row in Atelier, a check box in Studio.
fn opt_row(ui: &mut Ui, compact: bool, label: &str, on: bool) -> bool {
    if compact {
        let mut v = on;
        ui.checkbox(&mut v, label).changed()
    } else {
        toggle_row(ui, label, on)
    }
}

fn note(ui: &mut Ui, text: impl Into<String>, color: Color32) {
    ui.add(egui::Label::new(egui::RichText::new(text.into()).font(regular(11.5)).color(color)).wrap());
}

/// Bake panel content, shared by both interfaces: run, diagnostic layers,
/// result, optional pass, export.
pub(crate) fn bake_body(ui: &mut Ui, st: &mut AppState, compact: bool) {
    use forme_core::bake::{Quality, STAGES};
    note(ui, "Lignes de construction lues dans l'historique : jaune = arêtes et jonctions, orange = rails de fillet. Le maillage doit partir de ces lignes.", TEXT2);
    section(ui, "Densité");
    let running = st.bake.job.is_some();
    if compact {
        egui::ComboBox::from_id_salt("bake-quality-panel").width(160.0).selected_text(st.bake.quality.label()).show_ui(ui, |ui| {
            for q in [Quality::Draft, Quality::Standard, Quality::Max] {
                ui.selectable_value(&mut st.bake.quality, q, q.label());
            }
        });
    } else {
        let q = [(Quality::Draft, "Légère", ACCENT), (Quality::Standard, "Standard", ACCENT), (Quality::Max, "Dense", ACCENT)];
        if let Some(v) = segmented(ui, &q, st.bake.quality, 36.0) {
            if !running {
                st.bake.quality = v;
            }
        }
    }
    match st.bake_plan() {
        Ok(size) => note(ui, format!("Par lignes de construction · quads de ~{} mm · fillets en rangées", fmt_mm(size as f32)), OK),
        Err(why) => note(ui, format!("Bake : {why}"), ERROR),
    }
    ui.add_space(6.0);
    if let Some(j) = &st.bake.job {
        let f = j.progress.fraction();
        let stage = j.progress.stage.load(std::sync::atomic::Ordering::Relaxed) as usize;
        let el = st.now - j.started;
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
        ui.painter().rect_filled(r, 4.0, WELL);
        ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(r.width() * f.max(0.02), r.height())), 4.0, ACCENT);
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(14.0).color(ACCENT));
            ui.label(egui::RichText::new(format!("{} · {:.0} % · {:.0} s", STAGES.get(stage).copied().unwrap_or(""), f * 100.0, el)).font(medium(12.5)).color(LABEL));
        });
        if secondary_btn(ui, "Annuler le Bake", ui.available_width()).clicked() {
            st.cancel_bake();
        }
    } else if primary_btn(ui, Some(Icon::Cube), if st.bake.result.is_some() { "Nouveau Bake" } else { "Lancer le Bake" }, Some(ui.available_width())).clicked() {
        st.start_bake();
    }

    // what the algorithm sees and builds
    section(ui, "Diagnostic");
    if opt_row(ui, compact, "Lignes de construction (jaune)", st.bake.diag.lines) {
        st.bake.diag.lines = !st.bake.diag.lines;
    }
    if opt_row(ui, compact, "Rails de fillet (orange)", st.bake.diag.rails) {
        st.bake.diag.rails = !st.bake.diag.rails;
    }
    match &st.bake.lines {
        Some((v, l)) if *v == st.scene_version => {
            let rails = l.lines.iter().filter(|x| x.kind.is_rail()).count();
            note(
                ui,
                format!("{} lignes : {} d'arêtes, {} de jonctions, dont {} rails · {:.0} ms", l.lines.len(), l.count(false), l.count(true), rails, l.seconds * 1000.0),
                LABEL,
            );
        }
        _ if st.bake.diag.lines || st.bake.diag.rails => note(ui, "Lecture des lignes…", TEXT2),
        _ => {}
    }
    let has = st.bake.result.is_some();
    if opt_row(ui, compact, "Afficher le maillage baké", st.bake.show) && has {
        st.bake.show = !st.bake.show && !st.bake.stale(st.scene_version);
    }
    if opt_row(ui, compact, "Rôles (fillets / paths)", st.bake.diag.roles) {
        st.bake.diag.roles = !st.bake.diag.roles;
        if st.bake.diag.roles && has {
            st.bake.show = !st.bake.stale(st.scene_version);
        }
    }
    if opt_row(ui, compact, "Filaire (arêtes des polygones)", st.bake.wire) {
        st.bake.wire = !st.bake.wire;
    }

    if let Some((v, why)) = st.bake.error.clone() {
        section(ui, "Résultat");
        note(ui, format!("Bake impossible : {why}"), ERROR);
        if v != st.scene_version {
            note(ui, "Le modèle a changé depuis : relance le Bake.", TEXT2);
        }
        return;
    }
    let Some(b) = st.bake.result.clone() else { return };
    let s = &b.stats;
    section(ui, "Résultat");
    if st.bake.stale(st.scene_version) {
        note(ui, "Le modèle a changé depuis ce Bake : relance-le.", CARVE);
    }
    let total = b.faces.len().max(1);
    stat_row(ui, "Faces", format!("{}  ({} % quads)", fmt_int(b.faces.len()), s.quads * 100 / total), TEXT);
    stat_row(ui, "Quads · tris · n-gones", format!("{} · {} · {}", fmt_int(s.quads), fmt_int(s.tris), fmt_int(s.ngons)), LABEL);
    stat_row(ui, "Écart", format!("moy. {} · max {} mm", fmt_mm_fine(s.mean_dev), fmt_mm_fine(s.max_dev)), LABEL);
    let defects = s.open_edges + s.nonmanifold_edges;
    stat_row(ui, "Fermeture", if defects == 0 { "étanche".to_string() } else { format!("{} arêtes à reprendre", fmt_int(defects)) }, if defects == 0 { OK } else { CARVE });
    stat_row(ui, "Durée", format!("{} patchs · {:.2} s", fmt_int(s.patches), s.seconds).replace('.', ","), LABEL);

    section(ui, "Passe secondaire (option)");
    let deg = forme_core::bake::decimate::FLAT_LOOPS_DEG;
    if opt_row(ui, compact, &format!("Retirer les loops plates (< {deg:.0}°)"), st.bake.flat) {
        st.set_flat_pass(!st.bake.flat);
    }
    if st.bake.pass.is_some() {
        note(ui, "Passe en cours…", LABEL);
    } else if st.bake.flat && s.faces_before_flat > 0 {
        note(ui, format!("{} loops retirées · {} → {} faces", s.flat_loops, fmt_int(s.faces_before_flat), fmt_int(b.faces.len())), OK);
    }

    section(ui, "Export");
    if let Some(m) = &st.bake.last_export {
        note(ui, format!("{m} · {}", files::documents_dir().display()), OK);
    }
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 8.0) * 0.5;
        if secondary_btn(ui, "OBJ (quads)", w).clicked() {
            st.export_bake(true, false);
        }
        if secondary_btn(ui, "GLB", w).clicked() {
            st.export_bake(false, true);
        }
    });
}

fn fmt_mm_fine(v: f64) -> String {
    if v < 0.001 {
        format!("{:.4}", v).replace('.', ",")
    } else {
        format!("{:.3}", v).replace('.', ",")
    }
}

pub(crate) fn files_dialog(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    if dim(ctx, screen, "files-dim") {
        st.files_open = false;
        return;
    }
    egui::Area::new(Id::new("files")).order(Order::Tooltip).pivot(Align2::CENTER_CENTER).fixed_pos(screen.center()).show(ctx, |ui| {
        panel().inner_margin(Margin::same(20)).show(ui, |ui| {
            ui.set_width(420.0);
            ui.label(egui::RichText::new("Documents").font(semibold(18.0)).color(TEXT));
            ui.add_space(4.0);
            section(ui, "Nom du document");
            let mut name = st.doc_name.clone();
            if ui.add(egui::TextEdit::singleline(&mut name).desired_width(380.0).font(medium(14.0))).changed() {
                st.doc_name = name;
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if primary_btn(ui, None, "Enregistrer", Some(130.0)).clicked() {
                    st.save();
                }
                if secondary_btn(ui, "Nouveau", 110.0).clicked() {
                    st.new_document(Scene::new(), "Sans titre");
                    st.files_open = false;
                }
                if secondary_btn(ui, "Démo grenade", 130.0).clicked() {
                    st.new_document(demo::grenade(), "Grenade sci-fi");
                    st.camera = crate::camera::OrbitCamera::default();
                    st.files_open = false;
                }
            });
            section(ui, "Récents");
            let list = files::list();
            if list.is_empty() {
                ui.label(egui::RichText::new("Aucun document enregistré pour l'instant.").color(TEXT2));
            }
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for (n, path) in list {
                    let (r, resp) = ui.allocate_exact_size(vec2(380.0, 44.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 9.0, RAISED);
                    }
                    icons::paint(ui.painter(), Rect::from_min_size(r.min + vec2(10.0, 13.0), vec2(18.0, 18.0)), Icon::Cube, TEXT_ICON);
                    ui.painter().text(r.min + vec2(38.0, 22.0), Align2::LEFT_CENTER, &n, medium(14.0), TEXT);
                    if resp.clicked() {
                        st.open(&path);
                        st.files_open = false;
                    }
                }
            });
        });
    });
}

// ---------------------------------------------------------------- settings

/// Light panel: viewport and display options, applied live and kept in
/// settings.json. Floating under its button, the viewport stays usable.
pub fn display_panel(ctx: &egui::Context, st: &mut AppState) {
    if !st.display_open {
        return;
    }
    let at = pos2(st.display_anchor[0], st.display_anchor[1] + 8.0);
    let resp = egui::Area::new(Id::new("display-panel")).order(Order::Foreground).fade_in(false).pivot(Align2::RIGHT_TOP).fixed_pos(at).show(ctx, |ui| {
        panel().inner_margin(Margin::same(16)).show(ui, |ui| {
            ui.set_width(320.0);
            ui.set_max_width(320.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Affichage").font(semibold(16.0)).color(TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Label::new(egui::RichText::new("Réinitialiser").font(regular(12.0)).color(ACCENT_TEXT)).sense(Sense::click())).clicked() {
                        st.settings.display = crate::settings::Display::default();
                        st.save_settings();
                    }
                });
            });
            let d0 = st.settings.display;
            let d = &mut st.settings.display;
            section(ui, "Rendu");
            if toggle_row(ui, "Ombres", d.shadows) {
                d.shadows = !d.shadows;
            }
            if toggle_row(ui, "Occlusion ambiante", d.ao) {
                d.ao = !d.ao;
            }
            ui.label(egui::RichText::new("Anticrénelage (image immobile)").font(regular(13.0)).color(LABEL));
            let items = [(1u32, "Non", NEUTRAL_MODE), (4, "4×", ACCENT), (8, "8×", ACCENT), (16, "16×", ACCENT)];
            if let Some(v) = segmented(ui, &items, d.aa, 34.0) {
                d.aa = v;
            }
            section(ui, "Lumière");
            slider_row(ui, "Orientation", &mut d.light_azimuth, -180.0..=180.0, 5.0, "°");
            slider_row(ui, "Hauteur", &mut d.light_elevation, 5.0..=90.0, 5.0, "°");
            slider_row(ui, "Intensité", &mut d.light_intensity, 0.0..=2.0, 0.05, "");
            slider_row(ui, "Exposition", &mut d.exposure, 0.3..=2.5, 0.05, "");
            section(ui, "Vue");
            let grid = st.grid;
            if toggle_row(ui, "Grille au sol", grid) {
                st.grid = !st.grid;
            }
            if st.settings.display != d0 {
                st.save_settings();
            }
        });
    });
    // a click outside (not on the light button) closes it
    if ctx.input(|i| i.pointer.any_pressed()) {
        let p = ctx.input(|i| i.pointer.interact_pos());
        let button = Rect::from_center_size(pos2(st.display_anchor[0] - 22.0, st.display_anchor[1] - 22.0), vec2(48.0, 48.0));
        if let Some(p) = p {
            if !resp.response.rect.contains(p) && !button.contains(p) {
                st.display_open = false;
            }
        }
    }
}

/// Light button: opens the display panel under it.
pub(crate) fn light_toggle(st: &mut AppState, resp: &Response) {
    if resp.clicked() {
        st.display_open = !st.display_open;
        st.display_anchor = [resp.rect.right(), resp.rect.bottom()];
    }
}

/// Settings: the interface (Atelier or Studio) and the creation options.
/// Shown over both interfaces.
pub fn settings_dialog(ctx: &egui::Context, st: &mut AppState) {
    if !st.settings_open {
        return;
    }
    let screen = ctx.content_rect();
    if dim(ctx, screen, "settings-dim") {
        st.settings_open = false;
        return;
    }
    egui::Area::new(Id::new("settings")).order(Order::Tooltip).fade_in(false).pivot(Align2::CENTER_CENTER).fixed_pos(screen.center()).show(ctx, |ui| {
        panel().inner_margin(Margin::same(20)).show(ui, |ui| {
            ui.set_width(440.0);
            ui.set_max_width(440.0);
            ui.label(egui::RichText::new("Réglages").font(semibold(18.0)).color(TEXT));
            section(ui, "Interface");
            for m in UiMode::ALL {
                let on = st.settings.ui_mode == m;
                let blurb = ui.painter().layout(m.blurb().to_string(), regular(12.0), TEXT2, 440.0 - 58.0);
                let (r, resp) = ui.allocate_exact_size(vec2(440.0, 38.0 + blurb.size().y + 10.0), Sense::click());
                let p = ui.painter();
                if on {
                    p.rect(r, 12.0, ACCENT_BG, Stroke::new(1.0, ACCENT_BORDER), StrokeKind::Inside);
                } else {
                    p.rect(r, 12.0, if resp.hovered() { RAISED } else { Color32::TRANSPARENT }, Stroke::new(1.0, DIVIDER), StrokeKind::Inside);
                }
                let c = pos2(r.min.x + 22.0, r.center().y);
                p.circle_stroke(c, 8.0, Stroke::new(1.5, if on { ACCENT } else { TEXT2 }));
                if on {
                    p.circle_filled(c, 4.5, ACCENT);
                }
                p.text(pos2(r.min.x + 42.0, r.min.y + 20.0), Align2::LEFT_CENTER, m.label(), semibold(14.0), if on { ACCENT_TEXT } else { TEXT });
                p.galley(pos2(r.min.x + 42.0, r.min.y + 32.0), blurb, TEXT2);
                if resp.clicked() {
                    st.set_ui_mode(m);
                }
            }
            ui.label(egui::RichText::new("Le choix est gardé pour les prochaines sessions.").font(regular(11.5)).color(TEXT2));
            section(ui, "Création");
            if toggle_row(ui, "Accrocher aux faces des objets (AutoGrid)", st.settings.snap_faces) {
                st.settings.snap_faces = !st.settings.snap_faces;
                st.save_settings();
            }
            ui.label(
                egui::RichText::new("Activé : la base part de la face sous le curseur, sinon du sol ; avec Snap, elle s'accroche aux arêtes et aux sommets. Monte pour ajouter (+), descends pour creuser (−), Ctrl inverse pendant la création.")
                    .font(regular(11.5))
                    .color(TEXT2),
            );
            ui.add_space(10.0);
            ui.add(egui::Label::new(egui::RichText::new(crate::version_title()).font(regular(11.0)).color(TEXT2)).wrap());
            ui.add(egui::Label::new(egui::RichText::new(format!("Fichier : {}", crate::settings::Settings::path().display())).font(regular(11.0)).color(TEXT2)).wrap());
            ui.add_space(6.0);
            if primary_btn(ui, None, "Fermer", Some(440.0)).clicked() {
                st.settings_open = false;
            }
        });
    });
}
