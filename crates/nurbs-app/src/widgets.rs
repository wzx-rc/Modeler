//! Widgets of the interface (the look of the first Forme NURBS): islands,
//! icon buttons, segmented controls, slider rows with a typed value, chips.

use crate::icons::{self, Icon};
use crate::theme::*;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Frame, Id, Margin, Order, Rect, Response, Sense, Stroke, StrokeKind, Ui};

/// Desktop layout: compact rows for the mouse.
const DESK: bool = true;

/// 12,5 (French decimals, like the first app).
pub fn fmt_mm(v: f32) -> String {
    let s = format!("{:.1}", v);
    // no "-0,0" for a value that rounds to zero
    let s = if s == "-0.0" { "0.0".to_string() } else { s };
    s.replace('.', ",")
}

pub fn panel() -> Frame {
    Frame::new().fill(PANEL()).stroke(Stroke::new(1.0, PANEL_BORDER())).corner_radius(CornerRadius::same(16)).inner_margin(Margin::same(8))
}

pub fn icon_btn(ui: &mut Ui, icon: Icon, selected: bool, tip: &str, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let p = ui.painter();
    let rr = if size <= 36.0 { 7.0 } else { 10.0 };
    if selected {
        p.rect(rect.shrink(0.5), rr, accent_bg(), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
    } else if resp.hovered() || resp.is_pointer_button_down_on() {
        p.rect_filled(rect, rr, RAISED());
    }
    let c = if selected { accent_text() } else { TEXT_ICON() };
    let isz = if size < 32.0 { 15.0 } else if size < 40.0 { 17.0 } else { 20.0 };
    icons::paint(p, Rect::from_center_size(rect.center(), vec2(isz, isz)), icon, c);
    resp.on_hover_text(tip)
}

pub fn primary_btn(ui: &mut Ui, icon: Option<Icon>, label: &str, width: Option<f32>) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), semibold(14.0), on_accent());
    let w = width.unwrap_or(galley.size().x + 32.0 + if icon.is_some() { 24.0 } else { 0.0 });
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
    let p = ui.painter();
    let fill = if resp.is_pointer_button_down_on() { accent_light() } else { accent() };
    p.rect_filled(rect, 10.0, fill);
    let tw = galley.size().x + if icon.is_some() { 24.0 } else { 0.0 };
    let x0 = rect.center().x - tw * 0.5;
    if let Some(i) = icon {
        icons::paint(p, Rect::from_min_size(pos2(x0, rect.center().y - 8.0), vec2(16.0, 16.0)), i, on_accent());
    }
    p.galley(pos2(x0 + if icon.is_some() { 24.0 } else { 0.0 }, rect.center().y - galley.size().y * 0.5), galley, on_accent());
    resp
}

pub fn secondary_btn(ui: &mut Ui, label: &str, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 44.0), Sense::click());
    let p = ui.painter();
    let fill = if resp.hovered() { RAISED() } else { Color32::TRANSPARENT };
    p.rect(rect.shrink(0.5), 10.0, fill, Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, label, medium(14.0), LABEL());
    resp
}

/// Segmented control. Returns the picked value.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, items: &[(T, &str, Color32)], current: T, height: f32) -> Option<T> {
    let height = if DESK { height.min(28.0) } else { height };
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    ui.painter().rect_filled(rect, 11.0, WELL());
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
            p.rect_filled(r, 8.0, RAISED());
        }
        p.text(r.center(), Align2::CENTER_CENTER, *label, semibold(if DESK { 11.5 } else { 12.0 }), if on { TEXT_STRONG() } else { pick(Color32::from_rgb(0xAE, 0xB3, 0xBA), Color32::from_rgb(0x6A, 0x71, 0x7B)) });
        if resp.clicked() {
            picked = Some(*v);
        }
    }
    picked
}

pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(title).font(semibold(12.0)).color(TEXT2()));
}

pub fn nice_ceil(x: f32) -> f32 {
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

/// A value with at most 3 decimals (Romain, 19 nov.: "3 après la virgule
/// suffit"), trailing zeros dropped, at least one decimal, French comma.
pub fn fmt3(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    let mut s = format!("{:.3}", if r == 0.0 { 0.0 } else { r });
    while s.ends_with('0') && !s.ends_with(".0") {
        s.pop();
    }
    s.replace('.', ",")
}

/// Rounds a value to 3 decimals.
pub fn round3(v: f32) -> f32 {
    ((v as f64 * 1000.0).round() / 1000.0) as f32
}

/// Thin slider for the mouse (Romain, 19 nov.: "plus fin… gris très clair,
/// blanc, avec des petits liserés", like the component modes' bar): a hair
/// track, the part up to the value in light grey, a small pill for a knob.
/// Sizes use an adaptive range so the knob sits mid-track whatever the
/// scale; the range is frozen while dragging.
pub fn touch_slider(ui: &mut Ui, id: Id, v: &mut f32, min: f32, max: f32, adaptive: bool, step: f32, width: f32) -> (bool, bool) {
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
    let rail = Rect::from_center_size(rect.center(), vec2(width - if DESK { 10.0 } else { 20.0 }, if DESK { 1.5 } else { 4.0 }));
    let mut changed = false;
    let started = resp.drag_started() || resp.clicked();
    if resp.dragged() || resp.clicked() || resp.drag_started() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.x - rail.min.x) / rail.width()).clamp(0.0, 1.0);
            let mut nv = min + t * (hi - min);
            if step > 0.0 {
                nv = (nv / step).round() * step;
            }
            let nv = round3(nv);
            if (nv - *v).abs() > 1e-6 {
                *v = nv;
                changed = true;
            }
        }
    }
    let t = ((*v - min) / (hi - min)).clamp(0.0, 1.0);
    let kx = rail.min.x + t * rail.width();
    let p = ui.painter();
    let hot = resp.hovered() || resp.dragged() || resp.is_pointer_button_down_on();
    let active = resp.dragged() || resp.is_pointer_button_down_on();
    p.rect_filled(rail, 1.0, pick(Color32::from_rgb(0x3A, 0x3D, 0x44), Color32::from_rgb(0xCF, 0xD3, 0xD9)));
    let zero = if min < 0.0 { rail.min.x + (-min / (hi - min)) * rail.width() } else { rail.min.x };
    p.rect_filled(Rect::from_x_y_ranges(zero.min(kx)..=zero.max(kx), rail.y_range()), 1.0, pick(Color32::from_rgb(0xC8, 0xCC, 0xD2), Color32::from_rgb(0x5A, 0x60, 0x6A)));
    if DESK {
        // the knob: a small upright pill, light grey with a fine dark edge
        // and a thin light ring round it when hovered or held
        let k = Rect::from_center_size(pos2(kx, rail.center().y), vec2(6.0, 12.0));
        let fill = if active { Color32::WHITE } else { pick(Color32::from_rgb(0xDD, 0xE0, 0xE4), Color32::from_rgb(0xFA, 0xFB, 0xFC)) };
        p.rect(k, 3.0, fill, Stroke::new(1.0, pick(Color32::from_rgb(0x16, 0x18, 0x1C), Color32::from_rgb(0x9A, 0xA0, 0xA8))), StrokeKind::Outside);
        if hot {
            p.rect_stroke(k.expand(2.5), 4.5, Stroke::new(1.0, pick(Color32::from_rgba_unmultiplied(0xE8, 0xEA, 0xEE, 120), Color32::from_rgba_unmultiplied(0x30, 0x34, 0x3A, 110))), StrokeKind::Outside);
        }
    } else {
        let k = pos2(kx, rail.center().y);
        p.circle_filled(k + vec2(0.0, 1.0), 10.5, tint(Color32::BLACK, 0.35));
        p.circle(k, if active { 11.0 } else { 9.5 }, Color32::from_rgb(0xF2, 0xF3, 0xF5), Stroke::new(if active { 2.5 } else { 0.0 }, accent()));
    }
    (changed, started && changed || resp.drag_started())
}

/// Number field in mm / ° / ×, at most 3 decimals, fixed width (the panel
/// never changes width with the value). A click types a value (Enter or a
/// click away keeps it, Esc drops it); a drag left / right with the left or
/// the middle button changes it (Shift: ten times finer).
pub fn value_field(ui: &mut Ui, v: &mut f32, range: std::ops::RangeInclusive<f32>, step: f64, suffix: &str, width: f32) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click_and_drag());
    let edit_key = resp.id.with("edit");
    let raw_key = resp.id.with("raw");
    let (lo, hi) = (*range.start(), *range.end());
    let text_of = |x: f32| if suffix.is_empty() { fmt3(x as f64) } else { format!("{} {suffix}", fmt3(x as f64)) };
    let editing: Option<String> = ui.data(|d| d.get_temp(edit_key));
    if let Some(mut text) = editing {
        let tid = resp.id.with("text");
        let te = ui.put(
            rect,
            egui::TextEdit::singleline(&mut text).id(tid).font(regular(12.5)).horizontal_align(egui::Align::RIGHT).margin(vec2(5.0, 2.0)),
        );
        if !te.has_focus() && !te.lost_focus() {
            te.request_focus();
        }
        let (esc, enter) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Enter)));
        if te.lost_focus() || enter {
            ui.data_mut(|d| d.remove::<String>(edit_key));
            if !esc {
                let t = text.trim().trim_end_matches(suffix).trim().replace(',', ".");
                if let Ok(x) = t.parse::<f64>() {
                    let nv = round3((x as f32).clamp(lo, hi));
                    if (nv - *v).abs() > 1e-7 {
                        *v = nv;
                        resp.mark_changed();
                    }
                }
            }
        } else {
            ui.data_mut(|d| d.insert_temp(edit_key, text));
        }
        return resp;
    }
    // a drag (left or middle button): the value follows the pointer across
    if resp.drag_started() {
        ui.data_mut(|d| d.insert_temp(raw_key, *v as f64));
    }
    if resp.dragged() {
        let (dx, fine) = ui.input(|i| (i.pointer.delta().x, i.modifiers.shift));
        let raw = ui.data(|d| d.get_temp::<f64>(raw_key)).unwrap_or(*v as f64) + dx as f64 * step * if fine { 0.1 } else { 1.0 };
        let raw = raw.clamp(lo as f64, hi as f64);
        ui.data_mut(|d| d.insert_temp(raw_key, raw));
        // on the field's own steps (Shift: a tenth of them), 3 decimals at most
        let q = if fine { step * 0.1 } else { step };
        let nv = round3(((raw / q).round() * q) as f32);
        if (nv - *v).abs() > 1e-7 {
            *v = nv;
            resp.mark_changed();
        }
    }
    if resp.clicked() {
        ui.data_mut(|d| d.insert_temp(edit_key, fmt3(*v as f64)));
        ui.ctx().request_repaint();
    }
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let p = ui.painter();
    let hot = resp.hovered() || resp.dragged();
    p.rect(rect, 6.0, if hot { RAISED() } else { WELL() }, Stroke::new(1.0, if resp.dragged() { TEXT2() } else { DIVIDER() }), StrokeKind::Inside);
    p.text(rect.right_center() - vec2(6.0, 0.0), Align2::RIGHT_CENTER, text_of(*v), regular(12.5), TEXT_STRONG());
    resp
}

/// label · slider · value. Returns (changed, gesture started).
pub fn slider_row(ui: &mut Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, step: f64, suffix: &str) -> (bool, bool) {
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
            ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, label, regular(12.5), LABEL());
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
        ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, label, regular(13.0), LABEL());
        let sw = (ui.available_width() - 70.0).max(40.0);
        let (min, max) = (*range.start(), *range.end());
        let adaptive = suffix == "mm" && min >= 0.0 && max > 20.0;
        let hint = if adaptive { 10.0 } else { max };
        out = touch_slider(ui, id, v, min, hint.min(max), adaptive, step as f32, sw);
        *v = v.clamp(min, max);
        let (vr, _) = ui.allocate_exact_size(vec2(62.0, 30.0), Sense::hover());
        let txt = if suffix.is_empty() { format!("{}", fmt_mm(*v)) } else { format!("{} {}", fmt_mm(*v), suffix) };
        ui.painter().text(vr.right_center(), Align2::RIGHT_CENTER, txt, regular(13.0), TEXT_STRONG());
    });
    out
}

pub fn toggle_row(ui: &mut Ui, label: &str, on: bool) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        // desktop: a small switch on a 28 pt row; iPad: 44 pt touch target
        let (row, tw, th, kr) = if DESK { (28.0, 34.0, 18.0, 7.0) } else { (44.0, 52.0, 32.0, 13.0) };
        ui.set_height(row);
        let (lr, _) = ui.allocate_exact_size(vec2(ui.available_width() - tw - ui.spacing().item_spacing.x, row - 4.0), Sense::hover());
        ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, label, regular(if DESK { 12.5 } else { 13.0 }), LABEL());
        let (r, resp) = ui.allocate_exact_size(vec2(tw, th), Sense::click());
        let p = ui.painter();
        p.rect_filled(r, th * 0.5, if on { accent() } else { RAISED() });
        let knob = if on { r.right_center() - vec2(th * 0.5, 0.0) } else { r.left_center() + vec2(th * 0.5, 0.0) };
        p.circle_filled(knob, kr, if on { on_accent() } else { TEXT2() });
        clicked = resp.clicked();
    });
    clicked
}

pub fn divider(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, PANEL_BORDER());
}

/// A floating island (graphite card, thin border).
pub fn island() -> Frame {
    Frame::new().fill(tint(PANEL(), 0.96)).stroke(Stroke::new(1.0, PANEL_BORDER())).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(5))
}

/// Compact icon + label toggle (28 pt), for the islands and the Studio view.
pub fn chip(ui: &mut Ui, icon: Option<Icon>, label: &str, on: bool, tip: &str) -> Response {
    let g = ui.painter().layout_no_wrap(label.to_string(), medium(12.5), TEXT());
    let w = g.size().x + 20.0 + if icon.is_some() { 20.0 } else { 0.0 };
    let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
    let p = ui.painter();
    let (bg, border, fg) = if on { (accent_bg(), accent_border(), accent_text()) } else if resp.hovered() { (RAISED(), DIVIDER(), TEXT()) } else { (Color32::TRANSPARENT, DIVIDER(), TEXT2()) };
    p.rect(r, 7.0, bg, Stroke::new(1.0, border), StrokeKind::Inside);
    let mut x = r.min.x + 10.0;
    if let Some(i) = icon {
        icons::paint(p, Rect::from_min_size(pos2(x, r.center().y - 7.5), vec2(15.0, 15.0)), i, fg);
        x += 20.0;
    }
    p.text(pos2(x, r.center().y), Align2::LEFT_CENTER, label, medium(12.5), fg);
    resp.on_hover_text(tip)
}

/// Inline segmented control sized by its labels (28 pt): (value, label, key hint).
pub fn seg_inline<T: PartialEq + Copy>(ui: &mut Ui, id: &str, items: &[(T, &str, &str)], current: T) -> Option<T> {
    let font = medium(12.5);
    let widths: Vec<f32> = items
        .iter()
        .map(|(_, l, k)| ui.painter().layout_no_wrap(l.to_string(), font.clone(), TEXT()).size().x + 22.0 + if k.is_empty() { 0.0 } else { 14.0 })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0 + 2.0 * (items.len() as f32 - 1.0);
    let (well, _) = ui.allocate_exact_size(vec2(total, 30.0), Sense::hover());
    ui.painter().rect(well, 8.0, WELL(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
    let mut x = well.min.x + 2.0;
    let mut picked = None;
    for (i, ((v, l, k), w)) in items.iter().zip(widths).enumerate() {
        let r = Rect::from_min_size(pos2(x, well.min.y + 2.0), vec2(w, 26.0));
        x += w + 2.0;
        let resp = ui.interact(r, Id::new((id, i)), Sense::click());
        let on = *v == current;
        let p = ui.painter();
        if on {
            p.rect(r, 6.0, RAISED(), Stroke::new(1.0, pick(Color32::from_rgb(0x35, 0x3A, 0x42), Color32::from_rgb(0xCD, 0xD2, 0xD8))), StrokeKind::Inside);
        } else if resp.hovered() {
            p.rect_filled(r, 6.0, tint(RAISED(), 0.6));
        }
        let tx = r.min.x + 11.0;
        let g = p.layout_no_wrap(l.to_string(), font.clone(), if on { TEXT() } else { TEXT2() });
        let gw = g.size().x;
        p.galley(pos2(tx, r.center().y - g.size().y * 0.5), g, TEXT());
        if !k.is_empty() {
            p.text(pos2(tx + gw + 5.0, r.center().y + 1.0), Align2::LEFT_CENTER, *k, medium(9.5), pick(Color32::from_rgb(0x64, 0x6B, 0x75), Color32::from_rgb(0x8A, 0x90, 0x99)));
        }
        if resp.clicked() {
            picked = Some(*v);
        }
    }
    picked
}

/// Small separator between groups of an island.
pub fn island_sep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(9.0, 20.0), Sense::hover());
    ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(1.0, 20.0)), 0.0, DIVIDER());
}

/// The shortcut letter, small, in the bottom-right corner of a tool button.
pub fn key_hint(ui: &Ui, r: &Response, key: &str) {
    if !DESK {
        return;
    }
    let at = r.rect.right_bottom() - vec2(4.0, 3.0);
    ui.painter().text(at, Align2::RIGHT_BOTTOM, key, medium(9.0), Color32::from_rgb(0x8A, 0x8F, 0x97));
}

// ------------------------------------------------------------ panel grips

/// Where the user moved a floating panel (offset from its place), kept
/// between sessions in egui's memory.
pub fn panel_offset(ctx: &egui::Context, key: &str) -> egui::Vec2 {
    ctx.data_mut(|d| d.get_persisted::<egui::Vec2>(Id::new(("panel-offset", key)))).unwrap_or(egui::Vec2::ZERO)
}

/// A grip on top of a floating panel: drag it to move the panel, double-click
/// to put it back. Six dots, lit when hovered.
pub fn grip(ui: &mut Ui, key: &str) {
    // (a wide panel such as the history strip keeps its own width)
    let w = ui.available_width().min(400.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, 12.0), Sense::hover());
    let hit = Rect::from_center_size(r.center(), vec2(64.0, 14.0));
    let resp = ui.interact(hit, Id::new(("panel-grip", key)), Sense::click_and_drag());
    let id = Id::new(("panel-offset", key));
    if resp.dragged() {
        let d = resp.drag_delta();
        ui.ctx().data_mut(|m| {
            let o = m.get_persisted::<egui::Vec2>(id).unwrap_or(egui::Vec2::ZERO);
            m.insert_persisted(id, o + d);
        });
    }
    if resp.double_clicked() {
        ui.ctx().data_mut(|m| m.insert_persisted(id, egui::Vec2::ZERO));
    }
    let lit = resp.hovered() || resp.dragged();
    let c = if lit { TEXT() } else { pick(Color32::from_rgb(0x5A, 0x61, 0x6B), Color32::from_rgb(0xA2, 0xA8, 0xB0)) };
    for row in [-2.5f32, 2.5] {
        for col in [-8.0f32, 0.0, 8.0] {
            ui.painter().circle_filled(r.center() + vec2(col, row), 1.6, c);
        }
    }
    let resp = resp.on_hover_text("Drag to move the panel · double-click to put it back");
    if lit {
        ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
    }
}
pub fn chip_label(ui: &mut Ui, text: &str, bg: Color32, fg: Color32) {
    let g = ui.painter().layout_no_wrap(text.to_string(), medium(11.5), fg);
    let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 14.0, 20.0), Sense::hover());
    ui.painter().rect_filled(r, 6.0, bg);
    ui.painter().galley(pos2(r.min.x + 7.0, r.center().y - g.size().y * 0.5), g, fg);
}

pub fn dim(ctx: &egui::Context, screen: Rect, id: &str) -> bool {
    let mut close = false;
    egui::Area::new(Id::new(id)).order(Order::Foreground).fade_in(false).fixed_pos(screen.min).show(ctx, |ui| {
        let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
        ui.painter().rect_filled(r, 0.0, tint(Color32::BLACK, 0.45));
        close = resp.clicked();
    });
    close
}

