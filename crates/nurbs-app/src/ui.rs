//! The interface, the look of the first Forme NURBS (Établi): the view
//! first, floating islands on top (document, tool and Add / Carve, view),
//! the tool column on the right, the scene and the selected shape on the
//! left, the history strip at the bottom.

use crate::camera::ViewPreset;
use crate::files;
use crate::icons::{self, Icon};
use crate::materials;
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
/// The fixed Sub Outliner's list height (about five rows).
const SUB_FIXED_H: f32 = 140.0;

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

/// A feature's icon: its primitive, or the fillet's.
pub fn feature_icon(f: &Feature) -> Icon {
    if let Some(k) = &f.cut {
        if k.is_delete() {
            return Icon::Trash;
        }
        return if k.segs.is_empty() { Icon::EdgePoint } else { Icon::Knife };
    }
    if f.tweak.is_some() {
        return Icon::Move;
    }
    match (&f.round, &f.push) {
        (Some(_), _) => Icon::Fillet,
        (_, Some(p)) if p.inset > 0.0 => Icon::Inset,
        (_, Some(_)) => Icon::Extrude,
        _ => prim_icon(f.prim.kind()),
    }
}

/// What a step does, in a few words: "Fillet · 2 mm", "Inset · 2 mm · −3 mm".
pub fn step_info(f: &Feature) -> String {
    if let Some(k) = &f.cut {
        if !k.remove_faces.is_empty() {
            return format!("{} face{}", k.remove_faces.len(), if k.remove_faces.len() > 1 { "s" } else { "" });
        }
        if !k.remove_edges.is_empty() {
            return format!("{} edge{}", k.remove_edges.len(), if k.remove_edges.len() > 1 { "s" } else { "" });
        }
        let (n, l) = (k.pts.len(), k.segs.len());
        return if l > 0 { format!("{l} line{}", if l > 1 { "s" } else { "" }) } else { format!("{n} point{}", if n > 1 { "s" } else { "" }) };
    }
    if let Some(t) = &f.tweak {
        return format!("{} mm", fmt_mm(t.delta.length() as f32));
    }
    match (&f.round, &f.push) {
        (Some(r), _) => format!("{} mm", fmt_mm(r.size as f32)),
        (_, Some(p)) if p.inset > 0.0 && p.depth != 0.0 => format!("{} · {} mm", fmt_mm(p.inset as f32), fmt_signed(p.depth)),
        (_, Some(p)) if p.inset > 0.0 => format!("{} mm", fmt_mm(p.inset as f32)),
        (_, Some(p)) => format!("{} mm", fmt_signed(p.depth)),
        _ => f.prim.kind().label().to_string(),
    }
}

fn fmt_signed(v: f64) -> String {
    if v > 0.0 { format!("+{}", fmt_mm(v as f32)) } else if v < 0.0 { format!("−{}", fmt_mm(-v as f32)) } else { "0".into() }
}

/// A feature's colour: Add, Carve, or the edges' yellow for a fillet.
pub fn feature_color(f: &Feature) -> Color32 {
    if f.is_solid() {
        op_color(f.op)
    } else if f.push.is_some() || f.tweak.is_some() {
        FACE_SEL
    } else {
        EDGE_SEL
    }
}

pub fn prim_icon(k: PrimKind) -> Icon {
    match k {
        PrimKind::Box => Icon::Box,
        PrimKind::Cylinder => Icon::Cylinder,
        PrimKind::Profile => Icon::Sketch,
        PrimKind::Path | PrimKind::Surface => Icon::Path,
    }
}

pub fn prim_key(k: PrimKind) -> &'static str {
    match k {
        PrimKind::Box => "B",
        PrimKind::Cylinder => "C",
        PrimKind::Profile => "L",
        PrimKind::Path => "P",
        PrimKind::Surface => "U",
    }
}

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    let left = if st.panel_visible { 2.0 * EDGE + LEFT_W } else { EDGE };
    st.view_rect = [left, EDGE + ISLAND_H, screen.width() - 2.0 * EDGE - CARDS_W, screen.height() - HISTORY_H];
    top_bar(ctx, st, screen);
    rail(ctx, st, screen);
    materials_island(ctx, st);
    history_strip(ctx, st, screen);
    help_box(ctx, st);
    deviation_legend(ctx, st);
    if st.panel_visible {
        inspector(ctx, st, screen);
    }
    context_bar(ctx, st);
    op_window(ctx, st);
    files_dialog(ctx, st, screen);
    toast(ctx, st, screen);
    shortcuts(ctx, st);
    if st.tool != Tool::Select {
        st.last_tool = Some(st.tool);
    }
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
                let n = st.doc.features.iter().filter(|f| f.is_solid()).count();
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
                mode_pill(ui, st);
                island_sep(ui);
                let tool = match st.tool {
                    Tool::Create(k) => k.label().to_uppercase(),
                    Tool::Push(k) => k.label().to_uppercase(),
                    Tool::Knife => "KNIFE".into(),
                    Tool::Point => "POINT ON EDGE".into(),
                    Tool::Slot => "SLOT · ON THE SCAN".into(),
                    Tool::Select => format!("SELECTION · {}", st.sel_mode.label().to_uppercase()),
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
                // Edge and Vertex modes: what a drag on an edge or a sharp
                // corner makes (Rectify's option)
                if st.tool == Tool::Select && matches!(st.sel_mode, SelMode::Edge | SelMode::Vertex) {
                    island_sep(ui);
                    let (well, _) = ui.allocate_exact_size(vec2(2.0 * 72.0 + 6.0, 28.0), Sense::hover());
                    ui.painter().rect(well, 14.0, WELL(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
                    for (i, (c, name)) in [(false, "FILLET"), (true, "CHAMFER")].into_iter().enumerate() {
                        let r = Rect::from_min_size(well.min + vec2(3.0 + i as f32 * 72.0, 3.0), vec2(72.0, 22.0));
                        let resp = ui.interact(r, Id::new(("isl-cut", i)), Sense::click());
                        let on = st.round_chamfer == c;
                        if on {
                            ui.painter().rect_filled(r, 11.0, EDGE_SEL);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(r, 11.0, RAISED());
                        }
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, name, mono(11.0), if on { on_accent() } else { TEXT2() });
                        let tip = if c { "A drag on an edge or a sharp corner chamfers it (Ctrl + B: chamfer what is picked)" } else { "A drag on an edge or a sharp corner rounds it" };
                        if resp.on_hover_text(tip).clicked() {
                            st.round_chamfer = c;
                        }
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
                if icon_btn(ui, Icon::Xray, st.xray, "X-ray (Alt + X): the part see-through, the scan behind it shown", 28.0).clicked() {
                    st.xray = !st.xray;
                }
                if icon_btn(ui, Icon::Quads, st.quads, "Quads view, face by face: each B-rep face cut into quad patches, fillets in orange", 28.0).clicked() {
                    st.set_quads_view(!st.quads);
                }
                if icon_btn(ui, Icon::Remesh, st.remesh_on, "Remesh view (QuadRemesher style): the whole part remeshed in quads that follow its edges and fillets, made in the background", 28.0).clicked() {
                    st.set_remesh_view(!st.remesh_on);
                }
                if st.remesh_on {
                    // QuadRemesher's Target Quad Count: a click goes to the next
                    const TARGETS: [usize; 6] = [500, 1000, 1500, 3000, 6000, 12000];
                    let t = st.remesh_target;
                    let label = if t >= 1000 { format!("{}k", fmt_mm(t as f32 / 1000.0)) } else { t.to_string() };
                    let g = ui.painter().layout_no_wrap(label, mono(11.0), TEXT());
                    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 16.0, 28.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 7.0, RAISED());
                    }
                    ui.painter().galley(r.center() - g.size() * 0.5, g, TEXT());
                    let resp = resp.on_hover_text("Target quad count (about): click for more, right click for fewer");
                    let i = TARGETS.iter().position(|x| *x == t).unwrap_or(2);
                    if resp.clicked() {
                        st.set_remesh_target(TARGETS[(i + 1) % TARGETS.len()]);
                    } else if resp.secondary_clicked() {
                        st.set_remesh_target(TARGETS[(i + TARGETS.len() - 1) % TARGETS.len()]);
                    }
                }
                if (st.quads || st.remesh_on) && icon_btn(ui, Icon::Export, false, if st.remesh_on { "Export the remesh as .obj (Documents/Forme NURBS, <name>-remesh.obj)" } else { "Export the quads as .obj (Documents/Forme NURBS)" }, 28.0).clicked() {
                    st.export_obj();
                }
                if icon_btn(ui, Icon::Light, st.settings.light, "Light theme", 28.0).clicked() {
                    st.settings.light = !st.settings.light;
                    st.save_settings();
                }
            });
        });
    });
}

/// The component modes (like Plasticity): Vertex, Edge, Face, Object, the
/// current one lit (with the Select tool).
fn mode_pill(ui: &mut Ui, st: &mut AppState) {
    let n = SelMode::ALL.len() as f32;
    let (well, _) = ui.allocate_exact_size(vec2(n * 30.0 + 6.0, 30.0), Sense::hover());
    ui.painter().rect(well, 9.0, WELL(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
    for (i, m) in SelMode::ALL.into_iter().enumerate() {
        let r = Rect::from_min_size(well.min + vec2(3.0 + i as f32 * 30.0, 3.0), vec2(30.0, 24.0));
        let resp = ui.interact(r, Id::new(("isl-selmode", i)), Sense::click());
        let on = st.sel_mode == m && st.tool == Tool::Select;
        let lit = st.sel_mode == m;
        let p = ui.painter();
        if on {
            p.rect_filled(r, 7.0, accent());
        } else if lit {
            p.rect(r, 7.0, tint(accent(), 0.12), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
        } else if resp.hovered() {
            p.rect_filled(r, 7.0, RAISED());
        }
        let icon = [Icon::ModeVertex, Icon::ModeEdge, Icon::ModeFace, Icon::ModeObject][i];
        icons::paint(p, Rect::from_center_size(r.center(), vec2(17.0, 17.0)), icon, if on { on_accent() } else if lit { accent() } else { TEXT_ICON() });
        let tip = match m {
            SelMode::Vertex => "Vertex (1 or F9): pick the part's corners · drag a rectangle to pick several",
            SelMode::Edge => "Edge (2 or F10): pick edges to fillet or chamfer · drag a rectangle to pick several",
            SelMode::Face => "Face (3 or F11): pick faces to inset (I) or extrude (Ctrl + E) · drag a rectangle to pick several",
            SelMode::Object => "Object (4, Q or F8): pick and move whole shapes",
        };
        if resp.on_hover_text(tip).clicked() {
            st.set_sel_mode(m);
        }
    }
}

/// The mode shown: the selected shape's with the Select tool, else the one
/// for new shapes.
fn shown_op(st: &AppState) -> Op {
    if st.tool == Tool::Select {
        if let Some(f) = st.single().filter(|f| f.is_solid()) {
            return f.op;
        }
    }
    st.mode
}

fn set_op_from_ui(st: &mut AppState, op: Op) {
    st.mode = op;
    if st.tool == Tool::Select {
        let ids: Vec<FeatureId> = st.selection.iter().copied().filter(|id| st.doc.feature(*id).map(|f| f.is_solid()).unwrap_or(false)).collect();
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
            let select = st.tool == Tool::Select;
            let m = st.sel_mode;
            let (icon, color) = match m {
                SelMode::Vertex => (Icon::ModeVertex, TEXT2()),
                SelMode::Edge => (Icon::ModeEdge, EDGE_SEL),
                SelMode::Face => (Icon::ModeFace, FACE_SEL),
                SelMode::Object => (Icon::Select, TEXT2()),
            };
            let sub = format!("{} · {}", m.label().to_uppercase(), m.key());
            if card(ui, Some(icon), "SELECTION", &sub, select, color, "Select (Esc): what a click picks is set by the modes at the top (1 or F9 vertex, 2 or F10 edge, 3 or F11 face, 4 or F8 object)").clicked() {
                st.select_tool();
            }
            // Maya's Q W E R: the gizmo on what is picked
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let w = ((CARDS_W - 12.0 - 3.0 * 4.0) / 4.0).floor();
                for (g, icon, key, tip) in [
                    (GizmoMode::Select, Icon::Select, "Q", "Select (Q): no gizmo, clicks and rectangles only"),
                    (GizmoMode::Move, Icon::Move, "W", "Move (W): arrows and squares move what is picked, dots resize a shape"),
                    (GizmoMode::Rotate, Icon::Rotate, "E", "Rotate (E): three rings turn the picked shapes round X, Y, Z (15° steps while Snap is on)"),
                    (GizmoMode::Scale, Icon::Scale, "R", "Scale (R): a cube per axis scales along the shape's own axes, the centre scales all (drag right: bigger, left: smaller)"),
                ] {
                    let r = icon_btn(ui, icon, st.gizmo == g, tip, w);
                    key_hint(ui, &r, key);
                    if r.clicked() {
                        st.set_gizmo(g);
                    }
                }
            });
            divider(ui);
            let mode = st.mode;
            let m = if mode == Op::Add { "ADD" } else { "CARVE" };
            for k in PRIMS {
                let key = prim_key(k);
                let on = st.tool == Tool::Create(k);
                let tip = format!("{} ({key}) · again, or Shift + {key}: a 20 mm {} at once, on top of the selection (Carve: through it)", k.label(), k.label().to_lowercase());
                if card(ui, Some(prim_icon(k)), &k.label().to_uppercase(), &format!("{m} · {key}"), on, op_color(mode), &tip).clicked() {
                    st.arm(k);
                }
            }
            let sk = PrimKind::Profile;
            let tip = "Sketch (L): on a face or the ground, click the corners, click the first point (or Enter) to close, then pull it up or down";
            if card(ui, Some(Icon::Sketch), "SKETCH", &format!("{m} · L"), st.tool == Tool::Create(sk), op_color(mode), tip).clicked() {
                st.arm(sk);
            }
            let pk = PrimKind::Path;
            let tip = "Path (P): a curve with no thickness, on a face or the ground. Click the corners; Enter ends it open, a click on the first point closes it. Its corners take a fillet or a bevel in Vertex mode (1)";
            if card(ui, Some(Icon::Path), "PATH", "CURVE · P", st.tool == Tool::Create(pk), PATH, tip).clicked() {
                st.arm(pk);
            }
            let uk = PrimKind::Surface;
            let tip = "Surface (U): a surface with no thickness. Click the outline's points (on a plane, or on the scan with ON SCAN in the top bar); a click on the first point (or Enter) fills it";
            if card(ui, Some(Icon::Path), "SURFACE", "SHEET · U", st.tool == Tool::Create(uk), PATH, tip).clicked() {
                st.arm(uk);
            }
            if st.doc.reference.is_some() {
                let tip = "Slot (S): drag along a slot of the scan (or a raised pill), from one end to the other; let go, the scan is read there and a sketch made as read (Carve when cut in, Add when raised)";
                if card(ui, Some(Icon::Scan), "SLOT", "SCAN · Shift+S", st.tool == Tool::Slot, accent(), tip).clicked() {
                    st.arm_slot();
                }
            }
            divider(ui);
            for (k, icon, tip) in [
                (PushKind::Inset, Icon::Inset, "Inset (I): a flat face's outline moved in (2 mm), sized with its dot; its arrow raises or sinks it. Click a face, or pick a face (3) and press I"),
                (PushKind::Extrude, Icon::Extrude, "Extrude (Ctrl + E): a face pulled out (adds) or pushed in (carves), a curved one along its normals. Press on a face and drag, or pick a face (3) and press Ctrl + E"),
            ] {
                let on = st.tool == Tool::Push(k);
                if card(ui, Some(icon), &k.label().to_uppercase(), &format!("FACE · {}", k.key()), on, FACE_SEL, tip).clicked() {
                    st.arm_push(k, None);
                }
            }
            divider(ui);
            for (t, icon, name, key, tip) in [
                (Tool::Knife, Icon::Knife, "KNIFE", "S", "Knife (S), like Maya's Multi-Cut: click points on the part's edges; each one after the first is joined to the one before across their face (flat or curved). Ctrl + click: a cut across the face; Ctrl + Shift + click: a loop round the part. Enter, Esc or a right click ends the cut"),
                (Tool::Point, Icon::EdgePoint, "POINT", "J", "Point on edge (J): click on an edge to cut a point into it (the middle catches the pointer)"),
            ] {
                if card(ui, Some(icon), name, &format!("EDGE · {key}"), st.tool == t, EDGE_SEL, tip).clicked() {
                    st.arm_knife(t);
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

// ----------------------------------------------------------- materials

/// The material shown as current: the first picked face's (Face mode),
/// the selected element's, or with nothing selected the new shapes'.
fn current_material(st: &AppState) -> Option<u32> {
    if st.sel_mode == SelMode::Face && !st.face_sel.is_empty() {
        return st.face_sel.first().and_then(|r| st.model_face_of(r)).map(|f| st.face_material(f));
    }
    if st.selection.is_empty() {
        return Some(st.material);
    }
    let f = st.single()?;
    let id = st.element_of(f.id);
    st.doc.feature(id).filter(|g| g.is_solid()).map(|_| st.material_of(id))
}

/// The materials island under the rail (Romain, 19 nov.: "je prends des
/// composants, je clique, ça applique le matériel, mais aussi… je prends
/// une sphère et je viens la lâcher sur un élément"): six balls; a click
/// gives the ball to what is picked, a drag carries it onto the part.
fn materials_island(ctx: &egui::Context, st: &mut AppState) {
    let Some(rail) = ctx.memory(|m| m.area_rect(Id::new("rail"))) else { return };
    let off = panel_offset(ctx, "materials");
    egui::Area::new(Id::new("materials")).order(Order::Middle).pivot(Align2::RIGHT_TOP).fixed_pos(pos2(rail.max.x, rail.max.y + 8.0) + off).show(ctx, |ui| {
        island().inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.set_width(CARDS_W - 12.0);
            grip(ui, "materials");
            let cur = current_material(st);
            let w = CARDS_W - 12.0;
            let (hr, _) = ui.allocate_exact_size(vec2(w, 16.0), Sense::hover());
            let (br, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
            let n = materials::MATERIALS.len();
            let gap = (w - 4.0 - 22.0 * n as f32) / (n - 1) as f32;
            let mut hovered: Option<usize> = None;
            for (i, m) in materials::MATERIALS.iter().enumerate() {
                let c = pos2(br.min.x + 2.0 + 11.0 + i as f32 * (22.0 + gap), br.center().y);
                let r = Rect::from_center_size(c, vec2(22.0, 22.0));
                ui.ctx().data_mut(|d| d.insert_temp(Id::new(("mat-ball", i)), c));
                let resp = ui.interact(r, Id::new(("mat-ball", i)), Sense::click_and_drag());
                let dragging = st.mat_drag.map(|d| d.material as usize == i).unwrap_or(false);
                let p = ui.painter();
                if cur == Some(i as u32) {
                    p.circle_stroke(c, 10.5, Stroke::new(1.2, accent()));
                } else if resp.hovered() || dragging {
                    p.circle_stroke(c, 10.5, Stroke::new(1.0, DIVIDER()));
                }
                material_ball(p, c, 8.0, m);
                if resp.hovered() || dragging {
                    hovered = Some(i);
                }
                if resp.drag_started() {
                    st.mat_drag = Some(MatDrag { material: i as u32, target: None });
                }
                if resp.dragged() {
                    let pos = ctx.input(|i| i.pointer.hover_pos());
                    let vr = st.view_rect;
                    let target = pos.filter(|q| q.x > vr[0] && q.x < vr[2] && q.y > vr[1] && q.y < vr[3] && ctx.layer_id_at(*q).map(|l| l.order == Order::Background).unwrap_or(false)).and_then(|q| st.drop_target([q.x, q.y]));
                    if let Some(d) = st.mat_drag.as_mut() {
                        d.target = target;
                    }
                    if let Some(q) = pos {
                        drag_ball(ctx, st, q, m);
                    }
                }
                if resp.drag_stopped() {
                    if let Some(t) = st.mat_drag.and_then(|d| d.target) {
                        st.drop_material(t, i as u32);
                    }
                    st.mat_drag = None;
                } else if resp.clicked() {
                    st.apply_material(i as u32);
                }
                resp.on_hover_text(format!("{}: click to give it to what is picked (Face mode: the picked faces), or drag it onto a shape (Face mode: onto a face)", m.name));
            }
            let p = ui.painter();
            p.text(pos2(hr.min.x + 2.0, hr.center().y), Align2::LEFT_CENTER, "MATERIAL", mono(10.0), LABEL());
            let name = hovered.or(cur.map(|c| c as usize)).map(|i| materials::get(i as u32).name).unwrap_or("–");
            p.text(pos2(hr.max.x - 2.0, hr.center().y), Align2::RIGHT_CENTER, name, regular(10.5), TEXT2());
        });
    });
    // a drag that ended elsewhere (the button let go off the ball's
    // response): nothing lands
    if st.mat_drag.is_some() && !ctx.input(|i| i.pointer.any_down()) {
        st.mat_drag = None;
    }
}

/// The ball carried by the pointer, with what it would land on.
fn drag_ball(ctx: &egui::Context, st: &AppState, at: egui::Pos2, m: &materials::Material) {
    let p = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("mat-drag")));
    let c = at + vec2(14.0, -14.0);
    p.circle_filled(c + vec2(0.0, 1.5), 12.0, Color32::from_black_alpha(70));
    material_ball(&p, c, 11.0, m);
    let what = match st.mat_drag.and_then(|d| d.target) {
        Some(DropTarget::Face { .. }) => "a face".to_string(),
        Some(DropTarget::Shape(id)) => st.doc.feature(id).map(|f| f.name.clone()).unwrap_or_default(),
        None => return,
    };
    let g = p.layout_no_wrap(what, medium(11.5), TEXT());
    let r = Rect::from_min_size(pos2(c.x + 16.0, c.y - g.size().y / 2.0 - 3.0), g.size() + vec2(12.0, 6.0));
    p.rect(r, 6.0, PANEL(), Stroke::new(1.0, PANEL_BORDER()), StrokeKind::Inside);
    p.galley(r.min + vec2(6.0, 3.0), g, TEXT());
}

// --------------------------------------------------------- history strip

/// One icon of the history strip.
struct Cell {
    icon: Icon,
    color: Color32,
    name: String,
    info: String,
    /// The operation it is (its switch in the pop-up).
    op: Option<FeatureId>,
    on: bool,
}

/// The history strip's icons, square like the component modes' at the top.
const CELL: f32 = 28.0;
const CELL_GAP: f32 = 6.0;

/// An undo step's icon and colour, from its name.
fn step_icon(label: &str) -> (Icon, Color32) {
    let l = label.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| l.contains(x));
    if has(&["cube", "box"]) {
        (Icon::Box, accent())
    } else if has(&["cylinder"]) {
        (Icon::Cylinder, accent())
    } else if has(&["sketch"]) {
        (Icon::Sketch, accent())
    } else if has(&["path"]) {
        (Icon::Path, PATH)
    } else if has(&["fillet", "chamfer", "round", "tension", "bevel"]) {
        (Icon::Fillet, EDGE_SEL)
    } else if has(&["inset"]) {
        (Icon::Inset, FACE_SEL)
    } else if has(&["extrude"]) {
        (Icon::Extrude, FACE_SEL)
    } else if has(&["knife", "cut"]) {
        (Icon::Knife, EDGE_SEL)
    } else if has(&["point"]) {
        (Icon::EdgePoint, EDGE_SEL)
    } else if has(&["rotate"]) {
        (Icon::Rotate, TEXT2())
    } else if has(&["scale", "size"]) {
        (Icon::Scale, TEXT2())
    } else if has(&["move"]) {
        (Icon::Move, TEXT2())
    } else if has(&["delete"]) {
        (Icon::Trash, carve())
    } else if has(&["duplicate"]) {
        (Icon::Duplicate, TEXT2())
    } else if has(&["material"]) {
        (Icon::Sphere, TEXT2())
    } else if has(&["hide", "turn"]) {
        (Icon::Eye, TEXT2())
    } else if has(&["history"]) {
        (Icon::Undo, TEXT2())
    } else {
        (Icon::Frame, TEXT2())
    }
}

/// What the strip shows: the element's history (the shape, then its
/// operations; the marker after the first `at` of them), else the scene's
/// undo history (the start, then each step; `at` steps done).
fn history_cells(st: &AppState) -> (Option<FeatureId>, Vec<Cell>, usize) {
    if let Some(el) = st.history_element() {
        let f = st.doc.feature(el).cloned().unwrap();
        let info = if f.is_path() { "Path".to_string() } else { format!("{} · {}", f.prim.kind().label(), f.op.label()) };
        let mut cells = vec![Cell { icon: feature_icon(&f), color: feature_color(&f), name: f.name.clone(), info, op: None, on: f.visible }];
        for id in st.element_ops(el) {
            let Some(g) = st.doc.feature(id) else { continue };
            cells.push(Cell { icon: feature_icon(g), color: feature_color(g), name: g.name.clone(), info: format!("{} · {}", g.kind_label(), step_info(g)), op: Some(id), on: g.visible });
        }
        return (Some(el), cells, st.marker_of(el));
    }
    let (names, at) = st.scene_history();
    let mut cells = vec![Cell { icon: Icon::Folder, color: TEXT2(), name: "Start".into(), info: "The document as opened".into(), op: None, on: true }];
    for (i, n) in names.iter().enumerate() {
        let (icon, color) = step_icon(n);
        cells.push(Cell { icon, color, name: n.clone(), info: format!("Step {} of {}", i + 1, names.len()), op: None, on: true });
    }
    (None, cells, at)
}

/// Goes to cell `i` of the strip: an element's marker, or the scene's
/// undo history.
fn history_go(st: &mut AppState, el: Option<FeatureId>, i: usize) {
    match el {
        Some(el) => {
            if st.marker_of(el) != i {
                st.set_marker(el, i);
            }
        }
        None => st.scene_go(i),
    }
}

/// The history strip (Romain, 19 nov.: "de tout petits icônes… sans
/// texte… les infos apparaissent" when hovered): the selected element's
/// own history, else the scene's (Ctrl + Z). A click goes back to that
/// step; held and dragged along the row, it goes back and forth live
/// (as in ZBrush).
fn history_strip(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let cx = st.principal()[0];
    let max_w = (st.view_rect[2] - st.view_rect[0] - 24.0).max(320.0);
    let off = panel_offset(ctx, "history");
    let (el, cells, at) = history_cells(st);
    let mut hovered: Option<(usize, Rect)> = None;
    egui::Area::new(Id::new("history")).order(Order::Middle).pivot(Align2::CENTER_BOTTOM).fixed_pos(pos2(cx, screen.max.y - 14.0) + off).show(ctx, |ui| {
        panel().inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
            // as wide as its icons (the grip would take 400 points)
            let n = cells.len() as f32;
            let empty = el.is_none() && cells.len() == 1 && st.doc.features.is_empty();
            let content = if empty { 420.0 } else { 78.0 + 8.0 + 1.0 + 8.0 + (max_w - 140.0).min(n * (CELL + CELL_GAP) + 4.0) };
            ui.set_width(content.max(160.0));
            grip(ui, "history");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (tr, _) = ui.allocate_exact_size(vec2(78.0, CELL), Sense::hover());
                let p = ui.painter();
                p.text(pos2(tr.min.x, tr.center().y - 6.5), Align2::LEFT_CENTER, "HISTORY", mono(9.5), LABEL());
                let sub = match el {
                    Some(id) => st.doc.feature(id).map(|f| f.name.clone()).unwrap_or_default(),
                    None => "Scene".into(),
                };
                let mut job = egui::text::LayoutJob::single_section(sub, egui::TextFormat::simple(medium(11.5), TEXT()));
                job.wrap = egui::text::TextWrapping { max_width: tr.width(), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let g = p.layout_job(job);
                p.galley(pos2(tr.min.x, tr.center().y + 7.0 - g.size().y * 0.5), g, TEXT());
                let (sep, _) = ui.allocate_exact_size(vec2(1.0, CELL - 4.0), Sense::hover());
                ui.painter().rect_filled(sep, 0.0, DIVIDER());
                if el.is_none() && cells.len() == 1 && st.doc.features.is_empty() {
                    ui.label(egui::RichText::new("Pick a shape on the right, then drag in the view").font(regular(12.5)).color(TEXT2()));
                    return;
                }
                let n = cells.len();
                let w = n as f32 * (CELL + CELL_GAP);
                egui::ScrollArea::horizontal().id_salt("history-scroll").max_width((max_w - 140.0).min(w + 4.0)).show(ui, |ui| {
                    let (row, resp) = ui.allocate_exact_size(vec2(w, CELL), Sense::click_and_drag());
                    let cell = |i: usize| Rect::from_min_size(pos2(row.min.x + i as f32 * (CELL + CELL_GAP), row.min.y), vec2(CELL, CELL));
                    let idx = |x: f32| ((x - row.min.x + CELL_GAP * 0.5) / (CELL + CELL_GAP)).floor().clamp(0.0, n as f32 - 1.0) as usize;
                    let under = resp.interact_pointer_pos().or(resp.hover_pos()).map(|q| idx(q.x));
                    if resp.drag_started() && el.is_some() {
                        st.end_live();
                        st.begin_live("History");
                    }
                    if resp.dragged() {
                        if let Some(i) = under {
                            history_go(st, el, i);
                        }
                    }
                    if resp.drag_stopped() && el.is_some() {
                        st.end_live();
                    }
                    if resp.clicked() {
                        if let Some(i) = under {
                            history_go(st, el, i);
                            if let Some(e) = el {
                                // the step gone back to is selected (its handles, its window)
                                let id = if i == 0 { Some(e) } else { cells[i].op };
                                st.select_tool();
                                st.select(id, false);
                            }
                        }
                    }
                    let now_at = match el {
                        Some(e) => st.marker_of(e),
                        None => st.scene_history().1,
                    };
                    let _ = at;
                    let p = ui.painter();
                    for (i, c) in cells.iter().enumerate() {
                        let r = cell(i);
                        ui.ctx().data_mut(|d| d.insert_temp(Id::new(("hist-cell", i)), r.center()));
                        let past = i > now_at;
                        let sel = c.op.map(|id| st.is_selected(id)).unwrap_or(false) || (i == 0 && el.map(|e| st.is_selected(e)).unwrap_or(false));
                        if i == now_at {
                            p.rect(r, 7.0, tint(accent(), 0.12), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
                        } else if sel {
                            p.rect(r, 7.0, RAISED(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
                        } else if under == Some(i) && (resp.hovered() || resp.dragged()) {
                            p.rect_filled(r, 7.0, RAISED());
                        }
                        let dim = if past { 0.32 } else if !c.on { 0.45 } else { 1.0 };
                        let ic = if i == now_at { accent() } else { tint(TEXT_ICON(), dim) };
                        icons::paint(p, Rect::from_center_size(r.center(), vec2(15.0, 15.0)), c.icon, ic);
                        let dot = r.right_bottom() - vec2(4.5, 4.5);
                        p.circle_filled(dot, 4.0, PANEL());
                        p.circle_filled(dot, 2.6, tint(c.color, dim));
                        if !c.on {
                            // turned off: struck through
                            p.line_segment([r.left_bottom() + vec2(6.0, -6.0), r.right_top() + vec2(-6.0, 6.0)], Stroke::new(1.4, tint(carve(), 0.9)));
                        }
                    }
                    // the marker: a white bar after the current step
                    let m = cell(now_at);
                    p.rect_filled(Rect::from_center_size(pos2(m.max.x + CELL_GAP * 0.5, m.center().y), vec2(2.0, CELL - 6.0)), 1.0, TEXT_STRONG());
                    if resp.hovered() || resp.dragged() {
                        hovered = under.map(|i| (i, cell(i)));
                    }
                });
            });
        });
    });
    history_pop(ctx, st, &cells, el, hovered);
}

/// A small tick box and its label; true when clicked.
fn tick_row(ui: &mut Ui, label: &str, on: bool) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().clamp(120.0, 200.0), 20.0), Sense::click());
    let p = ui.painter();
    let b = Rect::from_center_size(pos2(r.min.x + 7.0, r.center().y), vec2(13.0, 13.0));
    if on {
        p.rect_filled(b, 3.0, accent());
        p.line_segment([b.min + vec2(3.0, 6.8), b.min + vec2(5.6, 9.4)], Stroke::new(1.6, on_accent()));
        p.line_segment([b.min + vec2(5.6, 9.4), b.min + vec2(10.2, 3.8)], Stroke::new(1.6, on_accent()));
    } else {
        p.rect(b, 3.0, Color32::TRANSPARENT, Stroke::new(1.2, if resp.hovered() { LABEL() } else { DIVIDER() }), StrokeKind::Inside);
    }
    p.text(pos2(b.max.x + 7.0, r.center().y), Align2::LEFT_CENTER, label, regular(12.0), if resp.hovered() { TEXT() } else { LABEL() });
    resp.clicked()
}

/// The pop-up over the hovered history icon: its name and what it does,
/// and for an operation its switch (Romain: "une petite coche"). It stays
/// while the pointer goes up into it.
fn history_pop(ctx: &egui::Context, st: &mut AppState, cells: &[Cell], el: Option<FeatureId>, hovered: Option<(usize, Rect)>) {
    let key = Id::new("history-pop");
    let mut cur: Option<(usize, Rect)> = ctx.data(|d| d.get_temp(key));
    let pop: Option<Rect> = ctx.data(|d| d.get_temp(key.with("rect")));
    let ptr = ctx.input(|i| i.pointer.hover_pos());
    if hovered.is_some() {
        cur = hovered;
    } else if let (Some((_, cr)), Some(q)) = (cur, ptr) {
        let bridge = pop.map(|r| Rect::from_min_max(pos2(cr.min.x - 4.0, r.max.y - 2.0), pos2(cr.max.x + 4.0, cr.min.y + 2.0)));
        let keep = pop.map(|r| r.expand(4.0).contains(q)).unwrap_or(false) || bridge.map(|b| b.contains(q)).unwrap_or(false);
        if !keep {
            cur = None;
        }
    } else {
        cur = None;
    }
    if st.marking.is_open() || st.mat_drag.is_some() {
        cur = None;
    }
    ctx.data_mut(|d| match cur {
        Some(c) => {
            d.insert_temp(key, c);
        }
        None => d.remove::<(usize, Rect)>(key),
    });
    let Some((i, cr)) = cur else { return };
    let Some(c) = cells.get(i) else { return };
    let area = egui::Area::new(key.with("area")).order(Order::Foreground).pivot(Align2::CENTER_BOTTOM).fixed_pos(pos2(cr.center().x, cr.min.y - 10.0)).show(ctx, |ui| {
        egui::Frame::new().fill(PANEL()).stroke(Stroke::new(1.0, PANEL_BORDER())).corner_radius(egui::CornerRadius::same(9)).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.set_min_width(170.0);
            ui.set_max_width(230.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.horizontal(|ui| {
                let (dr, _) = ui.allocate_exact_size(vec2(8.0, 16.0), Sense::hover());
                ui.painter().circle_filled(dr.center(), 3.5, c.color);
                ui.label(egui::RichText::new(&c.name).font(semibold(12.5)).color(TEXT()));
            });
            ui.label(egui::RichText::new(&c.info).font(regular(11.5)).color(TEXT2()));
            if let Some(id) = c.op {
                ui.add_space(3.0);
                let at = ui.cursor().min + vec2(7.0, 10.0);
                ui.ctx().data_mut(|d| d.insert_temp(Id::new("history-tick"), at));
                if tick_row(ui, if c.on { "On" } else { "Off: left out of the build" }, c.on) {
                    st.toggle_op(id);
                }
            }
            ui.add_space(2.0);
            let tip = match (el, i) {
                (Some(_), 0) => "Click: the shape alone, before its operations",
                (Some(_), _) => "Click: back to here · Hold and drag: go back and forth",
                (None, 0) => "Click: back to the start (undo all)",
                (None, _) => "Click: back to here (undo / redo) · Hold and drag: go back and forth",
            };
            ui.label(egui::RichText::new(tip).font(regular(11.0)).color(LABEL()));
        });
    });
    ctx.data_mut(|d| d.insert_temp(key.with("rect"), area.response.rect));
}

// ------------------------------------------------------------------ hud

/// What the help box is about: the tool, or Select and its mode.
fn help_title(st: &AppState) -> String {
    if st.marking.is_open() {
        return "Marking menu".into();
    }
    if let Some(d) = &st.mat_drag {
        return format!("Material · {}", materials::get(d.material).name);
    }
    match st.tool {
        Tool::Create(k) if matches!(k, PrimKind::Path) => "Path".into(),
        Tool::Create(k) if matches!(k, PrimKind::Surface) => if st.surface_on_scan { "Surface · on scan".into() } else { "Surface · on plane".into() },
        Tool::Create(k) => format!("{} · {}", k.label(), st.mode.label()),
        Tool::Push(k) => k.label().into(),
        Tool::Knife => "Knife".into(),
        Tool::Point => "Point on edge".into(),
        Tool::Slot => "Slot · on the scan".into(),
        Tool::Select => format!("{} · {}", st.gizmo.label(), st.sel_mode.label()),
    }
}

/// The help box (Romain, 19 nov.: "une petite pop-up discrète en bas à
/// droite… façon Plasticity", instead of the panels' text): the tool, then
/// what each key and gesture does now, one per line. Folded, a small
/// button.
/// The deviation's legend, under the top bar: the colours from inside to
/// outside the scan, the range (a click picks another), what was measured.
fn deviation_legend(ctx: &egui::Context, st: &mut AppState) {
    if !st.dev_on || st.reference.is_none() {
        return;
    }
    let vr = st.view_rect;
    let at = pos2((vr[0] + vr[2]) * 0.5, vr[1] + 10.0);
    egui::Area::new(Id::new("dev-legend")).order(Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(at).show(ctx, |ui| {
        egui::Frame::new().fill(tint(PANEL(), 0.92)).stroke(Stroke::new(1.0, PANEL_BORDER())).corner_radius(egui::CornerRadius::same(10)).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            let fmt = |v: f64| crate::widgets::fmt_mm(v as f32);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("DEVIATION").font(semibold(10.5)).color(TEXT2()));
                let txt = match &st.deviation {
                    _ if st.dev_busy() => "measuring…".to_string(),
                    Some(d) if d.near == 0 => "the part is not on the scan".to_string(),
                    // by area: a fillet's many small triangles count as their area
                    Some(d) => format!("{:.0} % within ±{} mm · half within {} · 95 % within {} mm", 100.0 * d.area.within, fmt(d.tol), fmt(d.area.p50), fmt(d.area.p95)),
                    None => String::new(),
                };
                ui.label(egui::RichText::new(txt).font(regular(12.0)).color(LABEL()));
                if icon_btn(ui, Icon::EyeOff, false, "Deviation off", 22.0).clicked() {
                    st.set_deviation(false);
                }
            });
            // the colours: inside (blue) to outside (red), the tolerance green
            let w = 320.0;
            let (r, _) = ui.allocate_exact_size(vec2(w, 12.0), Sense::hover());
            let p = ui.painter();
            let range = st.dev_range as f32;
            let tol = st.dev_tol as f32;
            let col = |d: f32| -> Color32 {
                let t = ((d.abs() - tol) / (range - tol).max(1e-4)).clamp(0.0, 1.0);
                let mix = |a: [f32; 3], b: [f32; 3], k: f32| [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k];
                let green = [0.05, 0.45, 0.10];
                let c = if d.abs() <= tol {
                    green
                } else if d > 0.0 {
                    if t < 0.5 { mix(green, [0.80, 0.62, 0.02], t * 2.0) } else { mix([0.80, 0.62, 0.02], [0.75, 0.04, 0.03], (t - 0.5) * 2.0) }
                } else if t < 0.5 {
                    mix(green, [0.03, 0.50, 0.70], t * 2.0)
                } else {
                    mix([0.03, 0.50, 0.70], [0.04, 0.08, 0.75], (t - 0.5) * 2.0)
                };
                // (the shader's colours are linear)
                let s = |v: f32| (v.max(0.0).powf(1.0 / 2.2) * 255.0).round() as u8;
                Color32::from_rgb(s(c[0]), s(c[1]), s(c[2]))
            };
            let n = 64;
            for i in 0..n {
                let x0 = r.min.x + w * i as f32 / n as f32;
                let d = -range + 2.0 * range * (i as f32 + 0.5) / n as f32;
                p.rect_filled(Rect::from_min_max(pos2(x0, r.min.y), pos2(x0 + w / n as f32 + 0.5, r.max.y)), 0.0, col(d));
            }
            let (lr, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
            let p = ui.painter();
            p.text(lr.left_center(), Align2::LEFT_CENTER, format!("−{} inside", fmt(st.dev_range)), regular(11.0), TEXT2());
            p.text(lr.center(), Align2::CENTER_CENTER, "0", regular(11.0), TEXT2());
            p.text(lr.right_center(), Align2::RIGHT_CENTER, format!("outside +{} mm", fmt(st.dev_range)), regular(11.0), TEXT2());
            // the range: a click picks another
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.label(egui::RichText::new("Range").font(regular(11.0)).color(TEXT2()));
                for (v, label) in [(0.1, "0,1"), (0.25, "0,25"), (0.5, "0,5"), (1.0, "1"), (2.0, "2")] {
                    let on = (st.dev_range - v).abs() < 1e-9;
                    let g = egui::RichText::new(label).font(if on { semibold(11.5) } else { regular(11.5) }).color(if on { TEXT_STRONG() } else { TEXT2() });
                    if ui.add(egui::Label::new(g).sense(Sense::click())).on_hover_text("The range the colours span (mm); green within a fifth of it, at most 0,1").clicked() {
                        st.dev_range = v;
                        st.dev_tol = (v * 0.2).min(0.1);
                        st.model_gen += 1;
                    }
                }
                ui.label(egui::RichText::new("mm").font(regular(11.0)).color(TEXT2()));
            });
        });
    });
}

fn help_box(ctx: &egui::Context, st: &mut AppState) {
    let text = crate::viewport::hint(st);
    let color = match st.tool {
        Tool::Create(_) => op_color(st.mode),
        Tool::Push(_) => FACE_SEL,
        Tool::Knife | Tool::Point => EDGE_SEL,
        Tool::Slot => accent(),
        Tool::Select if st.sel_mode == SelMode::Edge => EDGE_SEL,
        Tool::Select if st.sel_mode == SelMode::Face => FACE_SEL,
        Tool::Select => st.single().map(feature_color).unwrap_or(TEXT2()),
    };
    let vr = st.view_rect;
    let at = pos2(vr[2] - 12.0, vr[3] - 4.0);
    let open = st.settings.help;
    egui::Area::new(Id::new("help-box")).order(Order::Middle).pivot(Align2::RIGHT_BOTTOM).fixed_pos(at).show(ctx, |ui| {
        egui::Frame::new().fill(tint(PANEL(), 0.9)).stroke(Stroke::new(1.0, PANEL_BORDER())).corner_radius(egui::CornerRadius::same(10)).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
            if !open {
                let (r, resp) = ui.allocate_exact_size(vec2(64.0, 18.0), Sense::click());
                let p = ui.painter();
                p.circle_filled(pos2(r.min.x + 5.0, r.center().y), 3.5, color);
                p.text(pos2(r.min.x + 14.0, r.center().y), Align2::LEFT_CENTER, "Help", medium(11.5), LABEL());
                p.text(pos2(r.max.x, r.center().y), Align2::RIGHT_CENTER, "?", semibold(12.0), TEXT2());
                if resp.on_hover_text("Show what the keys do (the tool's help)").clicked() {
                    st.settings.help = true;
                    st.save_settings();
                }
                return;
            }
            ui.set_max_width(300.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.spacing_mut().interact_size.y = 18.0;
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(10.0, 16.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, color);
                ui.label(egui::RichText::new(help_title(st)).font(semibold(12.0)).color(TEXT()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                    let c = if resp.hovered() { TEXT() } else { TEXT2() };
                    ui.painter().line_segment([r.center() + vec2(-4.0, 0.0), r.center() + vec2(4.0, 0.0)], Stroke::new(1.4, c));
                    if resp.on_hover_text("Fold the help").clicked() {
                        st.settings.help = false;
                        st.save_settings();
                    }
                });
            });
            // one line per part of the hint; "Key: what it does" with the
            // key in a small chip
            for part in text.split(" · ") {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    match part.split_once(": ").filter(|(k, _)| k.len() <= 30) {
                        Some((k, what)) => {
                            let g = ui.painter().layout_no_wrap(k.to_string(), medium(11.0), LABEL());
                            let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 10.0, 17.0), Sense::hover());
                            ui.painter().rect(r, 4.0, RAISED(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
                            ui.painter().galley(r.center() - g.size() * 0.5, g, LABEL());
                            ui.label(egui::RichText::new(what).font(regular(11.5)).color(TEXT2()));
                        }
                        None => {
                            ui.label(egui::RichText::new(part).font(regular(11.5)).color(TEXT2()));
                        }
                    }
                });
            }
        });
    });
}

fn toast(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let Some(t) = &st.toast else { return };
    let (text, warn) = (t.text.clone(), t.warn);
    let cx = st.principal()[0];
    // above the history pop-up while it is open
    let key = Id::new("history-pop");
    let open = ctx.data(|d| d.get_temp::<(usize, Rect)>(key)).is_some();
    let pop = if open { ctx.data(|d| d.get_temp::<Rect>(key.with("rect"))) } else { None };
    let y = pop.map(|r| r.min.y - 8.0).unwrap_or(screen.max.y - 92.0).min(screen.max.y - 92.0);
    egui::Area::new(Id::new("toast")).order(Order::Tooltip).pivot(Align2::CENTER_BOTTOM).fixed_pos(pos2(cx, y)).interactable(false).show(ctx, |ui| {
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
    // an inset or extrusion: its handles are there instead (Del deletes it)
    if st.selected_push().is_some() {
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
    // below the hint line at the top of the view
    let at = pos2(((x0 + x1) * 0.5).clamp(vr[0] + 120.0, vr[2] - 120.0), (top - 16.0).clamp(vr[1] + 84.0, vr[3] - 40.0));
    egui::Area::new(Id::new("ctx-bar")).order(Order::Middle).pivot(Align2::CENTER_BOTTOM).fixed_pos(at).show(ctx, |ui| {
        island().inner_margin(Margin::same(3)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let solids = st.selection.iter().any(|id| st.doc.feature(*id).map(|f| f.is_solid()).unwrap_or(false));
                if solids {
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

// ------------------------------------------------- the operation window

/// What the operation window shows: the picked edges, faces or vertices,
/// the selected step (fillet, chamfer, inset, extrude, move) or path, or
/// the selected shape's own sizes (Romain 19 nov.: a tool's settings float
/// in the view, not in the left panel).
#[derive(Clone, Copy, PartialEq, Debug)]
enum OpPop {
    Edges,
    Faces,
    Verts,
    Step(FeatureId),
    Shape(FeatureId),
    /// A drawing tool's own options (the Surface's On plane / On scan).
    Tool,
}

fn op_pop(st: &AppState) -> Option<OpPop> {
    if st.tool == Tool::Create(PrimKind::Surface) && st.creation.is_none() {
        return Some(OpPop::Tool);
    }
    if st.creation.is_some() || st.sketch.is_some() || st.push.is_some() || st.tool != Tool::Select {
        return None;
    }
    if st.selection.is_empty() {
        return if !st.edge_sel.is_empty() {
            Some(OpPop::Edges)
        } else if !st.face_sel.is_empty() {
            Some(OpPop::Faces)
        } else if !st.vert_sel.is_empty() {
            Some(OpPop::Verts)
        } else {
            None
        };
    }
    let f = st.single()?;
    Some(if !f.is_solid() || f.is_path() { OpPop::Step(f.id) } else { OpPop::Shape(f.id) })
}

/// Where the worked component is on screen: the box round its points.
fn op_anchor(st: &AppState, k: OpPop) -> Option<Rect> {
    let pts: Vec<DVec3> = match k {
        OpPop::Edges => st.edge_sel.iter().map(|e| e.at).collect(),
        OpPop::Faces => st.face_sel.iter().map(|f| f.at).collect(),
        OpPop::Verts => st.vert_sel.clone(),
        OpPop::Step(id) => {
            let f = st.doc.feature(id)?;
            if f.is_path() {
                f.outline().concat()
            } else if let Some(t) = &f.tweak {
                vec![t.at() + t.delta]
            } else if let Some(r) = &f.round {
                r.edges.iter().map(|e| e.at).collect()
            } else {
                vec![f.origin]
            }
        }
        OpPop::Shape(id) => vec![st.doc.feature(id)?.origin],
        OpPop::Tool => {
            let vr = st.view_rect;
            let q = pos2(vr[2] - 300.0, vr[1] + 160.0);
            return Some(Rect::from_min_max(q, q));
        }
    };
    let view = st.view();
    let mut r: Option<Rect> = None;
    for p in pts {
        if let Some(s) = view.project(p.as_vec3()) {
            let q = pos2(s[0], s[1]);
            r = Some(r.map(|r| r.union(Rect::from_min_max(q, q))).unwrap_or(Rect::from_min_max(q, q)));
        }
    }
    r
}

/// The settings of the operation under way, in a small window next to the
/// component it works on (Romain 18 nov., 22 h 31: "en pop-up fenêtre avec
/// grip de déplacement… comme dans Maya quand on fait un Bevel"). It
/// follows the component; its grip moves it (double-click: back by it).
fn op_window(ctx: &egui::Context, st: &mut AppState) {
    // the marking menu open: nothing over it
    if st.marking.is_open() {
        return;
    }
    let Some(k) = op_pop(st) else { return };
    let Some(anchor) = op_anchor(st, k) else { return };
    // a new subject: the window comes back next to it
    let subject = format!("{k:?}");
    let key = Id::new("op-pop-subject");
    if ctx.data(|d| d.get_temp::<String>(key)) != Some(subject.clone()) {
        ctx.data_mut(|d| {
            d.insert_temp(key, subject);
            d.insert_persisted(Id::new(("panel-offset", "op-pop")), egui::Vec2::ZERO);
        });
    }
    const W: f32 = 236.0;
    let full = W + 20.0;
    let vr = st.view_rect;
    // at the component's height, beside the part so it covers neither the
    // part nor the gizmo: on its right, else its left, else by the component
    let view = st.view();
    let mut part = anchor;
    if let Some((lo, hi)) = st.model.as_ref().and_then(|m| m.bounds()) {
        for i in 0..8 {
            let c = glam::Vec3::new(if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] });
            if let Some(p) = view.project(c) {
                part = part.union(Rect::from_min_max(pos2(p[0], p[1]), pos2(p[0], p[1])));
            }
        }
    }
    // the paths too: they have no mesh
    for f in st.doc.features.iter().filter(|f| f.visible && f.is_path()) {
        for q in f.outline().concat() {
            if let Some(p) = view.project(q.as_vec3()) {
                part = part.union(Rect::from_min_max(pos2(p[0], p[1]), pos2(p[0], p[1])));
            }
        }
    }
    let gap = 28.0;
    // its height as drawn last frame keeps it inside the view
    let h = ctx.memory(|m| m.area_rect(Id::new("op-pop"))).map(|r| r.height()).unwrap_or(200.0);
    let (left, right, top, bottom) = (vr[0] + 8.0, vr[2] - 8.0, vr[1] + 48.0, vr[3] - 8.0);
    let y_by = (anchor.center().y - 60.0).clamp(top, (bottom - h).max(top));
    let x_by = (anchor.center().x - full * 0.5).clamp(left, (right - full).max(left));
    let at = if k == OpPop::Tool {
        // a tool's options: at the view's top right, out of the drawing
        pos2(right - full, top + 60.0)
    } else if part.max.x + gap + full < right {
        pos2(part.max.x + gap, y_by)
    } else if part.min.x - gap - full > left {
        pos2(part.min.x - gap - full, y_by)
    } else if part.max.y + gap + h < bottom {
        pos2(x_by, part.max.y + gap)
    } else if part.min.y - gap - h > top {
        pos2(x_by, part.min.y - gap - h)
    } else {
        let x = if anchor.max.x + 96.0 + full < right { anchor.max.x + 96.0 } else { anchor.min.x - 96.0 - full };
        pos2(x.clamp(left, (right - full).max(left)), y_by)
    } + panel_offset(ctx, "op-pop");
    egui::Area::new(Id::new("op-pop")).order(Order::Foreground).fixed_pos(at).show(ctx, |ui| {
        island().inner_margin(Margin::same(10)).show(ui, |ui| {
            ui.set_width(W);
            grip(ui, "op-pop");
            match k {
                OpPop::Edges => round_card(ui, st),
                OpPop::Faces => face_card(ui, st),
                OpPop::Verts => {
                    let n = st.vert_sel.len();
                    ui.label(egui::RichText::new(if n > 1 { format!("{n} vertices") } else { "Vertex".to_string() }).font(semibold(14.0)).color(TEXT()));
                    vertex_card(ui, st);
                }
                OpPop::Step(id) => {
                    let Some(f) = st.doc.feature(id).cloned() else { return };
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&f.name).font(semibold(14.0)).color(TEXT()));
                        chip_label(ui, f.kind_label(), RAISED(), LABEL());
                    });
                    if f.push.is_some() {
                        push_card(ui, st);
                    } else if f.tweak.is_some() {
                        tweak_card(ui, st, id);
                    } else if f.is_path() {
                        path_card(ui, st, &f);
                    } else {
                        round_card(ui, st);
                    }
                }
                OpPop::Tool => surface_options(ui, st),
                OpPop::Shape(id) => {
                    let Some(f) = st.doc.feature(id).cloned() else { return };
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&f.name).font(semibold(14.0)).color(TEXT()));
                        chip_label(ui, f.kind_label(), RAISED(), LABEL());
                    });
                    shape_card(ui, st, &f);
                }
            }
        });
    });
}

/// The Surface tool's options, in the operation window (Romain 19 nov.,
/// 21 h 50: the tool's options in its pop-up, not in the top bar): its
/// points locked on the plane under the first one, or put on the scan.
fn surface_options(ui: &mut Ui, st: &mut AppState) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Surface").font(semibold(14.0)).color(TEXT()));
        chip_label(ui, "Tool", RAISED(), LABEL());
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        // the Surface tool: its points locked on the plane under the
        // first one, or put on the scan
        let (well, _) = ui.allocate_exact_size(vec2(2.0 * 84.0 + 6.0, 28.0), Sense::hover());
        ui.painter().rect(well, 14.0, WELL(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
        for (i, (c, name)) in [(false, "ON PLANE"), (true, "ON SCAN")].into_iter().enumerate() {
            let r = Rect::from_min_size(well.min + vec2(3.0 + i as f32 * 84.0, 3.0), vec2(84.0, 22.0));
            let resp = ui.interact(r, Id::new(("isl-surf", i)), Sense::click());
            let on = st.surface_on_scan == c;
            if on {
                ui.painter().rect_filled(r, 11.0, PATH);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 11.0, RAISED());
            }
            ui.painter().text(r.center(), Align2::CENTER_CENTER, name, mono(11.0), if on { on_accent() } else { TEXT2() });
            let tip = if c { "Each point is put on the scan under the pointer; the surface follows the scan inside the outline" } else { "The points are locked on the plane under the first one (a face, the scan's face or the ground): a flat surface" };
            // switched while drawing: the points placed stay, the next
            // ones are put the new way (Romain 19 nov., 22 h 03)
            if resp.on_hover_text(&format!("{tip} · Tab: switch, even while drawing")).clicked() && st.surface_on_scan != c {
                st.surface_on_scan = c;
            }
        }
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        // the outline: straight sides, or the curve through the points
        // (SplitIt's), switched any time
        let (well, _) = ui.allocate_exact_size(vec2(2.0 * 84.0 + 6.0, 28.0), Sense::hover());
        ui.painter().rect(well, 14.0, WELL(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
        for (i, (c, name)) in [(false, "LINEAR"), (true, "CURVE")].into_iter().enumerate() {
            let r = Rect::from_min_size(well.min + vec2(3.0 + i as f32 * 84.0, 3.0), vec2(84.0, 22.0));
            let resp = ui.interact(r, Id::new(("isl-curve", i)), Sense::click());
            let on = st.surface_curve == c;
            if on {
                ui.painter().rect_filled(r, 11.0, PATH);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 11.0, RAISED());
            }
            ui.painter().text(r.center(), Align2::CENTER_CENTER, name, mono(11.0), if on { on_accent() } else { TEXT2() });
            let tip = if c { "A smooth curve through the points (Catmull-Rom); on the scan it stays on it · Shift + click a point: a sharp corner there" } else { "Straight sides between the points (Ctrl + drag a point: its fillet)" };
            if resp.on_hover_text(tip).clicked() {
                st.surface_curve = c;
            }
        }
    });
}

/// In the left panel: the settings are in the window in the view.
fn pop_note(ui: &mut Ui) {
    ui.label(egui::RichText::new("Its settings are in the window next to it in the view (its grip moves it)").font(regular(12.0)).color(TEXT2()));
}

// ------------------------------------------------------------ inspector

fn inspector(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let top = EDGE + ISLAND_H + 10.0;
    let inner_w = LEFT_W - 24.0;
    let total = screen.max.y - EDGE - top;
    let rows = st.doc.features.len().max(1) as f32;
    // the groups' headers, and the reference's group (header and row)
    let fs = &st.doc.features;
    let groups = [
        fs.iter().any(|f| f.is_solid() && !f.is_path() && !matches!(f.prim, Prim::Profile { .. })),
        fs.iter().any(|f| f.is_solid() && !f.is_path() && matches!(f.prim, Prim::Profile { .. })),
        fs.iter().any(|f| f.is_path()),
        fs.iter().any(|f| !f.is_solid() && !f.is_path()),
    ];
    let extra = groups.iter().filter(|g| **g).count() as f32 * 25.0 + if st.doc.reference.is_some() { 53.0 } else { 0.0 };
    let tree_h = if st.settings.show_tree { (rows * 28.0 + 34.0 + extra).clamp(96.0, (total * 0.36).max(120.0)) } else { 0.0 };
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
                if st.settings.sub_fixed {
                    // the fixed Sub Outliner: always there, the clicked
                    // element's operations load into it
                    let el = st.outline_element();
                    sub_outliner_body(ui, st, el, SUB_FIXED_H, true);
                    ui.add_space(4.0);
                    divider(ui);
                    ui.add_space(6.0);
                }
            }
            let props_max = (total - tree_h - 40.0 - if st.settings.show_tree && st.settings.sub_fixed { SUB_FIXED_H + 50.0 } else { 0.0 }).max(160.0);
            // an Area's Ui starts at last frame's size: without a minimum the scroll area never grows
            egui::ScrollArea::vertical().id_salt("props-scroll").max_height(props_max).min_scrolled_height(props_max).auto_shrink([false, true]).show(ui, |ui| {
                ui.set_width(inner_w);
                if st.selection.is_empty() && !st.edge_sel.is_empty() {
                    let n = st.edge_sel.len();
                    ui.label(egui::RichText::new(if n > 1 { format!("{n} edges picked") } else { "Edge picked".to_string() }).font(semibold(14.0)).color(TEXT()));
                    pop_note(ui);
                } else if st.selection.is_empty() && !st.face_sel.is_empty() {
                    let n = st.face_sel.len();
                    ui.label(egui::RichText::new(if n > 1 { format!("{n} faces picked") } else { "Face picked".to_string() }).font(semibold(14.0)).color(TEXT()));
                    pop_note(ui);
                } else if st.selection.is_empty() && !st.vert_sel.is_empty() {
                    let n = st.vert_sel.len();
                    ui.label(egui::RichText::new(if n > 1 { format!("{n} vertices picked") } else { "Vertex picked".to_string() }).font(semibold(14.0)).color(TEXT()));
                    let v = st.vert_sel[0];
                    ui.label(egui::RichText::new(format!("X {}  Y {}  Z {} mm", fmt_mm(v.x as f32), fmt_mm(v.y as f32), fmt_mm(v.z as f32))).font(regular(12.0)).color(TEXT2()));
                    pop_note(ui);
                } else if st.single().is_some() {
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
    if st.settings.show_tree && !st.settings.sub_fixed && !st.doc.features.is_empty() {
        // as tall as its operations need (it hid those past the shape
        // list's height: "La Sub Outliner ne se rafraîchit pas")
        let room = (screen.max.y - top - off.y - 140.0).max(tree_h);
        components_column(ctx, st, pos2(screen.min.x + EDGE + LEFT_W + 8.0, top) + off, room);
    }
}

fn tree(ui: &mut Ui, st: &mut AppState, height: f32) {
    let n = st.doc.features.iter().filter(|f| f.is_solid()).count();
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
    // Romain (19 nov.): the elements by kind; an element's operations in
    // the second column, beside the panel
    let feats = st.doc.features.clone();
    let ops_of = |st: &AppState, id: FeatureId| feats.iter().filter(|g| !g.is_solid() && !g.is_path() && st.owner_of(g.id) == Some(id)).count();
    let groups: [(&str, Box<dyn Fn(&Feature) -> bool>); 5] = [
        ("PRIMITIVES", Box::new(|f: &Feature| f.is_solid() && !f.is_path() && matches!(f.prim, Prim::Box { .. } | Prim::Cylinder { .. }))),
        ("SHAPES", Box::new(|f: &Feature| f.is_solid() && !f.is_path() && matches!(f.prim, Prim::Profile { .. }))),
        ("PATHS", Box::new(|f: &Feature| f.is_path())),
        ("SURFACES", Box::new(|f: &Feature| f.is_sheet())),
        ("OPERATIONS", Box::new(|_: &Feature| false)),
    ];
    let focus = st.outline_element();
    egui::ScrollArea::vertical().id_salt("tree-scroll").max_height(height - 32.0).min_scrolled_height(height - 32.0).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        if feats.is_empty() && st.doc.reference.is_none() {
            ui.label(egui::RichText::new("Empty · pick a shape on the right (B, C), or drop an .obj here").font(regular(12.0)).color(TEXT2()));
        }
        reference_rows(ui, st);
        for (gi, (title, keep)) in groups.iter().enumerate() {
            // the last group: operations whose shape cannot be told
            let members: Vec<&Feature> = if gi == 4 { feats.iter().filter(|f| !f.is_solid() && !f.is_path() && st.owner_of(f.id).is_none()).collect() } else { feats.iter().filter(|f| keep(f)).collect() };
            if members.is_empty() {
                continue;
            }
            let fid = Id::new(("tree-group", gi));
            let folded = ui.data(|d| d.get_temp::<bool>(fid)).unwrap_or(false);
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
            let p = ui.painter();
            icons::paint(p, Rect::from_center_size(pos2(r.min.x + 8.0, r.center().y), vec2(10.0, 10.0)), if folded { Icon::ChevronRight } else { Icon::Chevron }, TEXT2());
            p.text(pos2(r.min.x + 20.0, r.center().y), Align2::LEFT_CENTER, *title, semibold(10.0), if resp.hovered() { TEXT() } else { TEXT2() });
            p.text(pos2(r.max.x - 6.0, r.center().y), Align2::RIGHT_CENTER, members.len().to_string(), regular(11.0), TEXT2());
            if resp.clicked() {
                ui.data_mut(|d| d.insert_temp(fid, !folded));
            }
            if folded {
                continue;
            }
            for f in members {
                let n = if gi < 3 { ops_of(st, f.id) } else { 0 };
                tree_row(ui, st, f, n, focus == Some(f.id));
            }
            ui.add_space(3.0);
        }
    });
}

/// The outliner's REFERENCE group: the mesh to model on (dropped on the
/// window), shown or hidden, solid or seen through (Ghost), removed.
fn reference_rows(ui: &mut Ui, st: &mut AppState) {
    let Some(rm) = st.doc.reference.clone() else { return };
    let fid = Id::new("tree-group-ref");
    let folded = ui.data(|d| d.get_temp::<bool>(fid)).unwrap_or(false);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
    let p = ui.painter();
    icons::paint(p, Rect::from_center_size(pos2(r.min.x + 8.0, r.center().y), vec2(10.0, 10.0)), if folded { Icon::ChevronRight } else { Icon::Chevron }, TEXT2());
    p.text(pos2(r.min.x + 20.0, r.center().y), Align2::LEFT_CENTER, "REFERENCE", semibold(10.0), if resp.hovered() { TEXT() } else { TEXT2() });
    p.text(pos2(r.max.x - 6.0, r.center().y), Align2::RIGHT_CENTER, "1", regular(11.0), TEXT2());
    if resp.clicked() {
        ui.data_mut(|d| d.insert_temp(fid, !folded));
    }
    if folded {
        return;
    }
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(r, 6.0, RAISED());
    }
    let dim = if st.ref_shown { 1.0 } else { 0.4 };
    let x0 = r.min.x + 6.0;
    let c = Color32::from_rgb(0x9A, 0xA1, 0xAB);
    let br = Rect::from_min_size(pos2(x0, r.center().y - 8.0), vec2(16.0, 16.0));
    p.rect_filled(br, 4.0, tint(c, 0.16 * dim));
    icons::paint(p, br.shrink(2.0), Icon::Scan, tint(c, dim));
    let info = match (&st.reference, st.ref_busy(), st.ref_failed.as_ref() == Some(&rm.path)) {
        (Some(m), _, _) => format!("{} tris", crate::reference::short_count(m.triangles())),
        (None, true, _) => "loading…".to_string(),
        (None, _, true) => "not loaded".to_string(),
        _ => String::new(),
    };
    let info_g = p.layout_no_wrap(info, regular(12.0), tint(TEXT2(), dim));
    let info_x = r.max.x - 86.0 - info_g.size().x;
    p.galley(pos2(info_x, r.center().y - info_g.size().y * 0.5), info_g, Color32::WHITE);
    let name = crate::reference::name_of(&rm.path);
    let max_w = (info_x - 8.0 - (x0 + 24.0)).max(20.0);
    let mut job = egui::text::LayoutJob::single_section(name, egui::TextFormat::simple(regular(13.0), tint(TEXT(), dim)));
    job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let name_g = ui.painter().layout_job(job);
    ui.painter().galley(pos2(x0 + 24.0, r.center().y - name_g.size().y * 0.5), name_g, TEXT());
    // Deviation, Ghost, then the eye
    let dr = Rect::from_center_size(pos2(r.max.x - 66.0, r.center().y), vec2(16.0, 16.0));
    let dresp = ui.interact(dr.expand(4.0), Id::new("ref-dev"), Sense::click()).on_hover_text("Deviation: the part coloured by its distance to the scan");
    if st.dev_on {
        ui.painter().rect_filled(dr.expand(3.0), 4.0, accent_bg());
    }
    icons::paint(ui.painter(), dr, Icon::Deviation, if st.dev_on { accent() } else { tint(TEXT2(), 0.8) });
    let gr = Rect::from_center_size(pos2(r.max.x - 42.0, r.center().y), vec2(16.0, 16.0));
    let gresp = ui.interact(gr.expand(4.0), Id::new("ref-ghost"), Sense::click()).on_hover_text("Ghost: seen through, the part inside it shows");
    if st.ref_ghost {
        ui.painter().rect_filled(gr.expand(3.0), 4.0, accent_bg());
    }
    icons::paint(ui.painter(), gr, Icon::Ghost, if st.ref_ghost { accent() } else { tint(TEXT2(), 0.8) });
    let er = Rect::from_center_size(pos2(r.max.x - 18.0, r.center().y), vec2(16.0, 16.0));
    let eresp = ui.interact(er.expand(6.0), Id::new("ref-eye"), Sense::click()).on_hover_text("Show / hide");
    icons::paint(ui.painter(), er, if st.ref_shown { Icon::Eye } else { Icon::EyeOff }, tint(TEXT2(), 0.8));
    let resp = resp.on_hover_text(rm.path.clone());
    resp.context_menu(|ui| {
        ui.set_min_width(160.0);
        ui.spacing_mut().button_padding = vec2(8.0, 3.0);
        ui.spacing_mut().interact_size.y = 24.0;
        ui.style_mut().override_font_id = Some(regular(12.5));
        if ui.button(if st.ref_shown { "Hide" } else { "Show" }).clicked() {
            st.ref_shown = !st.ref_shown;
            ui.close();
        }
        if ui.button(if st.ref_ghost { "Solid" } else { "Ghost" }).clicked() {
            st.ref_ghost = !st.ref_ghost;
            ui.close();
        }
        if ui.button(if st.dev_on { "Deviation off" } else { "Deviation" }).clicked() {
            st.set_deviation(!st.dev_on);
            ui.close();
        }
        if ui.button("Primary shapes").clicked() {
            st.read_primaries();
            ui.close();
        }
        if ui.button("Frame").clicked() {
            if let Some(m) = &st.reference {
                let (lo, hi) = (m.lo.as_vec3(), m.hi.as_vec3());
                st.camera.frame(lo, hi);
            }
            ui.close();
        }
        if ui.button("Remove reference").clicked() {
            st.remove_reference();
            ui.close();
        }
    });
    if dresp.clicked() {
        st.set_deviation(!st.dev_on);
    } else if gresp.clicked() {
        st.ref_ghost = !st.ref_ghost;
    } else if eresp.clicked() {
        st.ref_shown = !st.ref_shown;
    }
    if st.reference.is_some() {
        ui.add_space(4.0);
        let w = ui.available_width();
        if secondary_btn(ui, "Primary shapes", w).on_hover_text("Scan → shapes, stage 1: the big blocks read on the scan, as slabs seen from its side").clicked() {
            st.read_primaries();
        }
    }
    ui.add_space(3.0);
}

/// One row of the outliner: a shape, a path or an operation. `ops`: how
/// many operations an element has (shown as a small count); `focused`: its
/// operations are in the second column.
fn tree_row(ui: &mut Ui, st: &mut AppState, f: &Feature, ops: usize, focused: bool) {
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
    ui.ctx().data_mut(|d| d.insert_temp(Id::new(("row", f.id)), r.center()));
    let selected = st.is_selected(f.id);
    let hovered_3d = st.hover_feature == Some(f.id);
    let p = ui.painter();
    if selected {
        p.rect(r, 6.0, accent_bg(), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
    } else if focused {
        p.rect(r, 6.0, RAISED(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
    } else if resp.hovered() || hovered_3d {
        p.rect_filled(r, 6.0, RAISED());
    }
    let x0 = r.min.x + 6.0;
    let is_op = !f.is_solid() && !f.is_path();
    let dim = if f.visible && !f.suspended { 1.0 } else { 0.4 };
    let c = feature_color(f);
    let br = Rect::from_min_size(pos2(x0, r.center().y - 8.0), vec2(16.0, 16.0));
    p.rect_filled(br, 4.0, tint(c, 0.16 * dim));
    if f.is_solid() {
        paint_op_glyph(p, br.center(), f.op, tint(c, dim), 9.0);
    } else {
        icons::paint(p, br.shrink(2.0), feature_icon(f), tint(c, dim));
    }
    let name_color = if selected { TEXT_STRONG() } else { tint(TEXT(), dim) };
    let info = if ops > 0 {
        format!("{} · {ops} op{}", step_info(f), if ops > 1 { "s" } else { "" })
    } else if f.suspended {
        "past the marker".to_string()
    } else {
        step_info(f)
    };
    let info_g = p.layout_no_wrap(info, regular(12.0), tint(TEXT2(), dim));
    let info_x = r.max.x - 40.0 - info_g.size().x;
    p.galley(pos2(info_x, r.center().y - info_g.size().y * 0.5), info_g, Color32::WHITE);
    let max_w = (info_x - 8.0 - (x0 + 24.0)).max(20.0);
    let mut job = egui::text::LayoutJob::single_section(f.name.clone(), egui::TextFormat::simple(if selected { semibold(13.0) } else { regular(13.0) }, name_color));
    job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let name_g = ui.painter().layout_job(job);
    ui.painter().galley(pos2(x0 + 24.0, r.center().y - name_g.size().y * 0.5), name_g, name_color);
    let er = Rect::from_center_size(pos2(r.max.x - 18.0, r.center().y), vec2(16.0, 16.0));
    let eresp = ui.interact(er.expand(6.0), Id::new(("eye", f.id)), Sense::click());
    if is_op {
        // an operation: on or off (Romain, 19 nov.: "activer ou désactiver
        // chacune des opérations… sans casser la chaîne")
        let b = Rect::from_center_size(er.center(), vec2(13.0, 13.0));
        let p = ui.painter();
        if f.visible {
            p.rect_filled(b, 3.0, tint(accent(), if f.suspended { 0.45 } else { 1.0 }));
            p.line_segment([b.min + vec2(3.0, 6.8), b.min + vec2(5.6, 9.4)], Stroke::new(1.6, on_accent()));
            p.line_segment([b.min + vec2(5.6, 9.4), b.min + vec2(10.2, 3.8)], Stroke::new(1.6, on_accent()));
        } else {
            p.rect(b, 3.0, Color32::TRANSPARENT, Stroke::new(1.2, if eresp.hovered() { LABEL() } else { DIVIDER() }), StrokeKind::Inside);
        }
    } else {
        icons::paint(ui.painter(), er, if f.visible { Icon::Eye } else { Icon::EyeOff }, tint(TEXT2(), 0.8));
    }
    let eresp = eresp.on_hover_text(if is_op { "On / off: left out of the build, the operations after it kept" } else { "Show / hide" });
    resp.context_menu(|ui| row_menu(ui, st, f));
    if eresp.clicked() {
        if is_op {
            st.toggle_op(f.id);
        } else {
            st.toggle_visible(f.id);
        }
    } else if resp.clicked() {
        if f.is_solid() {
            st.set_sel_mode(SelMode::Object);
            st.outline_focus = Some(f.id);
        } else if f.push.is_some() {
            st.set_sel_mode(SelMode::Face);
        } else if let Some(t) = &f.tweak {
            st.set_sel_mode(match t.comp {
                nurbs_kernel::Comp::Face(_) => SelMode::Face,
                nurbs_kernel::Comp::Edge(_) => SelMode::Edge,
                nurbs_kernel::Comp::Vertex(_) => SelMode::Vertex,
            });
        } else {
            st.select_tool();
            st.edge_sel.clear();
        }
        let add = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl);
        st.select(Some(f.id), add);
    }
}

/// The outliner's second column, beside the panel: the operations made on
/// the element clicked (fillets, insets, moves, cuts), in history order.
/// A row's right-click menu (Romain: "un petit clic droit"): an
/// operation turned off or on, the marker put after it, deleted; a shape
/// hidden or shown, deleted.
fn row_menu(ui: &mut Ui, st: &mut AppState, f: &Feature) {
    ui.set_min_width(160.0);
    ui.spacing_mut().button_padding = vec2(8.0, 3.0);
    ui.spacing_mut().interact_size.y = 24.0;
    ui.style_mut().override_font_id = Some(regular(12.5));
    let is_op = !f.is_solid() && !f.is_path();
    if is_op {
        if ui.button(if f.visible { "Turn off" } else { "Turn on" }).clicked() {
            st.toggle_op(f.id);
            ui.close();
        }
        if let Some(el) = st.owner_of(f.id) {
            if ui.button("Back to here").clicked() {
                let n = st.element_ops(el).iter().position(|x| *x == f.id).map(|i| i + 1).unwrap_or(0);
                st.set_marker(el, n);
                ui.close();
            }
            if st.marker_of(el) < st.element_ops(el).len() && ui.button("All operations back").clicked() {
                let n = st.element_ops(el).len();
                st.set_marker(el, n);
                ui.close();
            }
        }
    } else if ui.button(if f.visible { "Hide" } else { "Show" }).clicked() {
        st.toggle_visible(f.id);
        ui.close();
    }
    if ui.button("Delete").clicked() {
        st.select(Some(f.id), false);
        st.delete_selected();
        ui.close();
    }
}

/// The Sub Outliner's content: the element's name, then its operations in
/// history order, the marker (a white line) before those left out.
fn sub_outliner_body(ui: &mut Ui, st: &mut AppState, el: Option<FeatureId>, max_h: f32, fixed: bool) {
    ui.horizontal(|ui| {
        ui.set_height(24.0);
        ui.label(egui::RichText::new("SUB OUTLINER").font(semibold(10.5)).color(TEXT2()));
        if let Some(f) = el.and_then(|id| st.doc.feature(id)) {
            ui.label(egui::RichText::new(&f.name).font(semibold(12.5)).color(TEXT()));
        }
    });
    let Some(id) = el else {
        ui.label(egui::RichText::new("Click an element: its operations load here").font(regular(11.5)).color(TEXT2()));
        if fixed {
            ui.add_space((max_h - 46.0).max(0.0));
        }
        return;
    };
    let ops: Vec<Feature> = st.element_ops(id).iter().filter_map(|x| st.doc.feature(*x).cloned()).collect();
    if ops.is_empty() {
        ui.label(egui::RichText::new("No operations yet: its fillets, insets and moves show here").font(regular(11.5)).color(TEXT2()));
        if fixed {
            ui.add_space((max_h - 46.0).max(0.0));
        }
        return;
    }
    let marker = st.marker_of(id);
    let mut sa = egui::ScrollArea::vertical().id_salt(if fixed { "sub-fixed-scroll" } else { "components-scroll" }).max_height(max_h).auto_shrink([false, !fixed]);
    if fixed {
        // a fixed container: the same height whatever it holds
        sa = sa.min_scrolled_height(max_h);
    } else {
        // floating: as tall as its rows (egui kept it at its first height,
        // hiding the operations added after)
        sa = sa.min_scrolled_height((ops.len() as f32 * 29.0 + 8.0).min(max_h));
    }
    sa.show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        for (k, g) in ops.iter().enumerate() {
            if k == marker {
                history_marker_line(ui);
            }
            tree_row(ui, st, g, 0, false);
        }
        if marker >= ops.len() {
            history_marker_line(ui);
        }
    });
}

/// The history marker in a list: a short white line.
fn history_marker_line(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 7.0), Sense::hover());
    ui.painter().rect_filled(Rect::from_center_size(pos2(r.min.x + 22.0, r.center().y), vec2(32.0, 2.0)), 1.0, TEXT_STRONG());
}

/// The floating Sub Outliner (Settings: Floating): an island right of the
/// panel, as tall as the list, while an element is clicked.
fn components_column(ctx: &egui::Context, st: &mut AppState, at: egui::Pos2, height: f32) {
    let Some(id) = st.outline_element() else { return };
    if st.doc.feature(id).is_none() {
        return;
    }
    egui::Area::new(Id::new("components")).order(Order::Middle).fixed_pos(at).show(ctx, |ui| {
        island().inner_margin(Margin::same(10)).show(ui, |ui| {
            ui.set_width(220.0);
            sub_outliner_body(ui, st, Some(id), (height - 40.0).max(60.0), false);
        });
    });
}

/// Nothing selected: what new shapes will be.
fn defaults(ui: &mut Ui, st: &mut AppState) {
    // Add / Carve for new shapes: the top island; their material: the
    // materials island (Romain 19 nov.)
    section(ui, "Display");
    if toggle_row(ui, "Shape list", st.settings.show_tree) {
        st.settings.show_tree = !st.settings.show_tree;
        st.save_settings();
    }
    if st.settings.show_tree {
        ui.label(egui::RichText::new("Sub Outliner").font(regular(12.5)).color(LABEL()));
        let items = [(false, "Floating", accent()), (true, "Fixed", accent())];
        if let Some(v) = segmented(ui, &items, st.settings.sub_fixed, 28.0) {
            st.settings.sub_fixed = v;
            st.save_settings();
        }
        ui.add_space(2.0);
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
    if toggle_row(ui, "Snap to points (Ctrl: off)", st.settings.points) {
        st.settings.points = !st.settings.points;
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
            chip_label(ui, f.kind_label(), RAISED(), LABEL());
            if f.is_solid() {
                let c = op_color(f.op);
                chip_label(ui, f.op.label(), tint(c, 0.16), c);
            }
        });
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if f.is_solid() && icon_btn(ui, Icon::Duplicate, false, "Duplicate", 30.0).clicked() {
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
    // a step or a path: its settings are in the window by it in the view
    if !f.is_solid() || f.is_path() {
        pop_note(ui);
        if f.is_path() {
            let any_down = ui.input(|i| i.pointer.any_down());
            section(ui, "Position and rotation");
            position_rows(ui, st, &f);
            if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() {
                st.end_live();
            }
        }
        return;
    }

    // its sizes are in the window by it in the view; Add / Carve in the top
    // island, its material in the materials island (Romain 19 nov.)
    let any_down = ui.input(|i| i.pointer.any_down());
    if fold_section(ui, "Position and rotation", "position-open") {
        position_rows(ui, st, &f);
    }
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() {
        st.end_live();
    }
}

/// The selected shape's sizes (the operation window).
fn shape_card(ui: &mut Ui, st: &mut AppState, f: &Feature) {
    let id = f.id;
    // a slider drag is one undo step: it ends when no pointer button is down
    let any_down = ui.input(|i| i.pointer.any_down());
    section(ui, "Shape");
    match f.prim {
        Prim::Box { x, y, z } | Prim::Profile { x, y, z } => {
            for (k, label, v) in [(0, "Width", x), (1, "Height", y), (2, "Depth", z)] {
                let mut val = v as f32;
                let (ch, start) = slider_row(ui, label, &mut val, 0.1..=2000.0, 0.1, "mm");
                if ch {
                    feature_edit(st, id, "Size", start, |g| {
                        if let Prim::Box { x, y, z } | Prim::Profile { x, y, z } = &mut g.prim {
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
        Prim::Path { .. } => {}
    }
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() {
        st.end_live();
    }
}

/// A section title with a fold arrow; open by default, kept per session.
/// True when open.
fn fold_section(ui: &mut Ui, title: &str, key: &str) -> bool {
    let id = Id::new(key);
    let open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(true);
    ui.add_space(4.0);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(r, 6.0, RAISED());
    }
    icons::paint(p, Rect::from_center_size(pos2(r.min.x + 8.0, r.center().y), vec2(11.0, 11.0)), if open { Icon::Chevron } else { Icon::ChevronRight }, TEXT2());
    p.text(pos2(r.min.x + 20.0, r.center().y), Align2::LEFT_CENTER, title, semibold(12.0), TEXT2());
    if resp.clicked() {
        ui.data_mut(|d| d.insert_temp(id, !open));
    }
    open
}

/// A path: open or closed, its box, where it is. Its corners are cut in
/// Vertex mode.
fn path_card(ui: &mut Ui, st: &mut AppState, f: &Feature) {
    let any_down = ui.input(|i| i.pointer.any_down());
    let id = f.id;
    let Some(pr) = f.profile.clone() else { return };
    let Prim::Path { x, z } = f.prim else { return };
    section(ui, "Path");
    let cut = pr.corner.iter().filter(|c| c.abs() > 1e-9).count();
    let what = format!(
        "{} path, {} points{}",
        if pr.open { "An open" } else { "A closed" },
        pr.points.len(),
        if cut > 0 { format!(", {cut} corner{} cut", if cut > 1 { "s" } else { "" }) } else { String::new() }
    );
    ui.label(egui::RichText::new(what).font(regular(12.0)).color(TEXT()));
    let items = [(true, "Open", PATH), (false, "Closed", PATH)];
    if let Some(o) = segmented(ui, &items, pr.open, 40.0) {
        let closable = o || pr.points.len() >= 3;
        if closable {
            feature_edit(st, id, if o { "Open path" } else { "Close path" }, true, |g| {
                if let Some(p) = g.profile.as_mut() {
                    p.open = o;
                }
            });
            st.end_live();
        }
    }
    section(ui, "Size");
    for (k, label, v) in [(0, "Width", x), (2, "Depth", z)] {
        let mut val = v as f32;
        let (ch, start) = slider_row(ui, label, &mut val, 0.01..=2000.0, 0.1, "mm");
        if ch {
            feature_edit(st, id, "Size", start, |g| {
                if let Prim::Path { x, z } = &mut g.prim {
                    *(if k == 0 { x } else { z }) = val.max(0.01) as f64;
                }
            });
        }
    }
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() {
        st.end_live();
    }
}

/// The fillet or chamfer: of the picked edges (made at the first size), or
/// of the selected fillet. One undo step per slider drag.
fn round_card(ui: &mut Ui, st: &mut AppState) {
    let any_down = ui.input(|i| i.pointer.any_down());
    let existing = st.selected_round().and_then(|f| f.round.clone());
    let (size, chamfer, n) = match &existing {
        Some(r) => (r.size, r.chamfer, r.edges.len()),
        None => (0.0, st.round_chamfer, st.edge_sel.len()),
    };
    if existing.is_none() {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 5.0, EDGE_SEL);
            let title = if n > 1 { format!("{n} edges picked") } else { "Edge picked".to_string() };
            ui.label(egui::RichText::new(title).font(semibold(14.0)).color(TEXT()));
        });
    } else {
        section(ui, &format!("{} edge{}", n, if n > 1 { "s" } else { "" }));
    }
    ui.add_space(4.0);
    let kinds = [(false, "Fillet", EDGE_SEL), (true, "Chamfer", EDGE_SEL)];
    if let Some(c) = segmented(ui, &kinds, chamfer, 36.0) {
        if existing.is_some() {
            st.set_round(size, c, true);
            st.end_live();
        } else {
            st.round_chamfer = c;
        }
    }
    let max = st.round_max().max(size.ceil()) as f32;
    let mut v = size as f32;
    let (ch, start) = slider_row(ui, if chamfer { "Distance" } else { "Radius" }, &mut v, 0.0..=max, 0.1, "mm");
    if ch {
        st.set_round(v.max(0.0) as f64, chamfer, start || !st.is_live());
    }
    // one click sizes
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for q in [0.5, 1.0, 2.0, 3.0, 5.0] {
            let on = (size - q).abs() < 1e-6;
            let g = ui.painter().layout_no_wrap(fmt_mm(q as f32), medium(12.0), if on { on_accent() } else { TEXT() });
            let (r, resp) = ui.allocate_exact_size(vec2(44.0, 24.0), Sense::click());
            let p = ui.painter();
            p.rect(r, 6.0, if on { EDGE_SEL } else if resp.hovered() { RAISED() } else { WELL() }, Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
            p.galley(r.center() - g.size() * 0.5, g, TEXT());
            if resp.on_hover_text(format!("{} {} mm", if chamfer { "Chamfer" } else { "Fillet" }, fmt_mm(q as f32))).clicked() {
                st.set_round(q, chamfer, true);
                st.end_live();
            }
        }
    });
    // the radius the reference mesh has along these edges (retopo)
    if st.reference.is_some() && existing.as_ref().map(|r| !r.chamfer).unwrap_or(true) {
        ui.add_space(2.0);
        let w = ui.available_width();
        if primary_btn(ui, Some(Icon::Scan), "Fit to scan", Some(w)).on_hover_text("The fillet at the radius the reference mesh has across these edges").clicked() {
            st.fit_round_to_scan();
        }
    }
    // the fillet's tension (straight fillets between flat ends)
    if let (Some(r), Some(id)) = (existing.as_ref().filter(|r| !r.chamfer), st.selected_round().map(|f| f.id)) {
        ui.add_space(2.0);
        if let Some((t, start)) = tension_rows(ui, r.tension, nurbs_kernel::doc::circle_rho(std::f64::consts::FRAC_PI_2)) {
            st.set_round_tension(id, t, start || t == 0.0);
            if t == 0.0 {
                st.end_live();
            }
        }
    }
    // fillets made one after the other meet in one rounded corner
    if let Some(r) = existing.as_ref().filter(|r| !r.chamfer) {
        ui.add_space(2.0);
        if toggle_row(ui, "Blend corners", r.blend) {
            st.set_blend(!r.blend);
        }
    }
    if existing.is_none() {
        ui.add_space(2.0);
        if secondary_btn(ui, "Deselect the edges", ui.available_width()).clicked() {
            st.edge_sel.clear();
        }
    }
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() {
        st.end_live();
    }
}

/// Faces picked in Face mode: what can be done with them.
fn face_card(ui: &mut Ui, st: &mut AppState) {
    let n = st.face_sel.len();
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 5.0, FACE_SEL);
        let title = if n > 1 { format!("{n} faces picked") } else { "Face picked".to_string() };
        ui.label(egui::RichText::new(title).font(semibold(14.0)).color(TEXT()));
    });
    let flat = st.picked_faces().first().and_then(|f| st.model.as_ref().map(|m| m.faces[*f].kind == nurbs_kernel::model::SurfKind::Plane)).unwrap_or(false);
    if !flat {
        ui.label(egui::RichText::new("A round face: it extrudes along its normals; no inset on it").font(regular(12.0)).color(TEXT2()));
    }
    if n > 1 {
        ui.label(egui::RichText::new("Several faces: the tools work on the first one for now").font(regular(12.0)).color(TEXT2()));
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 6.0) * 0.5;
        if secondary_btn(ui, "Inset  I", w).clicked() {
            st.arm_push(PushKind::Inset, None);
        }
        if secondary_btn(ui, "Extrude  Ctrl+E", w).clicked() {
            st.arm_push(PushKind::Extrude, None);
        }
    });
    ui.add_space(2.0);
    if secondary_btn(ui, "Deselect the faces", ui.available_width()).clicked() {
        st.face_sel.clear();
    }
}

/// The selected inset or extrusion: its two sizes (one undo step per
/// slider drag).
fn push_card(ui: &mut Ui, st: &mut AppState) {
    let any_down = ui.input(|i| i.pointer.any_down());
    let Some(p) = st.selected_push().and_then(|f| f.push.clone()) else { return };
    section(ui, "Face");
    let span = st.round_max().max(p.inset.abs().ceil()).max(p.depth.abs().ceil()) as f32 * 2.0;
    // a curved face: no inset, only the depth along its normals
    let id = st.selected_push().map(|f| f.id);
    let curved = st.model.as_ref().map(|m| m.faces.iter().any(|x| x.tag.map(|t| t.pack() == p.face.tag || (Some(t.feature) == id && t.sub == 1)).unwrap_or(false) && x.kind != nurbs_kernel::model::SurfKind::Plane)).unwrap_or(false);
    if !curved || p.inset > 0.0 {
        let mut inset = p.inset as f32;
        let (ch, start) = slider_row(ui, "Inset", &mut inset, 0.0..=span, 0.1, "mm");
        if ch {
            st.set_push(inset.max(0.0) as f64, p.depth, start || !st.is_live());
        }
    }
    let mut depth = p.depth as f32;
    let (ch, start) = slider_row(ui, "Depth", &mut depth, -span..=span, 0.1, "mm");
    if ch {
        st.set_push(p.inset, depth as f64, start || !st.is_live());
    }
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() && st.push.is_none() {
        st.end_live();
    }
}

/// A fillet's tension: the slider shows the conic's rho (the circle's while
/// it is circular, 0); "Circular" puts it back. Returns the new value and
/// whether a new undo step starts.
fn tension_rows(ui: &mut Ui, cur: f64, circle: f64) -> Option<(f64, bool)> {
    let mut out = None;
    let mut v = if cur > 0.0 { cur } else { circle } as f32;
    let (ch, start) = slider_row(ui, "Tension", &mut v, 0.05..=0.95, 0.01, "");
    if ch {
        out = Some(((v as f64).clamp(0.05, 0.95), start));
    }
    if cur > 0.0 && secondary_btn(ui, "Circular", ui.available_width()).clicked() {
        out = Some((0.0, true));
    }
    out
}

/// Vertices picked: the arrows move them; at a corner of a thin plate, a
/// radius rounds it in its plane (a fillet on the short edge there).
fn vertex_card(ui: &mut Ui, st: &mut AppState) {
    // a sketch's corner: cut in its outline (exact arc or chamfer), the
    // whole height follows
    if let Some((size, chamfer, max)) = st.profile_corner_state() {
        let n = st.picked_profile_corners().len();
        let on_path = st.picked_profile_corners().iter().all(|(id, _)| st.doc.feature(*id).map(|f| f.is_path()).unwrap_or(false));
        section(ui, match (on_path, n > 1) {
            (true, true) => "Path corners",
            (true, false) => "Path corner",
            (false, true) => "Sketch corners",
            (false, false) => "Sketch corner",
        });
        let kinds = [(false, "Fillet", EDGE_SEL), (true, "Chamfer", EDGE_SEL)];
        if let Some(c) = segmented(ui, &kinds, chamfer, 36.0) {
            st.set_profile_corners(if size > 0.0 { size } else { 2.0f64.min(max) }, c, true);
            st.end_live();
        }
        let any_down = ui.input(|i| i.pointer.any_down());
        let mut v = size.min(max) as f32;
        let (ch, start) = slider_row(ui, if chamfer { "Distance" } else { "Radius" }, &mut v, 0.0..=(max as f32).max(0.1), 0.1, "mm");
        if ch {
            st.set_profile_corners(v as f64, chamfer, start);
        }
        if size > max + 1e-6 {
            ui.label(egui::RichText::new(format!("Stops at {} mm: half of its shorter side", fmt_mm(max as f32))).font(regular(12.0)).color(TEXT2()));
        }
        if !chamfer && size > 0.0 {
            let picked = st.picked_profile_corners();
            if let Some(&(id, i)) = picked.first() {
                let cur = st.corner_tension(id, i);
                let circle = if cur > 0.0 { nurbs_kernel::doc::circle_rho(std::f64::consts::FRAC_PI_2) } else { st.corner_rho(id, i).unwrap_or(0.41) };
                if let Some((t, start)) = tension_rows(ui, cur, circle) {
                    st.set_corners_tension(&picked, t, start || t == 0.0);
                    if t == 0.0 {
                        st.end_live();
                    }
                }
            }
        }
        if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() && st.push.is_none() {
            st.end_live();
        }
        return;
    }
    let corners = st.vert_sel.iter().filter(|v| !st.vertex_edges(**v).is_empty()).count();
    section(ui, "Round corner");
    if corners == 0 {
        ui.label(egui::RichText::new("Pick a corner of the part to round it").font(regular(12.0)).color(TEXT2()));
        return;
    }
    ui.label(egui::RichText::new(if corners > 1 { format!("{corners} corners: their edges rounded together, then set the radius") } else { "A corner: its edges rounded together, then set the radius".to_string() }).font(regular(12.0)).color(TEXT2()));
    ui.horizontal_wrapped(|ui| {
        for q in [1.0, 2.0, 5.0] {
            if chip(ui, Some(Icon::Fillet), &format!("{} mm", fmt_mm(q as f32)), false, "Round the corner: a fillet on its edges, meeting in a rounded corner (it stops at the largest that works)").clicked() {
                st.round_corners(q);
                st.end_live();
            }
        }
    });
}

/// A moved face, edge or vertex: how far, along X, Y and Z.
fn tweak_card(ui: &mut Ui, st: &mut AppState, id: FeatureId) {
    let any_down = ui.input(|i| i.pointer.any_down());
    let Some(t) = st.doc.feature(id).and_then(|f| f.tweak.clone()) else { return };
    section(ui, "Move");
    let span = (t.delta.abs().max_element().ceil() as f32 * 2.0).max(20.0);
    for (k, label) in [(0usize, "X"), (1, "Y"), (2, "Z")] {
        let mut v = t.delta[k] as f32;
        let (ch, start) = slider_row(ui, label, &mut v, -span..=span, 0.1, "mm");
        if ch {
            feature_edit(st, id, "Move", start, |g| {
                if let Some(tw) = g.tweak.as_mut() {
                    tw.delta[k] = v as f64;
                }
            });
        }
    }
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() && st.push.is_none() {
        st.end_live();
    }
}

/// A material as a small shaded ball: discs shrinking toward the light, a
/// rim, a highlight sharper on metals.
pub fn material_ball(p: &egui::Painter, c: egui::Pos2, r: f32, m: &materials::Material) {
    let steps = 18;
    for k in 0..steps {
        let t = k as f32 / (steps - 1) as f32;
        let lum = 0.45 + 0.75 * t;
        let ch = |x: f32| ((x * lum).clamp(0.0, 1.0) * 255.0) as u8;
        p.circle_filled(c + vec2(-0.3, -0.34) * r * t, r * (1.0 - 0.7 * t), Color32::from_rgb(ch(m.color[0]), ch(m.color[1]), ch(m.color[2])));
    }
    let spec = 0.22 + 0.45 * m.metal * (1.0 - m.rough);
    p.circle_filled(c + vec2(-0.36, -0.4) * r, r * (0.24 - 0.08 * m.metal), Color32::from_white_alpha((spec * 255.0) as u8));
    p.circle_stroke(c, r, Stroke::new(1.0, tint(Color32::WHITE, 0.22)));
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
            if secondary_btn(ui, "New scene: gun scan only", 348.0).on_hover_text("A blank document with the gun scan alone as the reference").clicked() {
                st.new_gun_scene();
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
    // the Surface's On plane / On scan, even in the middle of an outline
    // (Tab: egui's focus moving to a field is undone)
    if st.tool == Tool::Create(PrimKind::Surface) && st.creation.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Tab) && !i.modifiers.any()) {
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
        st.surface_on_scan = !st.surface_on_scan;
        let on = if st.surface_on_scan { "Next points: on the scan" } else { "Next points: on the plane" };
        st.toast(on, false);
        return;
    }
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
            } else if st.sketch.is_some() {
                // drawing an outline: only its last point goes
                st.sketch_back();
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
        if pressed(Key::B) {
            st.make_bevel();
        }
        // Maya: Ctrl + E extrudes (E is the rotate gizmo)
        if pressed(Key::E) && st.push.is_none() {
            let at = ctx.input(|i| i.pointer.hover_pos()).map(|p| [p.x, p.y]);
            st.arm_push(PushKind::Extrude, at);
        }
        return;
    }
    // the sketch under way: Enter closes it, Backspace removes its last point
    if st.sketch.is_some() {
        if pressed(Key::Enter) {
            let at = ctx.input(|i| i.pointer.hover_pos()).map(|p| [p.x, p.y]).unwrap_or(st.principal());
            if st.tool == Tool::Create(PrimKind::Path) {
                st.path_finish(false);
            } else {
                st.sketch_close(at);
            }
        }
        if pressed(Key::Backspace) || pressed(Key::Delete) {
            st.sketch_back();
            return;
        }
    }
    if pressed(Key::Escape) {
        if st.knife.is_some() {
            st.knife_end();
        } else if st.push.is_some() {
            st.abort_gestures();
            st.select_tool();
        } else if st.creation.is_some() || st.drag.is_some() || st.sketch.is_some() || st.scan_round.is_some() || st.scan_slot.is_some() {
            st.abort_gestures();
        } else if st.tool != Tool::Select {
            st.select_tool();
        } else if !st.edge_sel.is_empty() || !st.face_sel.is_empty() || !st.vert_sel.is_empty() {
            st.edge_sel.clear();
            st.face_sel.clear();
            st.vert_sel.clear();
        } else {
            st.selection.clear();
        }
    }
    for (k, m) in [(Key::Num1, SelMode::Vertex), (Key::Num2, SelMode::Edge), (Key::Num3, SelMode::Face), (Key::Num4, SelMode::Object), (Key::F8, SelMode::Object), (Key::F9, SelMode::Vertex), (Key::F10, SelMode::Edge), (Key::F11, SelMode::Face)] {
        if pressed(k) {
            st.set_sel_mode(m);
        }
    }
    // the Knife on S (Romain, 19 nov. 23 h 18), K too; the scan's Slot on Shift + S
    for (k, t) in [(Key::S, Tool::Knife), (Key::K, Tool::Knife), (Key::J, Tool::Point)] {
        if pressed(k) && !shift {
            st.arm_knife(t);
        }
    }
    if shift && pressed(Key::S) {
        st.arm_slot();
    }
    // the Knife's cut under way: Enter ends it
    if matches!(st.tool, Tool::Knife | Tool::Point) && pressed(Key::Enter) {
        st.knife_end();
    }
    // Maya: Q select (no gizmo), W move, E rotate, R scale (rotate and
    // scale work on whole shapes: back to Object mode)
    for (k, g) in [(Key::Q, GizmoMode::Select), (Key::W, GizmoMode::Move), (Key::E, GizmoMode::Rotate), (Key::R, GizmoMode::Scale)] {
        if pressed(k) {
            st.set_gizmo(g);
        }
    }
    for (k, kind) in [(Key::I, PushKind::Inset)] {
        if pressed(k) && st.push.is_none() {
            let at = ctx.input(|i| i.pointer.hover_pos()).map(|p| [p.x, p.y]);
            st.arm_push(kind, at);
        }
    }
    for (k, kind) in [(Key::B, PrimKind::Box), (Key::C, PrimKind::Cylinder), (Key::L, PrimKind::Profile), (Key::P, PrimKind::Path), (Key::U, PrimKind::Surface)] {
        if pressed(k) {
            if shift {
                st.quick_add(kind);
            } else {
                st.arm(kind);
            }
        }
    }
    // G: the last tool again (Maya)
    if pressed(Key::G) && !shift {
        let at = ctx.input(|i| i.pointer.hover_pos()).map(|p| [p.x, p.y]);
        st.repeat_tool(at);
    }
    let alt = ctx.input(|i| i.modifiers.alt);
    if alt && pressed(Key::X) {
        st.xray = !st.xray;
        st.toast(if st.xray { "X-ray on" } else { "X-ray off" }, false);
    } else if pressed(Key::X) {
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
