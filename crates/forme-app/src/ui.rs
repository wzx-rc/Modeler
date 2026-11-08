//! Panels, laid out like the mock-up: top bar, tool rail, mode bar, thumb Carve
//! button, properties + shape tree on the right, dialogs and toasts.

use crate::camera::ViewPreset;
use crate::files;
use crate::icons::{self, Icon};
use crate::kit::KitPiece;
use crate::state::*;
use crate::theme::*;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Frame, Id, Margin, Order, Rect, Response, Sense, Stroke, StrokeKind, Ui};
use forme_core::*;
use glam::{EulerRot, Quat, Vec3};

const RIGHT_W: f32 = 296.0;

fn panel() -> Frame {
    Frame::new().fill(PANEL).stroke(Stroke::new(1.0, PANEL_BORDER)).corner_radius(CornerRadius::same(16)).inner_margin(Margin::same(8))
}

fn mode_color(m: CombineMode) -> Color32 {
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
    icons::paint(p, Rect::from_center_size(rect.center(), vec2(20.0, 20.0)), icon, c);
    resp.on_hover_text(tip)
}

fn toggle_chip(ui: &mut Ui, icon: Icon, label: &str, on: bool) -> Response {
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

fn primary_btn(ui: &mut Ui, icon: Option<Icon>, label: &str, width: Option<f32>) -> Response {
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

fn secondary_btn(ui: &mut Ui, label: &str, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 44.0), Sense::click());
    let p = ui.painter();
    let fill = if resp.hovered() { RAISED } else { Color32::TRANSPARENT };
    p.rect(rect.shrink(0.5), 10.0, fill, Stroke::new(1.0, DIVIDER), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, label, medium(14.0), LABEL);
    resp
}

/// Segmented control. Returns the picked value.
fn segmented<T: PartialEq + Copy>(ui: &mut Ui, items: &[(T, &str, Color32)], current: T, height: f32) -> Option<T> {
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
        p.text(r.center(), Align2::CENTER_CENTER, *label, semibold(12.0), if on { TEXT_STRONG } else { Color32::from_rgb(0xAE, 0xB3, 0xBA) });
        if resp.clicked() {
            picked = Some(*v);
        }
    }
    picked
}

fn section(ui: &mut Ui, title: &str) {
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
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 34.0), Sense::click_and_drag());
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
    let rail = Rect::from_center_size(rect.center(), vec2(width - 20.0, 4.0));
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
    p.circle_filled(k + vec2(0.0, 1.0), 10.5, tint(Color32::BLACK, 0.35));
    p.circle(k, if active { 11.0 } else { 9.5 }, Color32::from_rgb(0xF2, 0xF3, 0xF5), Stroke::new(if active { 3.0 } else { 0.0 }, ACCENT));
    (changed, started && changed || resp.drag_started())
}

/// label · slider · value. Returns (changed, gesture started).
fn slider_row(ui: &mut Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, step: f64, suffix: &str) -> (bool, bool) {
    let mut out = (false, false);
    let id = ui.make_persistent_id(("slider", label));
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
        let mut btn = |ui: &mut Ui, t: &str| {
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

fn toggle_row(ui: &mut Ui, label: &str, on: bool) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.set_height(44.0);
        let (lr, _) = ui.allocate_exact_size(vec2(ui.available_width() - 56.0, 40.0), Sense::hover());
        ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, label, regular(13.0), LABEL);
        let (r, resp) = ui.allocate_exact_size(vec2(52.0, 32.0), Sense::click());
        let p = ui.painter();
        p.rect_filled(r, 16.0, if on { ACCENT } else { RAISED });
        let knob = if on { r.right_center() - vec2(16.0, 0.0) } else { r.left_center() + vec2(16.0, 0.0) };
        p.circle_filled(knob, 13.0, if on { ON_ACCENT } else { TEXT2 });
        clicked = resp.clicked();
    });
    clicked
}

fn divider(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, PANEL_BORDER);
}

// ------------------------------------------------------------------ layout

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    st.screen = [screen.width(), screen.height()];
    top_bar(ctx, st, screen);
    rail(ctx, st);
    if matches!(st.tool, Tool::Kit(_)) || st.kit_open {
        kit_popup(ctx, st);
    }
    mode_bar(ctx, st, screen);
    thumb_button(ctx, st, screen);
    hud(ctx, st);
    right_column(ctx, st, screen);
    toast(ctx, st, screen);
    if st.export_open {
        export_dialog(ctx, st, screen);
    }
    if st.files_open {
        files_dialog(ctx, st, screen);
    }
    shortcuts(ctx, st);
}

fn shortcuts(ctx: &egui::Context, st: &mut AppState) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let (undo, redo, del, dup, esc, enter, alt, f) = ctx.input(|i| {
        let cmd = i.modifiers.command;
        (
            cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
            cmd && (i.key_pressed(egui::Key::Y) || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
            i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
            cmd && i.key_pressed(egui::Key::D),
            i.key_pressed(egui::Key::Escape),
            i.key_pressed(egui::Key::Enter),
            i.modifiers.alt,
            i.key_pressed(egui::Key::F),
        )
    });
    if undo {
        st.undo();
    }
    if redo {
        st.redo();
    }
    if del {
        st.delete_selected();
    }
    if dup {
        st.duplicate_selected();
    }
    if esc {
        st.commit_placement();
        st.cancel_profile();
        st.tool = Tool::Select;
    }
    if enter {
        if st.profile.is_some() {
            st.close_profile();
        }
        st.commit_placement();
    }
    if f {
        st.frame_selection();
    }
    // Alt = keyboard version of the thumb button
    if alt != st.carve_held && st.gesture == Gesture::None {
        st.carve_held = alt;
    }
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
            if icon_btn(ui, Icon::Undo, false, "Annuler (tap à 2 doigts)", 44.0).clicked() {
                st.undo();
            }
            if icon_btn(ui, Icon::Redo, false, "Rétablir (tap à 3 doigts)", 44.0).clicked() {
                st.redo();
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
                if toggle_chip(ui, Icon::Mirror, "Symétrie X", st.sym_x).clicked() {
                    st.sym_x = !st.sym_x;
                    st.toast(if st.sym_x { "Symétrie X : les nouvelles formes sont doublées" } else { "Symétrie X désactivée" });
                }
                if toggle_chip(ui, Icon::Magnet, "Snap", st.snap).clicked() {
                    st.snap = !st.snap;
                }
                if toggle_chip(ui, Icon::Grid, "Grille", st.grid).clicked() {
                    st.grid = !st.grid;
                    st.view_changed();
                }
                let (sep, _) = ui.allocate_exact_size(vec2(9.0, 24.0), Sense::hover());
                ui.painter().rect_filled(Rect::from_center_size(sep.center(), vec2(1.0, 24.0)), 0.0, DIVIDER);
                if primary_btn(ui, Some(Icon::Export), "Exporter", None).clicked() {
                    st.export_open = !st.export_open;
                }
            });
        });
}

fn rail(ctx: &egui::Context, st: &mut AppState) {
    egui::Area::new(Id::new("rail")).order(Order::Middle).fixed_pos(pos2(12.0, 80.0)).show(ctx, |ui| {
        panel().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.set_width(44.0);
            if icon_btn(ui, Icon::Select, st.tool == Tool::Select, "Sélection", 44.0).clicked() {
                st.commit_placement();
                st.tool = Tool::Select;
            }
            divider(ui);
            for k in PrimKind::PLACEABLE {
                if icon_btn(ui, prim_icon(k), st.tool == Tool::Place(k), k.label(), 44.0).clicked() {
                    st.commit_placement();
                    st.tool = Tool::Place(k);
                    st.kit_open = false;
                }
            }
            divider(ui);
            if icon_btn(ui, Icon::Profile, st.tool == Tool::Profile, "Profil (dessiner un contour)", 44.0).clicked() {
                st.commit_placement();
                st.tool = Tool::Profile;
                st.ensure_profile();
                st.kit_open = false;
            }
            let kit_on = matches!(st.tool, Tool::Kit(_));
            if icon_btn(ui, Icon::Kit, kit_on, "Kit de pièces (vis, rivets…)", 44.0).clicked() {
                st.commit_placement();
                if !kit_on {
                    st.tool = Tool::Kit(KitPiece::Screw);
                }
                st.kit_open = !st.kit_open || !kit_on;
            }
            let rep_on = st.selection.and_then(|id| st.scene.node(id)).map(|n| n.repeat != Repeat::None).unwrap_or(false);
            if icon_btn(ui, Icon::Repeat, rep_on, "Répétition radiale", 44.0).clicked() {
                st.toggle_repeat();
            }
        });
    });
}

fn kit_popup(ctx: &egui::Context, st: &mut AppState) {
    egui::Area::new(Id::new("kit")).order(Order::Foreground).fixed_pos(pos2(84.0, 560.0)).show(ctx, |ui| {
        panel().inner_margin(Margin::same(10)).show(ui, |ui| {
            ui.set_width(196.0);
            ui.label(egui::RichText::new("Kit de pièces").font(semibold(13.0)).color(TEXT));
            for piece in KitPiece::ALL {
                let on = st.tool == Tool::Kit(piece);
                let (r, resp) = ui.allocate_exact_size(vec2(196.0, 40.0), Sense::click());
                let p = ui.painter();
                if on {
                    p.rect(r, 9.0, ACCENT_BG, Stroke::new(1.0, ACCENT_BORDER), StrokeKind::Inside);
                } else if resp.hovered() {
                    p.rect_filled(r, 9.0, RAISED);
                }
                icons::paint(p, Rect::from_min_size(r.min + vec2(10.0, 11.0), vec2(18.0, 18.0)), Icon::Kit, if on { ACCENT_TEXT } else { TEXT_ICON });
                p.text(r.min + vec2(38.0, 20.0), Align2::LEFT_CENTER, piece.label(), medium(13.0), if on { ACCENT_TEXT } else { TEXT });
                if resp.clicked() {
                    st.tool = Tool::Kit(piece);
                }
            }
        });
    });
}

fn mode_bar(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let cx = st.principal()[0];
    egui::Area::new(Id::new("modes"))
        .order(Order::Middle)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(pos2(cx, screen.max.y - 18.0))
        .show(ctx, |ui| {
            panel().inner_margin(Margin::same(6)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let current = st.effective_mode();
                    let target_mode = st.selection.and_then(|id| st.scene.node(id)).filter(|n| n.shape().is_some()).map(|n| n.combine.mode);
                    let shown = if st.carve_held { current } else { target_mode.unwrap_or(current) };
                    for m in CombineMode::ALL {
                        let g = ui.painter().layout_no_wrap(m.label().to_string(), semibold(14.0), TEXT);
                        let w = (g.size().x + 44.0).max(92.0);
                        let (r, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
                        let on = shown == m;
                        let c = mode_color(m);
                        let p = ui.painter();
                        if on {
                            p.rect(r, 11.0, tint(c, 0.16), Stroke::new(1.5, c), StrokeKind::Inside);
                        } else if resp.hovered() {
                            p.rect_filled(r, 11.0, RAISED);
                        }
                        let total = 14.0 + 8.0 + g.size().x;
                        let x0 = r.center().x - total * 0.5;
                        paint_mode_glyph(p, pos2(x0 + 7.0, r.center().y), m, c, 13.0);
                        p.galley(pos2(x0 + 22.0, r.center().y - g.size().y * 0.5), g, if on { TEXT_STRONG } else { Color32::from_rgb(0xAE, 0xB3, 0xBA) });
                        if resp.clicked() {
                            st.set_mode(m);
                        }
                    }
                });
            });
        });
}

fn thumb_button(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    egui::Area::new(Id::new("thumb")).order(Order::Middle).fixed_pos(pos2(12.0, screen.max.y - 102.0)).show(ctx, |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::click_and_drag());
        let held = resp.is_pointer_button_down_on() || (st.carve_held && st.gesture != Gesture::None) || st.carve_held;
        if resp.is_pointer_button_down_on() {
            st.carve_held = true;
        } else if resp.drag_stopped() || resp.clicked() {
            st.carve_held = false;
        }
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

fn hud(ctx: &egui::Context, st: &mut AppState) {
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

fn toast(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
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
    let tree_h = if st.show_tree { 268.0 } else { 0.0 };
    let props_max = screen.height() - 80.0 - 12.0 - if st.show_tree { tree_h + 12.0 } else { 0.0 };
    egui::Area::new(Id::new("props")).order(Order::Middle).fixed_pos(pos2(x, 80.0)).show(ctx, |ui| {
        panel().inner_margin(Margin::same(14)).show(ui, |ui| {
            ui.set_width(RIGHT_W - 28.0);
            egui::ScrollArea::vertical().max_height(props_max - 28.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.set_width(RIGHT_W - 28.0);
                if st.tool == Tool::Profile {
                    profile_panel(ui, st);
                } else if st.selection.and_then(|id| st.scene.node(id)).is_some() {
                    properties(ui, st);
                } else {
                    scene_panel(ui, st);
                }
            });
        });
    });
    if st.show_tree {
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
                    let r = ui.add(egui::Label::new(egui::RichText::new(&node.name).font(semibold(17.0)).color(TEXT)).sense(Sense::click()).truncate());
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
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if icon_btn(ui, Icon::Duplicate, false, "Dupliquer", 44.0).clicked() {
            st.duplicate_selected();
        }
        if icon_btn(ui, if node.visible { Icon::Eye } else { Icon::EyeOff }, false, "Masquer", 44.0).clicked() {
            st.toggle_visible(id);
        }
        if icon_btn(ui, Icon::Frame, false, "Cadrer", 44.0).clicked() {
            st.frame_selection();
        }
        if icon_btn(ui, Icon::Trash, false, "Supprimer", 44.0).clicked() {
            st.delete_selected();
            return;
        }
    });
    if st.selection.is_none() {
        return;
    }
    divider(ui);

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
        let items: Vec<(CombineMode, &str, Color32)> = CombineMode::ALL
            .iter()
            .map(|m| (*m, if *m == CombineMode::Intersect { "Inter." } else { m.label() }, mode_color(*m)))
            .collect();
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
        if matches!(node.combine.mode, CombineMode::Groove | CombineMode::Tongue) {
            let mut d = node.combine.depth;
            let (ch, start) = slider_row(ui, "Profondeur", &mut d, 0.1..=8.0, 0.1, "mm");
            if ch {
                st.edit_live("Profondeur", id, start, |n| n.combine.depth = d);
            }
            let mut w = node.combine.width;
            let (ch, start) = slider_row(ui, "Largeur", &mut w, 0.1..=6.0, 0.1, "mm");
            if ch {
                st.edit_live("Largeur", id, start, |n| n.combine.width = w);
            }
        }
    }

    section(ui, "Organisation");
    let kinds = [(0u8, "Aucune", ACCENT), (1u8, "Radiale", ACCENT), (2u8, "Linéaire", ACCENT)];
    let cur = match node.repeat {
        Repeat::None => 0,
        Repeat::Radial { .. } => 1,
        Repeat::Linear { .. } => 2,
    };
    if let Some(k) = segmented(ui, &kinds, cur, 40.0) {
        let r = match k {
            1 => Repeat::Radial { count: 4, axis: Axis::Y },
            2 => Repeat::Linear { count: 3, step: Vec3::new(10.0, 0.0, 0.0) },
            _ => Repeat::None,
        };
        st.edit("Répétition", id, |n| n.repeat = r);
    }
    match node.repeat {
        Repeat::Radial { count, axis } => {
            if let Some(c) = stepper(ui, "Copies", count, 2, 48) {
                st.edit("Copies", id, |n| n.repeat = Repeat::Radial { count: c, axis });
            }
            let axes = [(Axis::X, "Axe X", AXIS_X), (Axis::Y, "Axe Y", AXIS_Y), (Axis::Z, "Axe Z", AXIS_Z)];
            if let Some(a) = segmented(ui, &axes, axis, 36.0) {
                st.edit("Axe", id, |n| n.repeat = Repeat::Radial { count, axis: a });
            }
        }
        Repeat::Linear { count, step } => {
            if let Some(c) = stepper(ui, "Copies", count, 2, 64) {
                st.edit("Copies", id, |n| n.repeat = Repeat::Linear { count: c, step });
            }
            let a = step.abs().max_position();
            let mut len = step.length();
            let (ch, start) = slider_row(ui, "Pas", &mut len, 0.5..=80.0, 0.5, "mm");
            if ch {
                let dir = step.normalize_or(Vec3::X);
                st.edit_live("Pas", id, start, |n| n.repeat = Repeat::Linear { count, step: dir * len });
            }
            let axes = [(0usize, "X", AXIS_X), (1usize, "Y", AXIS_Y), (2usize, "Z", AXIS_Z)];
            if let Some(na) = segmented(ui, &axes, a, 36.0) {
                let mut v = Vec3::ZERO;
                v[na] = len.max(1.0);
                st.edit("Axe", id, |n| n.repeat = Repeat::Linear { count, step: v });
            }
        }
        Repeat::None => {}
    }
    ui.horizontal(|ui| {
        ui.set_height(40.0);
        let (lr, _) = ui.allocate_exact_size(vec2(84.0, 36.0), Sense::hover());
        ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, "Symétrie", regular(13.0), LABEL);
        let m = node.mirror;
        let items = [(0, "X", m.x), (1, "Y", m.y), (2, "Z", m.z)];
        for (i, l, on) in items {
            let (r, resp) = ui.allocate_exact_size(vec2(52.0, 36.0), Sense::click());
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

    section(ui, "Position et rotation");
    let mut pos = node.transform.pos;
    let (ex, ey, ez) = node.transform.rot.to_euler(EulerRot::XYZ);
    let mut rot = Vec3::new(ex.to_degrees(), ey.to_degrees(), ez.to_degrees());
    let mut changed_pos = false;
    let mut changed_rot = false;
    ui.horizontal(|ui| {
        for (i, (l, c)) in [("X", AXIS_X), ("Y", AXIS_Y), ("Z", AXIS_Z)].iter().enumerate() {
            ui.label(egui::RichText::new(*l).font(semibold(12.0)).color(*c));
            changed_pos |= ui.add(egui::DragValue::new(&mut pos[i]).speed(0.25).max_decimals(1).suffix(" mm")).changed();
        }
    });
    ui.horizontal(|ui| {
        for (i, (l, c)) in [("rX", AXIS_X), ("rY", AXIS_Y), ("rZ", AXIS_Z)].iter().enumerate() {
            ui.label(egui::RichText::new(*l).font(semibold(12.0)).color(*c));
            changed_rot |= ui.add(egui::DragValue::new(&mut rot[i]).speed(1.0).max_decimals(0).suffix("°")).changed();
        }
    });
    if changed_pos {
        st.edit("Position", id, |n| n.transform.pos = pos);
    }
    if changed_rot {
        let q = Quat::from_euler(EulerRot::XYZ, rot.x.to_radians(), rot.y.to_radians(), rot.z.to_radians());
        st.edit("Rotation", id, |n| n.transform.rot = q);
    }

    section(ui, "Matière");
    material_palette(ui, st, Some(id), node.material);
}

fn chip_label(ui: &mut Ui, text: &str, bg: Color32, fg: Color32) {
    let g = ui.painter().layout_no_wrap(text.to_string(), medium(11.5), fg);
    let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 14.0, 20.0), Sense::hover());
    ui.painter().rect_filled(r, 6.0, bg);
    ui.painter().galley(pos2(r.min.x + 7.0, r.center().y - g.size().y * 0.5), g, fg);
}

fn material_palette(ui: &mut Ui, st: &mut AppState, target: Option<NodeId>, current: u32) {
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
            p.text(r.min + vec2(32.0, 20.0), Align2::LEFT_CENTER, name, regular(12.0), if on { ACCENT_TEXT } else { LABEL });
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
    if !matches!(sh.kind, PrimKind::Sphere | PrimKind::Capsule | PrimKind::Torus) {
        row(ui, "Rondeur", &mut s.round, 0.0..=max_round.max(0.1), 0.1, 1.0, "mm");
    }
    if matches!(sh.kind, PrimKind::Box | PrimKind::Cylinder) {
        row(ui, "Chanfrein", &mut s.chamfer, 0.0..=max_round.max(0.1), 0.1, 1.0, "mm");
    }
    if !matches!(sh.kind, PrimKind::Sphere | PrimKind::Torus | PrimKind::Profile) {
        row(ui, "Taper", &mut s.taper, -0.9..=0.9, 0.01, 1.0, "");
    }
    row(ui, "Coque", &mut s.shell, 0.0..=10.0, 0.1, 1.0, "mm");
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
    let items: Vec<(CombineMode, &str, Color32)> = CombineMode::ALL
        .iter()
        .map(|m| (*m, if *m == CombineMode::Intersect { "Inter." } else { m.label() }, mode_color(*m)))
        .collect();
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

fn profile_panel(ui: &mut Ui, st: &mut AppState) {
    let n = st.profile.as_ref().map(|d| d.points.len()).unwrap_or(0);
    ui.label(egui::RichText::new("Nouveau profil").font(regular(12.0)).color(TEXT2));
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Contour").font(semibold(17.0)).color(TEXT));
        chip_label(ui, &format!("{n} points · ouvert"), RAISED, LABEL);
    });
    divider(ui);
    section(ui, "Tracé");
    for (smooth, text) in [(true, "Glisser : point de courbe"), (false, "Tap : point d'angle")] {
        ui.horizontal(|ui| {
            ui.set_height(28.0);
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            if smooth {
                ui.painter().circle_filled(r.center(), 5.0, TEXT);
            } else {
                ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(10.0, 10.0)), 0.0, TEXT);
            }
            ui.label(egui::RichText::new(text).color(LABEL));
        });
    }
    if toggle_row(ui, "Snap demi-millimètre", st.snap) {
        st.snap = !st.snap;
    }
    let Some(d) = st.profile.as_mut() else { return };
    section(ui, "Opération");
    let modes = [(ProfileMode::Extrude, "Extruder", ACCENT), (ProfileMode::Revolve, "Révolution", ACCENT)];
    if let Some(m) = segmented(ui, &modes, d.mode, 40.0) {
        d.mode = m;
    }
    let mut t = d.thickness * 2.0;
    if slider_row(ui, "Épaisseur", &mut t, 0.5..=60.0, 0.5, "mm").0 {
        d.thickness = t * 0.5;
    }
    let mut r = d.round;
    if slider_row(ui, "Rondeur", &mut r, 0.0..=5.0, 0.1, "mm").0 {
        d.round = r;
    }
    let sym = d.symmetric;
    if toggle_row(ui, "Symétrique au plan", sym) {
        d.symmetric = !sym;
    }
    ui.add_space(6.0);
    let w = RIGHT_W - 28.0;
    if primary_btn(ui, None, "Fermer et extruder", Some(w)).clicked() {
        st.close_profile();
    }
    if secondary_btn(ui, "Annuler le dernier point", w).clicked() {
        st.profile_undo_point();
    }
    if secondary_btn(ui, "Effacer le tracé", w).clicked() {
        st.cancel_profile();
        st.ensure_profile();
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
            let selected = st.selection == Some(id);
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
            p.text(pos2(x0 + 44.0, r.center().y), Align2::LEFT_CENTER, &n.name, if selected { semibold(13.0) } else { regular(13.0) }, name_color);
            let mut info = n.type_label();
            if n.repeat.count() > 1 {
                info += &format!(" ×{}", n.repeat.count());
            }
            if n.mirror.any() {
                info += " ⇋";
            }
            p.text(pos2(r.max.x - 40.0, r.center().y), Align2::RIGHT_CENTER, info, regular(12.0), if selected { Color32::from_rgb(0xC3, 0xD9, 0xF2) } else { tint(TEXT2, dim) });
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
                st.select(Some(id));
            }
        }
    });
}

// ----------------------------------------------------------------- dialogs

fn dim(ctx: &egui::Context, screen: Rect, id: &str) -> bool {
    let mut close = false;
    egui::Area::new(Id::new(id)).order(Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
        let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
        ui.painter().rect_filled(r, 0.0, tint(Color32::BLACK, 0.45));
        close = resp.clicked();
    });
    close
}

fn export_dialog(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
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

fn files_dialog(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
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
