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
use crate::workspace::Workspace;
use egui::{pos2, vec2, Align2, Color32, Id, Margin, Order, Rect, Response, Sense, Stroke, StrokeKind, Ui};
use glam::{DQuat, DVec3, EulerRot};

/// The iPad's layout (the mock-up of 21 nov.).
mod tablet;
/// The Studio interface (the mock-up D3 of 22 nov., a setting): another
/// layout and way of working, on the iPad and the PC.
mod studio;
use nurbs_kernel::doc::{CurveKind, CurveOp, PrimKind};
use nurbs_kernel::{Feature, FeatureId, Op, Prim};

/// Margin between the islands and the screen edges.
const EDGE: f32 = 14.0;
/// Height of the top islands.
const ISLAND_H: f32 = 40.0;
/// The tabs at the very top (Forme, Sculpt, Poly), then the islands' row.
const TABS_H: f32 = 30.0;
const TOP: f32 = 8.0 + TABS_H + 6.0;
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
    if let Some(m) = &f.mesh {
        return match m.kind {
            nurbs_kernel::mesh::MeshKind::Sculpt => Icon::TabSculpt,
            nurbs_kernel::mesh::MeshKind::Poly => Icon::TabPoly,
            nurbs_kernel::mesh::MeshKind::Sdf => Icon::TabSdf,
        };
    }
    if let Some(k) = &f.cut {
        if k.is_delete() {
            return Icon::Trash;
        }
        return if k.segs.is_empty() { Icon::EdgePoint } else { Icon::Knife };
    }
    if f.tweak.is_some() {
        return Icon::Move;
    }
    if f.shell.is_some() {
        return Icon::Shell;
    }
    if f.mirror.is_some() {
        return Icon::Mirror;
    }
    if f.array.is_some() {
        return Icon::Array;
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
    if let Some(sh) = &f.shell {
        let n = sh.faces.len();
        let open = if n == 0 { "closed".to_string() } else { format!("{n} open") };
        return format!("{} mm · {open}", fmt_mm(sh.thickness as f32));
    }
    match (&f.round, &f.push) {
        (Some(r), _) => format!("{} mm", fmt_mm(r.size as f32)),
        (_, Some(p)) if p.inset > 0.0 && p.depth != 0.0 => format!("{} · {} mm", fmt_mm(p.inset as f32), fmt_signed(p.depth)),
        (_, Some(p)) if p.inset > 0.0 => format!("{} mm", fmt_mm(p.inset as f32)),
        (_, Some(p)) => format!("{} mm", fmt_signed(p.depth)),
        _ => f.kind_label().to_string(),
    }
}

fn fmt_signed(v: f64) -> String {
    if v > 0.0 { format!("+{}", fmt_mm(v as f32)) } else if v < 0.0 { format!("−{}", fmt_mm(-v as f32)) } else { "0".into() }
}

/// A feature's colour: Add, Carve, or the edges' yellow for a fillet.
pub fn feature_color(f: &Feature) -> Color32 {
    if let Some(m) = &f.mesh {
        return match m.kind {
            nurbs_kernel::mesh::MeshKind::Sculpt => Workspace::Sculpt.swatch(),
            nurbs_kernel::mesh::MeshKind::Poly => Workspace::Poly.swatch(),
            nurbs_kernel::mesh::MeshKind::Sdf => Workspace::Sdf.swatch(),
        };
    }
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
    // the Studio interface (studio.rs), on the iPad and the PC: its own
    // layout, the iPad's large rows and keypad
    set_tablet(st.touch.on || st.settings.studio);
    if st.settings.studio {
        studio::show(ctx, st);
        if st.tool != Tool::Select {
            st.last_tool = Some(st.tool);
        }
        return;
    }
    // the iPad: the layout of the mock-up (tablet.rs)
    if st.touch.on {
        tablet::show(ctx, st);
        if st.tool != Tool::Select {
            st.last_tool = Some(st.tool);
        }
        return;
    }
    let screen = ctx.content_rect();
    let left = if st.panel_visible { 2.0 * EDGE + LEFT_W } else { EDGE };
    st.view_rect = [left, TOP + ISLAND_H, screen.width() - 2.0 * EDGE - CARDS_W, screen.height() - HISTORY_H];
    tabs(ctx, st, screen);
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
    group_bar(ctx, st);
    align_window(ctx, st);
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
    egui::Area::new(Id::new("isl-doc")).order(Order::Middle).fixed_pos(screen.min + vec2(EDGE, TOP)).show(ctx, |ui| {
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
    egui::Area::new(Id::new("isl-modes")).order(Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(pos2(cx, screen.min.y + TOP)).show(ctx, |ui| {
        island().show(ui, |ui| {
            ui.set_height(ISLAND_H - 10.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if st.workspace != Workspace::Sculpt && st.workspace != Workspace::Sdf {
                    mode_pill(ui, st);
                    island_sep(ui);
                }
                let tool = match st.tool {
                    Tool::Create(k) => k.label().to_uppercase(),
                    Tool::Curve(k) => format!("CURVE · {}", k.label().to_uppercase()),
                    Tool::Push(k) => k.label().to_uppercase(),
                    Tool::Knife => "KNIFE".into(),
                    Tool::Point => "POINT ON EDGE".into(),
                    Tool::Slot if st.slot_on_scan => "SLOT · ON THE SCAN".into(),
                    Tool::Slot => format!("SLOT · {}", st.mode.label().to_uppercase()),
                    Tool::Select if st.workspace == Workspace::Poly => format!("POLY · {}", st.sel_mode.label().to_uppercase()),
                    // the Sculpt tab's island is wide (size, strength): short
                    Tool::Select if st.workspace == Workspace::Sculpt => "SELECT".into(),
                    Tool::Select if st.workspace == Workspace::Sdf => "SDF".into(),
                    Tool::Form => format!("FORMS · {}", st.form.brush.label().to_uppercase()),
                    Tool::Align => "ALIGN · PICK THE TARGET".into(),
                    Tool::ShellPick => "SHELL · PICK FACES TO OPEN".into(),
                    Tool::Select => format!("SELECTION · {}", st.sel_mode.label().to_uppercase()),
                    Tool::Sculpt => st.sculpt.brush.label().to_uppercase(),
                    Tool::PolyLoop => "INSERT EDGE LOOP".into(),
                    Tool::PolyCut => "CUT".into(),
                };
                let g = ui.painter().layout_no_wrap(tool, mono(11.5), TEXT());
                let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 20.0, 28.0), Sense::hover());
                ui.painter().galley(pos2(r.min.x + 10.0, r.center().y - g.size().y * 0.5), g, TEXT());
                // tab_anim: the tab's own controls come in
                crate::tab_anim::enter(ui, vec2(0.0, -6.0), |ui| match st.workspace {
                    Workspace::Sculpt => sculpt_controls(ui, st),
                    Workspace::Poly => poly_controls(ui, st),
                    Workspace::Sdf => sdf_controls(ui, st),
                    Workspace::Forme => forme_controls(ui, st),
                });
                island_sep(ui);
                if icon_btn(ui, Icon::Magnet, st.settings.snap, &format!("Snap to {} mm", fmt_mm(st.settings.step as f32)), 28.0).clicked() {
                    st.settings.snap = !st.settings.snap;
                    st.save_settings();
                }
                if icon_btn(ui, Icon::Grid, st.settings.grid, "Ground grid", 28.0).clicked() {
                    st.settings.grid = !st.settings.grid;
                    st.save_settings();
                }
                let tip = if st.isolate.is_some() { "Isolate on: click to show everything again (Ctrl + I)" } else { "Isolate: show the selection alone (Ctrl + I)" };
                if icon_btn(ui, Icon::Isolate, st.isolate.is_some(), tip, 28.0).clicked() {
                    st.toggle_isolate();
                }
            });
        });
    });

    // right: the view
    egui::Area::new(Id::new("isl-view")).order(Order::Middle).pivot(Align2::RIGHT_TOP).fixed_pos(pos2(screen.max.x - EDGE, screen.min.y + TOP)).show(ctx, |ui| {
        island().show(ui, |ui| {
            ui.set_height(ISLAND_H - 10.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let cur = st.camera.preset_of();
                let presets = [(ViewPreset::Perspective, "3D", ""), (ViewPreset::Front, "Front", ""), (ViewPreset::Right, "Right", ""), (ViewPreset::Top, "Top", "")];
                if let Some(p) = seg_inline(ui, "isl-preset", &presets, cur) {
                    st.stop_flight();
                    st.camera.preset(p);
                }
                island_sep(ui);
                if icon_btn(ui, Icon::Frame, false, "Frame the whole part (F with nothing selected)", 28.0).clicked() {
                    st.frame_all();
                }
                if icon_btn(ui, Icon::Xray, st.xray, "X-ray (Alt + X): the part see-through, the scan behind it shown", 28.0).clicked() {
                    st.xray = !st.xray;
                }
                // the converters to quads: not in the iPad edition
                if !st.manual_only {
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
                }
                if icon_btn(ui, Icon::Light, st.settings.light, "Light theme", 28.0).clicked() {
                    st.settings.light = !st.settings.light;
                    st.save_settings();
                }
            });
        });
    });
}

/// The Forme tab's part of the centre island: ADD | CARVE for new shapes
/// (and the selected one), FILLET | CHAMFER in Edge and Vertex modes.
fn forme_controls(ui: &mut Ui, st: &mut AppState) {
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
}

/// The Sculpt tab's part of the centre island: the brush's size (on
/// screen) and strength, Mirror.
fn sculpt_controls(ui: &mut Ui, st: &mut AppState) {
    island_sep(ui);
    let small = |ui: &mut Ui, t: &str| {
        let g = ui.painter().layout_no_wrap(t.to_string(), mono(10.0), TEXT2());
        let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 2.0, 28.0), Sense::hover());
        ui.painter().galley(pos2(r.min.x, r.center().y - g.size().y * 0.5), g, TEXT2());
    };
    small(ui, "SIZE");
    let mut r = st.sculpt.radius;
    touch_slider(ui, Id::new("sculpt-size"), &mut r, 5.0, 300.0, false, 1.0, 72.0);
    st.sculpt.radius = r.clamp(5.0, 300.0);
    small(ui, &format!("{:.0}", st.sculpt.radius));
    island_sep(ui);
    small(ui, "STRENGTH");
    let mut k = st.sculpt.strength;
    touch_slider(ui, Id::new("sculpt-strength"), &mut k, 0.0, 1.0, false, 0.01, 72.0);
    st.sculpt.strength = k.clamp(0.0, 1.0);
    small(ui, &format!("{:.2}", st.sculpt.strength));
    island_sep(ui);
    if chip(ui, Some(Icon::Mirror), "MIRROR", st.sculpt.mirror, "Mirror (M): the brush works on both sides of the clay's own X").clicked() {
        st.sculpt.mirror = !st.sculpt.mirror;
    }
}

/// The Poly tab's part of the centre island: the smooth preview of the
/// selected Poly meshes.
fn poly_controls(ui: &mut Ui, st: &mut AppState) {
    let ids = st.poly_ids();
    let on = !ids.is_empty() && ids.iter().all(|id| st.doc.feature(*id).and_then(|f| f.mesh.as_ref()).map(|m| m.smooth).unwrap_or(false));
    island_sep(ui);
    if chip(ui, Some(Icon::SmoothMesh), "SMOOTH", on, "Smooth preview (Alt + S): the selected Poly mesh drawn subdivided, its cage kept").clicked() {
        st.poly_toggle_smooth();
    }
}

/// The SDF tab's part of the centre island: Add / Carve / Intersect and the
/// blend radius, for the next shapes and the picked one.
fn sdf_controls(ui: &mut Ui, st: &mut AppState) {
    use forme_core::CombineMode;
    island_sep(ui);
    let picked = st.sdf_picked().and_then(|(f, n)| crate::sdf::scene_of(f).and_then(|s| s.node(n)).map(|nd| (nd.combine.mode, nd.combine.blend)));
    let (mode, blend) = picked.unwrap_or((st.sdf.mode, st.sdf.blend));
    for (m, icon, label, tip) in [
        (CombineMode::Add, Icon::BoolUnion, "ADD", "Add: the shape joins the object, blended with what is before it"),
        (CombineMode::Carve, Icon::BoolSubtract, "CARVE", "Carve: the shape cuts into what is before it, its rim blended"),
        (CombineMode::Intersect, Icon::BoolIntersect, "INTERSECT", "Intersect: only what is inside both stays"),
    ] {
        if chip(ui, Some(icon), label, mode == m, tip).clicked() {
            st.sdf.mode = m;
            if picked.is_some() {
                st.sdf_edit_node(&format!("Make it {}", m.label()), |n| n.combine.mode = m);
            }
        }
    }
    island_sep(ui);
    let small = |ui: &mut Ui, t: &str| {
        let g = ui.painter().layout_no_wrap(t.to_string(), mono(10.0), TEXT2());
        let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 2.0, 28.0), Sense::hover());
        ui.painter().galley(pos2(r.min.x, r.center().y - g.size().y * 0.5), g, TEXT2());
    };
    small(ui, "BLEND");
    let mut b = blend;
    let (changed, _) = touch_slider(ui, Id::new("sdf-blend"), &mut b, 0.0, 10.0, false, 0.1, 80.0);
    let b = b.clamp(0.0, 30.0);
    if changed && (b - blend).abs() > 1e-4 {
        st.sdf.blend = b;
        if picked.is_some() {
            st.sdf_live_node("Blend", |n| n.combine.blend = b);
        }
    }
    if !ui.input(|i| i.pointer.any_down()) && st.is_live() && st.sdf_drag.is_none() {
        st.end_live();
    }
    small(ui, &format!("{} mm", fmt_mm(b)));
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
            st.set_ops(&ids, op);
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

/// The tabs at the very top (Romain, 20 nov.: "tout en haut, créer des
/// jolis tabs avec marqué Forme, Sculpt, Poly"): each one its icon, its
/// name and a dot of its colour; the current one filled with it.
fn tabs(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let cx = st.principal()[0];
    egui::Area::new(Id::new("isl-tabs")).order(Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(pos2(cx, screen.min.y + 8.0)).show(ctx, |ui| {
        island().inner_margin(Margin::same(3)).corner_radius(egui::CornerRadius::same(12)).show(ui, |ui| {
            ui.set_height(TABS_H - 6.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                // the iPad leaves Sculpt and Poly out (`workspaces`)
                let shown = st.workspaces();
                let tabs: Vec<(Rect, egui::Response)> = shown.iter().map(|_| ui.allocate_exact_size(vec2(116.0, TABS_H - 6.0), Sense::click())).collect();
                let mut rects = [tabs[0].0; 4];
                for (w, (r, _)) in shown.iter().zip(&tabs) {
                    rects[w.index() as usize] = *r;
                }
                let p = ui.painter().clone();
                for (w, (r, resp)) in shown.iter().copied().zip(&tabs) {
                    if st.workspace != w && resp.hovered() {
                        p.rect_filled(*r, 9.0, RAISED());
                    }
                }
                // the lit pill (tab_anim: it slides from the old tab)
                p.rect_filled(crate::tab_anim::pill(ui.ctx(), &rects, st.workspace), 9.0, accent());
                crate::tab_anim::streak(ui.ctx(), &rects);
                let mut go = None;
                for (w, (r, resp)) in shown.iter().copied().zip(tabs) {
                    let i = w.index() as usize;
                    let on = st.workspace == w;
                    let lit = crate::tab_anim::lit(ui.ctx(), w, st.workspace);
                    let off = if resp.hovered() { TEXT() } else { TEXT2() };
                    let fg = off.lerp_to_gamma(on_accent(), lit);
                    let ir = Rect::from_center_size(pos2(r.min.x + 22.0, r.center().y), vec2(15.0, 15.0));
                    icons::paint(&p, ir, w.icon(), fg);
                    p.text(pos2(r.min.x + 36.0, r.center().y), Align2::LEFT_CENTER, w.label(), medium(13.0), fg);
                    // its colour
                    if lit < 0.99 {
                        p.circle_filled(pos2(r.max.x - 14.0, r.center().y), 3.5, w.swatch().gamma_multiply(1.0 - lit));
                    }
                    if resp.on_hover_text(format!("{} (F{}) · Ctrl + Tab: the next tab", w.tip(), i + 1)).clicked() && !on {
                        go = Some(w);
                    }
                }
                if let Some(w) = go {
                    st.set_workspace(w);
                }
            });
        });
    });
}

/// The Sculpt tab's tools: new clay, the brushes, Convert a Poly mesh.
fn sculpt_rail(ui: &mut Ui, st: &mut AppState) {
    use crate::sculpt::Brush;
    let tip = "Sphere (O): a clay sphere, 20 mm, on the view's centre (about 41 000 points)";
    if card(ui, Some(Icon::Sphere), "CLAY SPHERE", "NEW · O", false, accent(), tip).clicked() {
        st.add_sculpt(false);
    }
    let tip = "Block (B): a clay block with rounded edges, 20 mm";
    if card(ui, Some(Icon::Box), "CLAY BLOCK", "NEW · B", false, accent(), tip).clicked() {
        st.add_sculpt(true);
    }
    divider(ui);
    let b = st.sculpt.brush;
    let on = st.tool == Tool::Sculpt;
    if card(ui, Some(b.icon()), "BRUSH", &format!("{} · {}", b.label().to_uppercase(), b.key_name()), on, accent(), b.tip()).clicked() {
        st.abort_gestures();
        st.tool = Tool::Sculpt;
        st.sel_mode = SelMode::Object;
    }
    for row in [&Brush::ALL[..4], &Brush::ALL[4..]] {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = ((CARDS_W - 12.0 - 3.0 * 4.0) / 4.0).floor();
            for k in row {
                let r = icon_btn(ui, k.icon(), on && st.sculpt.brush == *k, k.tip(), w);
                key_hint(ui, &r, k.key_name());
                if r.clicked() {
                    st.set_brush(*k);
                }
            }
        });
    }
    if toggle_row(ui, "Mirror X", st.sculpt.mirror) {
        st.sculpt.mirror = !st.sculpt.mirror;
    }
    // a Poly mesh selected: made clay in place
    if st.selection.iter().any(|id| st.is_poly(*id)) {
        divider(ui);
        if card(ui, Some(Icon::ToSculpt), "TO CLAY", "FROM POLY", false, accent(), "The selected Poly mesh smoothed into clay, in place (the Poly mesh is kept, hidden)").clicked() {
            st.poly_to_sculpt();
        }
    }
}

/// The Poly tab's tools: new meshes, the face and edge operations, the
/// mesh's own (Subdivide, Smooth preview, Send to Sculpt).
fn poly_rail(ui: &mut Ui, st: &mut AppState) {
    use crate::poly::{PolyShape, PullKind};
    section_small(ui, "CREATE");
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = ((CARDS_W - 12.0 - 3.0 * 4.0) / 4.0).floor();
        for (k, icon, key) in [(PolyShape::Cube, Icon::Box, "B"), (PolyShape::Cylinder, Icon::Cylinder, "C"), (PolyShape::Sphere, Icon::Sphere, "O"), (PolyShape::Plane, Icon::PolyPlane, "P")] {
            let r = icon_btn(ui, icon, false, &format!("Poly {} ({key}): 20 mm, on the view's centre", k.label().to_lowercase()), w);
            key_hint(ui, &r, key);
            if r.clicked() {
                st.add_poly(k);
            }
        }
    });
    divider(ui);
    let faces = st.sel_mode == SelMode::Face;
    let at = st.principal();
    if card(ui, Some(Icon::Extrude), "EXTRUDE", "FACES · CTRL+E", st.poly_pull.as_ref().map(|g| g.kind == PullKind::Extrude).unwrap_or(false), FACE_SEL, "Extrude (Ctrl + E): the picked faces pulled out along their normal; the pointer sets how far, a click keeps it").clicked() {
        st.begin_poly_pull(PullKind::Extrude, at);
    }
    if card(ui, Some(Icon::Inset), "INSET", "FACES · I", st.poly_pull.as_ref().map(|g| g.kind == PullKind::Inset).unwrap_or(false), FACE_SEL, "Inset (I): a smaller face inside each picked face; the pointer sets how much, a click keeps it").clicked() {
        st.begin_poly_pull(PullKind::Inset, at);
    }
    if card(ui, Some(Icon::EdgeLoop), "EDGE LOOP", "EDGES · CTRL+R", st.tool == Tool::PolyLoop, EDGE_SEL, "Insert Edge Loop (Ctrl + R): a click on an edge cuts a loop across its ring of quads").clicked() {
        st.arm_poly_loop();
    }
    let bev_on = st.poly_bevel.as_ref().map(|b| b.live).unwrap_or(false);
    if card(ui, Some(Icon::PolyBevel), "BEVEL", "EDGES · CTRL+B", bev_on, EDGE_SEL, "Bevel (Ctrl + B, Maya's): the picked edges (a face's edges, or the whole mesh) cut into rows, flat (Chamfer) or rounded, the corners filled; the pointer sets the width, [ ] the segments, a click keeps it").clicked() {
        st.begin_poly_bevel(at);
    }
    if card(ui, Some(Icon::PolyCut), "CUT", "MULTI-CUT · K", st.tool == Tool::PolyCut, EDGE_SEL, "Cut (K, Maya's Multi-Cut): clicks on edges or points join across the faces; Ctrl + click: an edge loop there; Enter or a right click ends the cut").clicked() {
        st.arm_poly_cut();
    }
    let _ = faces;
    divider(ui);
    if card(ui, Some(Icon::Subdivide), "SUBDIVIDE", "MESH · 4 × FACES", false, TEXT2(), "Subdivide: the selected Poly mesh's faces cut in four, smoothed (Catmull-Clark)").clicked() {
        st.poly_subdivide();
    }
    if card(ui, Some(Icon::ToSculpt), "TO SCULPT", "MESH · CLAY", false, Workspace::Sculpt.swatch(), "Send to Sculpt: the selected Poly mesh smoothed into clay, in place (the Poly mesh is kept, hidden); then the Sculpt tab").clicked() {
        st.poly_to_sculpt();
        st.set_workspace(Workspace::Sculpt);
    }
}

/// The SDF tab's tools: the five shapes (into the SDF object worked on,
/// else a new one), the operation of the next ones.
fn sdf_rail(ui: &mut Ui, st: &mut AppState) {
    use crate::sdf::{prim_icon, prim_key, prim_label, SDF_PRIMS};
    use forme_core::CombineMode;
    let into = st.sdf_target().and_then(|id| st.doc.feature(id)).map(|f| f.name.clone());
    section_small(ui, if into.is_some() { "ADD A SHAPE" } else { "NEW SDF OBJECT" });
    for row in SDF_PRIMS.chunks(3) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = ((CARDS_W - 12.0 - 2.0 * 4.0) / 3.0).floor();
            for k in row {
                let key = prim_key(*k);
                let tip = match &into {
                    Some(n) => format!("{} ({key}): into {n}, {} with a blend of {} mm", prim_label(*k), st.sdf.mode.label().to_lowercase(), fmt_mm(st.sdf.blend)),
                    None => format!("{} ({key}): a new SDF object on the view's centre, standing on the ground", prim_label(*k)),
                };
                let r = icon_btn(ui, prim_icon(*k), false, &tip, w);
                key_hint(ui, &r, key);
                if r.clicked() {
                    st.sdf_add(*k);
                }
            }
        });
    }
    divider(ui);
    let picked = st.sdf_picked().and_then(|(f, n)| crate::sdf::scene_of(f).and_then(|s| s.node(n)).map(|nd| nd.combine.mode));
    let mode = picked.unwrap_or(st.sdf.mode);
    for (m, icon, key, sub) in [(CombineMode::Add, Icon::BoolUnion, "A", "UNION · BLEND"), (CombineMode::Carve, Icon::BoolSubtract, "X", "CUT · BLEND"), (CombineMode::Intersect, Icon::BoolIntersect, "I", "COMMON PART")] {
        let c = if m == CombineMode::Carve { carve() } else { accent() };
        if card(ui, Some(icon), &m.label().to_uppercase(), &format!("{sub} · {key}"), mode == m, c, &format!("{} ({key}): the next shapes, and the picked one", m.label())).clicked() {
            st.sdf.mode = m;
            if picked.is_some() {
                st.sdf_edit_node(&format!("Make it {}", m.label()), |n| n.combine.mode = m);
            }
        }
    }
}

/// An SDF object's window: its shapes in order (a click picks one), the
/// picked one's operation, blend, sizes and fillet.
fn sdf_card(ui: &mut Ui, st: &mut AppState, f: &Feature) {
    use forme_core::{CombineMode, PrimKind};
    let Some(scene) = crate::sdf::scene_of(f).cloned() else { return };
    let any_down = ui.input(|i| i.pointer.any_down());
    let list = crate::sdf::shapes(&scene);
    let tris = f.mesh.as_ref().map(|m| m.faces.len()).unwrap_or(0);
    let fine = st.sdf_meshed.get(&f.id).map(|s| s.1).unwrap_or(false);
    let g = ui.painter().layout_no_wrap(format!("{} shape{} · {} triangles{}", list.len(), if list.len() > 1 { "s" } else { "" }, tris, if fine { "" } else { " · meshing…" }), regular(12.0), TEXT2());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    ui.painter().galley(pos2(r.min.x, r.center().y - g.size().y * 0.5), g, TEXT2());
    section(ui, "Shapes");
    let picked = st.sdf_sel.filter(|(id, _)| *id == f.id).map(|(_, n)| n);
    // the same rows as the Sub Outliner's (`common::Item`): on / off, the
    // marker, the right-click menu
    let marker = st.item_marker(f.id);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        for (k, n) in list.iter().enumerate() {
            if k == marker {
                history_marker_line(ui);
            }
            item_row(ui, st, crate::common::Item::Shape(f.id, *n));
        }
    });
    let Some(n) = picked else {
        let g = ui.painter().layout_no_wrap("Click a shape (here or in the view) to edit it".to_string(), regular(11.5), TEXT2());
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
        ui.painter().galley(pos2(r.min.x, r.center().y - g.size().y * 0.5), g, TEXT2());
        return;
    };
    let Some(nd) = scene.node(n).cloned() else { return };
    let Some(sh) = nd.shape().cloned() else { return };
    section(ui, &nd.name);
    let mut mode_pick = segmented(ui, &[(CombineMode::Add, "Add", accent()), (CombineMode::Carve, "Carve", carve()), (CombineMode::Intersect, "Intersect", TEXT())], nd.combine.mode, 28.0);
    // Groove and Tongue (Forme SDF's): a line cut in, or raised, where the
    // shape's surface crosses what is before it
    if let Some(m) = ui.push_id("sdf-groove", |ui| segmented(ui, &[(CombineMode::Groove, "Groove", carve()), (CombineMode::Tongue, "Tongue", accent())], nd.combine.mode, 26.0)).inner {
        mode_pick = Some(m);
    }
    if let Some(m) = mode_pick {
        if crate::sdf::SDF_MODES.contains(&m) {
            st.sdf.mode = m;
        }
        st.sdf_edit_node(&format!("Make it {}", m.label()), |x| x.combine.mode = m);
    }
    let mut live = false;
    let mut b = nd.combine.blend;
    if slider_row(ui, "Blend", &mut b, 0.0..=30.0, 0.1, "mm").0 && (b - nd.combine.blend).abs() > 1e-5 {
        st.sdf_live_node("Blend", |x| x.combine.blend = b);
        live = true;
    }
    if matches!(nd.combine.mode, CombineMode::Groove | CombineMode::Tongue) {
        let mut v = nd.combine.depth;
        if slider_row(ui, "Depth", &mut v, 0.05..=20.0, 0.05, "mm").0 && (v - nd.combine.depth).abs() > 1e-5 {
            st.sdf_live_node("Groove depth", |x| x.combine.depth = v);
            live = true;
        }
        let mut v = nd.combine.width;
        if slider_row(ui, "Width", &mut v, 0.05..=20.0, 0.05, "mm").0 && (v - nd.combine.width).abs() > 1e-5 {
            st.sdf_live_node("Groove width", |x| x.combine.width = v);
            live = true;
        }
    }
    // sizes are half sizes in the engine: shown whole
    let rows: Vec<(&str, usize)> = match sh.kind {
        PrimKind::Box => vec![("Width", 0), ("Height", 1), ("Depth", 2)],
        PrimKind::Cylinder | PrimKind::Capsule => vec![("Diameter", 0), ("Height", 1)],
        PrimKind::Sphere => vec![("Diameter", 0)],
        PrimKind::Torus => vec![("Diameter", 0), ("Thickness", 2)],
        _ => vec![("Size", 0)],
    };
    for (label, a) in rows {
        let was = sh.size[a] * 2.0;
        let mut v = was;
        if slider_row(ui, label, &mut v, 0.2..=500.0, 0.1, "mm").0 && (v - was).abs() > 1e-5 {
            let k = sh.kind;
            st.sdf_live_node("Size", |x| {
                if let Some(s) = x.shape_mut() {
                    if k == PrimKind::Sphere {
                        s.size = glam::Vec3::splat(v * 0.5);
                    } else {
                        s.size[a] = v * 0.5;
                    }
                }
            });
            live = true;
        }
    }
    if matches!(sh.kind, PrimKind::Box | PrimKind::Cylinder) {
        let was = sh.round;
        let mut v = was;
        if slider_row(ui, "Fillet", &mut v, 0.0..=50.0, 0.1, "mm").0 && (v - was).abs() > 1e-5 {
            st.sdf_live_node("Fillet", |x| {
                if let Some(s) = x.shape_mut() {
                    s.round = v;
                    s.edges.clear();
                }
            });
            live = true;
        }
    }
    // the blend's profile: round, or a flat chamfer between the shapes
    if nd.combine.blend > 0.0 {
        use forme_core::BlendKind;
        if let Some(k) = segmented(ui, &[(BlendKind::Smooth, "Round blend", accent()), (BlendKind::Chamfer, "Chamfer blend", accent())], nd.combine.kind, 26.0) {
            st.sdf_edit_node("Blend kind", |x| x.combine.kind = k);
        }
    }
    // where it sits in its object (mm; the gizmo moves it too)
    if fold_section(ui, "Position", "sdf-pos") {
        if let Some(pos) = xyz_fields(ui, ["X", "Y", "Z"], nd.transform.pos.to_array(), "") {
            st.sdf_live_node("Move shape", |x| x.transform.pos = glam::Vec3::from(pos));
            live = true;
        }
        let (ex, ey, ez) = nd.transform.rot.to_euler(glam::EulerRot::XYZ);
        if let Some(e) = xyz_fields(ui, ["rX", "rY", "rZ"], [ex.to_degrees(), ey.to_degrees(), ez.to_degrees()], "°") {
            st.sdf_live_node("Rotate shape", |x| x.transform.rot = glam::Quat::from_euler(glam::EulerRot::XYZ, e[0].to_radians(), e[1].to_radians(), e[2].to_radians()));
            live = true;
        }
    }
    // mirror across the object's own planes, repeat round or along an axis
    // (Forme SDF's engine does both; the copies blend like the shape)
    if fold_section(ui, "Mirror and repeat", "sdf-rep") {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(egui::RichText::new("Mirror").font(regular(12.5)).color(LABEL()));
            for (a, label) in ["X", "Y", "Z"].into_iter().enumerate() {
                let on = [nd.mirror.x, nd.mirror.y, nd.mirror.z][a];
                if chip(ui, None, label, on, &format!("Mirrored across the object's {label} plane")).clicked() {
                    st.sdf_edit_node(&format!("Mirror {label}"), |x| match a {
                        0 => x.mirror.x = !on,
                        1 => x.mirror.y = !on,
                        _ => x.mirror.z = !on,
                    });
                }
            }
        });
        use forme_core::{Axis, Repeat};
        let kind = match nd.repeat {
            Repeat::None => 0,
            Repeat::Radial { .. } => 1,
            Repeat::Linear { .. } => 2,
        };
        if let Some(k) = segmented(ui, &[(0, "No repeat", TEXT()), (1, "Round", accent()), (2, "Along", accent())], kind, 26.0) {
            let r = match k {
                1 => Repeat::Radial { count: 6, axis: Axis::Y },
                2 => Repeat::Linear { count: 3, step: glam::Vec3::new((sh.local_half_extents().x * 2.0 + 2.0).max(4.0), 0.0, 0.0) },
                _ => Repeat::None,
            };
            st.sdf_edit_node("Repeat", |x| x.repeat = r);
        }
        match nd.repeat {
            Repeat::Radial { count, axis } => {
                let mut c = count as f32;
                if slider_row(ui, "Copies", &mut c, 2.0..=48.0, 1.0, "").0 && c.round() as u32 != count {
                    let c = c.round().max(2.0) as u32;
                    st.sdf_live_node("Repeat", |x| x.repeat = Repeat::Radial { count: c, axis });
                    live = true;
                }
                if let Some(ax) = segmented(ui, &[(Axis::X, "Round X", AXIS_X), (Axis::Y, "Round Y", AXIS_Y), (Axis::Z, "Round Z", AXIS_Z)], axis, 24.0) {
                    st.sdf_edit_node("Repeat", |x| x.repeat = Repeat::Radial { count, axis: ax });
                }
            }
            Repeat::Linear { count, step } => {
                let mut c = count as f32;
                if slider_row(ui, "Copies", &mut c, 2.0..=48.0, 1.0, "").0 && c.round() as u32 != count {
                    let c = c.round().max(2.0) as u32;
                    st.sdf_live_node("Repeat", |x| x.repeat = Repeat::Linear { count: c, step });
                    live = true;
                }
                if let Some(s) = xyz_fields(ui, ["X", "Y", "Z"], step.to_array(), "") {
                    st.sdf_live_node("Repeat", |x| x.repeat = Repeat::Linear { count, step: glam::Vec3::from(s) });
                    live = true;
                }
            }
            Repeat::None => {}
        }
    }
    if !any_down && !live && st.is_live() && st.sdf_drag.is_none() {
        st.end_live();
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if secondary_btn(ui, "Duplicate", 96.0).clicked() {
            st.sdf_duplicate();
        }
        if secondary_btn(ui, "Delete shape", 110.0).clicked() {
            if !st.sdf_delete() {
                st.delete_selected();
            }
        }
    });
}

/// A small title in the rail.
fn section_small(ui: &mut Ui, title: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
    ui.painter().text(pos2(r.min.x + 2.0, r.center().y), Align2::LEFT_CENTER, title, semibold(9.5), TEXT2());
}

fn rail(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let top = TOP + ISLAND_H + 10.0;
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
            // tab_anim: the tab's own cards come in from the right
            crate::tab_anim::enter(ui, vec2(14.0, 0.0), |ui| match st.workspace {
                Workspace::Sculpt => sculpt_rail(ui, st),
                Workspace::Poly => poly_rail(ui, st),
                Workspace::Sdf => sdf_rail(ui, st),
                Workspace::Forme => {
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
                    let tip = "Curves (P: a line): curves with no thickness, on a face or the ground. The line: click the corners; Enter ends it open, a click on the first point closes it; its corners take a fillet or a bevel in Vertex mode (1). Below, 3ds Max's shapes, dragged out, their settings in their window";
                    let on = st.tool == Tool::Create(pk) || matches!(st.tool, Tool::Curve(_));
                    let sub = match st.tool {
                        Tool::Curve(k) => k.label().to_uppercase(),
                        _ => "PATH · P".to_string(),
                    };
                    if card(ui, Some(Icon::Path), "CURVES", &sub, on, PATH, tip).clicked() {
                        st.arm(pk);
                    }
                    // 3ds Max's Shapes: the Curves set
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let w = ((CARDS_W - 12.0 - 4.0 * 4.0) / 5.0).floor();
                        for (k, icon, tip) in [
                            (CurveKind::Rectangle, Icon::CurveRect, "Rectangle: drag corner to corner (Shift: a square); its width, length and corner radius in its window"),
                            (CurveKind::Circle, Icon::CurveCircle, "Circle: drag from its centre out; its radius in its window"),
                            (CurveKind::Ellipse, Icon::CurveEllipse, "Ellipse: drag corner to corner (Shift: a circle); its width and length in its window"),
                            (CurveKind::Polygon, Icon::CurvePolygon, "Polygon: drag from its centre out; its radius, sides and corner radius in its window"),
                            (CurveKind::Star, Icon::CurveStar, "Star: drag from its centre out; its points, inner radius and fillets in its window"),
                        ] {
                            if icon_btn(ui, icon, st.tool == Tool::Curve(k), tip, w).clicked() {
                                st.arm_curve(k);
                            }
                        }
                    });
                    let uk = PrimKind::Surface;
                    let tip = "Surface (U): a surface with no thickness. Click the outline's points (on a plane, or on the scan with ON SCAN in the top bar); a click on the first point (or Enter) fills it";
                    if card(ui, Some(Icon::Path), "SURFACE", "SHEET · U", st.tool == Tool::Create(uk), PATH, tip).clicked() {
                        st.arm(uk);
                    }
                    {
                        let tip = "Slot (Shift + S): drag from one end to the other on a face or the ground, move to set its width, click, then its height. ON SCAN (its options): read a slot of the scan instead";
                        if card(ui, Some(Icon::Scan), "SLOT", "SHAPE · Shift+S", st.tool == Tool::Slot, accent(), tip).clicked() {
                            st.arm_slot();
                        }
                    }
                    {
                        let tip = "Forms (M): a smooth surface (a sphere, a cylinder) pulled with brushes, like Nomad or ZBrush. Click the ground: a new form; press on a form and drag: the brush (Move, Inflate, Smooth, Flatten, Pinch, in its window); a plain Cube or Cylinder becomes a form";
                        let sub = format!("{} · M", st.form.brush.label().to_uppercase());
                        if card(ui, Some(Icon::Sphere), "FORMS", &sub, st.tool == Tool::Form, op_color(mode), tip).clicked() {
                            st.arm_form();
                        }
                    }
                    divider(ui);
                    for (k, icon, tip) in [
                        (PushKind::Inset, Icon::Inset, "Inset (I): a flat face's outline moved in (2 mm), sized with its dot; its arrow raises or sinks it. Click a face, or pick a face (3) and press I"),
                        (PushKind::Extrude, Icon::Extrude, "Extrude (Ctrl + E): a face pulled out (adds) or pushed in (carves), a curved one along its normals; a closed curve pulled into a solid. Press on a face or a curve and drag, or pick it and press Ctrl + E"),
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
                }
            });
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
    // the SDF tab's picked shape: its own (else its object's)
    if let Some((id, n)) = st.sdf_sel.filter(|_| st.sdf_comp()) {
        return Some(st.item_material(crate::common::Item::Shape(id, n)));
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
    materials_island_at(ctx, st, pos2(rail.max.x, rail.max.y + 8.0) + off, Align2::RIGHT_TOP, CARDS_W - 12.0);
}

/// The materials island at a place (the iPad: a pop-over beside the dock's
/// ball), `w` points wide.
fn materials_island_at(ctx: &egui::Context, st: &mut AppState, at: egui::Pos2, pivot: Align2, w: f32) {
    let big = tablet();
    let (bs, br_h) = if big { (34.0, 40.0) } else { (22.0, 24.0) };
    egui::Area::new(Id::new("materials")).order(Order::Middle).pivot(pivot).fixed_pos(at).show(ctx, |ui| {
        island().inner_margin(Margin::same(if big { 12 } else { 6 })).show(ui, |ui| {
            ui.set_width(w);
            if !big {
                grip(ui, "materials");
            }
            let cur = current_material(st);
            let (hr, _) = ui.allocate_exact_size(vec2(w, if big { 22.0 } else { 16.0 }), Sense::hover());
            let (br, _) = ui.allocate_exact_size(vec2(w, br_h), Sense::hover());
            let mats = &materials::MATERIALS;
            let n = mats.len();
            let gap = (w - 4.0 - bs * n as f32) / (n - 1) as f32;
            let mut hovered: Option<usize> = None;
            for (i, m) in mats.iter().enumerate() {
                let c = pos2(br.min.x + 2.0 + bs * 0.5 + i as f32 * (bs + gap), br.center().y);
                let r = Rect::from_center_size(c, vec2(bs, bs));
                ui.ctx().data_mut(|d| d.insert_temp(Id::new(("mat-ball", i)), c));
                let resp = ui.interact(r, Id::new(("mat-ball", i)), Sense::click_and_drag());
                let dragging = st.mat_drag.map(|d| d.material as usize == i).unwrap_or(false);
                let p = ui.painter();
                if cur == Some(i as u32) {
                    p.circle_stroke(c, bs * 0.48, Stroke::new(1.2, accent()));
                } else if resp.hovered() || dragging {
                    p.circle_stroke(c, bs * 0.48, Stroke::new(1.0, DIVIDER()));
                }
                material_ball(p, c, bs * 0.36, &st.look(i as u32));
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
                        drag_ball(ctx, st, q, &st.look(i as u32));
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
                resp.on_hover_text(format!("{}: click to give it to what is picked (Face mode: the picked faces; SDF tab: the picked shape), or drag it onto a shape; its colour, specular and transparency below", m.name));
            }
            let p = ui.painter();
            p.text(pos2(hr.min.x + 2.0, hr.center().y), Align2::LEFT_CENTER, "MATERIAL", mono(10.0), LABEL());
            let name = hovered.or(cur.map(|c| c as usize)).map(|i| materials::get(i as u32).name).unwrap_or("–");
            // a ball changed: a small reset by its name
            let mi = cur.unwrap_or(st.material);
            let changed = materials::changed(&st.doc, mi);
            p.text(pos2(hr.max.x - if changed { 18.0 } else { 2.0 }, hr.center().y), Align2::RIGHT_CENTER, name, regular(10.5), TEXT2());
            if changed {
                let rr = Rect::from_center_size(pos2(hr.max.x - 7.0, hr.center().y), vec2(14.0, 14.0));
                let resp = ui.interact(rr, Id::new("look-reset"), Sense::click());
                ui.painter().text(rr.center(), Align2::CENTER_CENTER, "↺", regular(12.0), if resp.hovered() { TEXT_STRONG() } else { LABEL() });
                if resp.on_hover_text(format!("{} back as it comes", materials::get(mi).name)).clicked() {
                    st.reset_look(mi);
                }
            }
            // the current ball's settings (Romain, 20 nov., 15 h 42: "quelques
            // petits slots basiques": base colour, specular, transparency)
            look_slots(ui, st, cur.unwrap_or(st.material), w);
        });
    });
    // a drag that ended elsewhere (the button let go off the ball's
    // response): nothing lands
    if st.mat_drag.is_some() && !ctx.input(|i| i.pointer.any_down()) {
        st.mat_drag = None;
    }
}

/// The settings of the ball shown as current, under the balls: its colour
/// (a click on the row opens a small picker below it), its specular and
/// its transparency (a press or a drag on the bar sets it, a double-click
/// puts it back as the ball comes). They change every face of that
/// material at once; each gesture is one undo step.
fn look_slots(ui: &mut Ui, st: &mut AppState, mi: u32, w: f32) {
    let look = st.look(mi);
    let mut held = false;
    let (sr, _) = ui.allocate_exact_size(vec2(w, 7.0), Sense::hover());
    ui.painter().hline(sr.x_range(), sr.center().y, Stroke::new(1.0, DIVIDER()));
    let ctx = ui.ctx().clone();
    let open_id = Id::new("look-color-open");
    let open = ctx.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    // colour
    let (r, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::click());
    ctx.data_mut(|d| d.insert_temp(Id::new(("look-slot", 0usize)), r));
    let hot = resp.hovered() || open;
    slot_frame(ui, r, hot);
    let p = ui.painter();
    p.text(pos2(r.min.x + 7.0, r.center().y), Align2::LEFT_CENTER, "Color", regular(10.5), LABEL());
    let c8 = look.color.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
    let sw = Rect::from_min_size(pos2(r.max.x - 7.0 - 24.0, r.center().y - 5.0), vec2(24.0, 10.0));
    p.rect(sw, 3.0, Color32::from_rgb(c8[0], c8[1], c8[2]), Stroke::new(1.0, tint(Color32::WHITE, 0.28)), StrokeKind::Outside);
    p.text(pos2(sw.min.x - 6.0, r.center().y), Align2::RIGHT_CENTER, format!("#{:02X}{:02X}{:02X}", c8[0], c8[1], c8[2]), mono(9.5), TEXT2());
    if resp.clicked() {
        ctx.data_mut(|d| d.insert_temp(open_id, !open));
    }
    resp.on_hover_text("Base color: click to open the picker");
    if open {
        // the picker beside the island, on its left (it closes on a click
        // anywhere else)
        // its foot level with the island's
        let foot = ctx.memory(|m| m.area_rect(Id::new("materials"))).map(|a| a.max.y).unwrap_or(r.max.y + 60.0);
        let pr = egui::Area::new(Id::new("look-picker")).order(Order::Foreground).pivot(Align2::RIGHT_BOTTOM).fixed_pos(pos2(r.min.x - 16.0, foot)).show(&ctx, |ui| {
            island().inner_margin(Margin::same(8)).show(ui, |ui| color_picker(ui, st, mi, 176.0)).inner
        });
        held |= pr.inner;
        let away = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().map(|q| !pr.response.rect.contains(q) && !r.contains(q)).unwrap_or(false));
        if away || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.data_mut(|d| d.insert_temp(open_id, false));
        }
    }
    ui.add_space(3.0);
    // specular and transparency
    let rows: [(&str, f32, f32, &str); 2] = [("Specular", look.specular, materials::SPECULAR, "Material specular"), ("Transparency", look.transparency, 0.0, "Material transparency")];
    for (k, (label, v, default, step)) in rows.into_iter().enumerate() {
        let (r, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::click_and_drag());
        ctx.data_mut(|d| d.insert_temp(Id::new(("look-slot", k + 1)), r));
        let down = resp.is_pointer_button_down_on() || resp.dragged();
        held |= down;
        let mut nv = None;
        if down {
            if let Some(q) = resp.interact_pointer_pos() {
                nv = Some((((q.x - r.min.x - 2.0) / (r.width() - 4.0)).clamp(0.0, 1.0) * 100.0).round() / 100.0);
            }
        }
        if resp.double_clicked() {
            nv = Some(default);
        }
        let shown = nv.unwrap_or(v);
        slot_frame(ui, r, resp.hovered() || down);
        let p = ui.painter();
        let fill = Rect::from_min_max(r.min, pos2(r.min.x + 2.0 + (r.width() - 4.0) * shown, r.max.y)).shrink(1.5);
        if fill.width() > 0.5 {
            p.rect_filled(fill, 4.0, tint(TEXT_STRONG(), if down { 0.2 } else { 0.13 }));
        }
        p.text(pos2(r.min.x + 7.0, r.center().y), Align2::LEFT_CENTER, label, regular(10.5), LABEL());
        p.text(pos2(r.max.x - 7.0, r.center().y), Align2::RIGHT_CENTER, format!("{} %", (shown * 100.0).round() as i32), regular(10.5), TEXT_STRONG());
        if let Some(x) = nv {
            st.edit_look(mi, step, |l| if k == 0 { l.specular = x } else { l.transparency = x });
        }
        resp.on_hover_text(if k == 0 { "Specular: the highlights' strength (50 % as the ball comes). Drag; double-click: back" } else { "Transparency: 0 % solid. Drag; double-click: back" });
        if k == 0 {
            ui.add_space(3.0);
        }
    }
    // the gesture let go: one undo step
    if !held && st.look_live.is_some() {
        st.end_look();
    }
}

/// A slot's well: rounded, a fine border lighter when hovered.
fn slot_frame(ui: &Ui, r: Rect, hot: bool) {
    ui.painter().rect(r, 5.0, WELL(), Stroke::new(1.0, if hot { TEXT2() } else { DIVIDER() }), StrokeKind::Inside);
}

/// The small colour picker under the Color row: saturation / value in a
/// square, the hue on a bar. Returns whether it is held.
fn color_picker(ui: &mut Ui, st: &mut AppState, mi: u32, w: f32) -> bool {
    use egui::ecolor::{hsv_from_rgb, rgb_from_hsv};
    ui.set_width(w);
    let (tr, _) = ui.allocate_exact_size(vec2(w, 16.0), Sense::hover());
    ui.painter().text(tr.left_center(), Align2::LEFT_CENTER, "COLOR", mono(10.0), LABEL());
    ui.painter().text(tr.right_center(), Align2::RIGHT_CENTER, materials::get(mi).name, regular(10.5), TEXT2());
    // a few hard-surface finishes, one click each
    const SWATCHES: [(&str, [f32; 3]); 8] = [
        ("Anodized black", [0.10, 0.10, 0.11]),
        ("Gunmetal", [0.29, 0.31, 0.34]),
        ("Aluminium", [0.72, 0.73, 0.75]),
        ("Off-white", [0.88, 0.87, 0.83]),
        ("Olive drab", [0.36, 0.38, 0.22]),
        ("Desert tan", [0.72, 0.60, 0.43]),
        ("Safety orange", [0.93, 0.40, 0.10]),
        ("Signal red", [0.70, 0.12, 0.10]),
    ];
    let (row, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
    let sw = (w - 7.0 * 4.0) / 8.0;
    let mut clicked = false;
    for (k, (name, c)) in SWATCHES.iter().enumerate() {
        let r = Rect::from_min_size(pos2(row.min.x + k as f32 * (sw + 4.0), row.min.y + 2.0), vec2(sw, 14.0));
        let resp = ui.interact(r, Id::new(("look-swatch", k)), Sense::click());
        let c8 = c.map(|x| (x * 255.0) as u8);
        ui.painter().rect(r, 3.0, Color32::from_rgb(c8[0], c8[1], c8[2]), Stroke::new(1.0, if resp.hovered() { TEXT_STRONG() } else { tint(Color32::WHITE, 0.18) }), StrokeKind::Inside);
        if resp.clicked() {
            st.edit_look(mi, "Material color", |l| l.color = *c);
            clicked = true;
        }
        resp.on_hover_text(*name);
    }
    if clicked {
        st.end_look();
    }
    let look = st.look(mi);
    // the hue kept while it cannot be read from the colour (a grey)
    let hue_id = Id::new(("look-hue", mi));
    let (mut h, s0, v0) = hsv_from_rgb(look.color);
    if s0 < 1e-3 || v0 < 1e-3 {
        h = ui.ctx().data(|d| d.get_temp::<f32>(hue_id)).unwrap_or(h);
    }
    let (mut s, mut v) = (s0, v0);
    ui.add_space(4.0);
    let (sq, sresp) = ui.allocate_exact_size(vec2(w, 104.0), Sense::click_and_drag());
    ui.add_space(4.0);
    let (hb, hresp) = ui.allocate_exact_size(vec2(w, 11.0), Sense::click_and_drag());
    ui.ctx().data_mut(|d| {
        d.insert_temp(Id::new(("look-slot", 3usize)), sq);
        d.insert_temp(Id::new(("look-slot", 4usize)), hb);
    });
    let sdown = sresp.is_pointer_button_down_on() || sresp.dragged();
    let hdown = hresp.is_pointer_button_down_on() || hresp.dragged();
    let mut changed = false;
    if sdown {
        if let Some(q) = sresp.interact_pointer_pos() {
            s = ((q.x - sq.min.x) / sq.width()).clamp(0.0, 1.0);
            v = 1.0 - ((q.y - sq.min.y) / sq.height()).clamp(0.0, 1.0);
            changed = true;
        }
    }
    if hdown {
        if let Some(q) = hresp.interact_pointer_pos() {
            h = ((q.x - hb.min.x) / hb.width()).clamp(0.0, 0.9999);
            changed = true;
        }
    }
    if changed {
        ui.ctx().data_mut(|d| d.insert_temp(hue_id, h));
        let rgb = rgb_from_hsv((h, s, v));
        st.edit_look(mi, "Material color", |l| l.color = rgb);
    }
    let c32 = |rgb: [f32; 3]| Color32::from_rgb((rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8);
    let p = ui.painter();
    // the square: white to the hue across, black down (exact by corners)
    let pure = c32(rgb_from_hsv((h, 1.0, 1.0)));
    let mut m = egui::Mesh::default();
    m.colored_vertex(sq.left_top(), Color32::WHITE);
    m.colored_vertex(sq.right_top(), pure);
    m.colored_vertex(sq.right_bottom(), Color32::BLACK);
    m.colored_vertex(sq.left_bottom(), Color32::BLACK);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(m);
    p.rect_stroke(sq, 3.0, Stroke::new(1.0, DIVIDER()), StrokeKind::Outside);
    let at = pos2(sq.min.x + s * sq.width(), sq.min.y + (1.0 - v) * sq.height());
    p.circle_stroke(at, 4.0, Stroke::new(1.5, Color32::WHITE));
    p.circle_stroke(at, 5.2, Stroke::new(1.0, Color32::from_black_alpha(140)));
    // the hue bar
    let mut m = egui::Mesh::default();
    let n = 12;
    for k in 0..=n {
        let t = k as f32 / n as f32;
        let c = c32(rgb_from_hsv((t.min(0.9999), 1.0, 1.0)));
        let x = hb.min.x + t * hb.width();
        m.colored_vertex(pos2(x, hb.min.y), c);
        m.colored_vertex(pos2(x, hb.max.y), c);
        if k > 0 {
            let i = (k * 2) as u32;
            m.add_triangle(i - 2, i - 1, i);
            m.add_triangle(i - 1, i + 1, i);
        }
    }
    p.add(m);
    p.rect_stroke(hb, 2.0, Stroke::new(1.0, DIVIDER()), StrokeKind::Outside);
    let hx = hb.min.x + h * hb.width();
    p.rect(Rect::from_center_size(pos2(hx, hb.center().y), vec2(4.0, hb.height() + 4.0)), 2.0, Color32::WHITE, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Outside);
    sdown || hdown
}

/// The ball carried by the pointer, with what it would land on.
fn drag_ball(ctx: &egui::Context, st: &AppState, at: egui::Pos2, m: &materials::Look) {
    let p = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("mat-drag")));
    let c = at + vec2(14.0, -14.0);
    p.circle_filled(c + vec2(0.0, 1.5), 12.0, Color32::from_black_alpha(70));
    material_ball(&p, c, 11.0, m);
    let what = match st.mat_drag.and_then(|d| d.target) {
        Some(DropTarget::Face { .. }) => "a face".to_string(),
        Some(DropTarget::Shape(id)) => st.doc.feature(id).map(|f| f.name.clone()).unwrap_or_default(),
        Some(DropTarget::Node(id, n)) => st.item_name(crate::common::Item::Shape(id, n)),
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
    /// The item it is (an operation, an SDF shape: its switch in the pop-up).
    op: Option<crate::common::Item>,
    on: bool,
}

/// The history strip's icons, square like the component modes' at the top.
/// A history cell (28 points; 48 on the iPad, the mock-up's) and the gap
/// between two.
fn cell_size() -> f32 {
    if tablet() { 48.0 } else { 28.0 }
}
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
    } else if has(&["shell", "open a face", "close a face"]) {
        (Icon::Shell, TEXT2())
    } else if has(&["group"]) {
        (Icon::Group, TEXT2())
    } else if has(&["mirror"]) {
        (Icon::Mirror, TEXT2())
    } else if has(&["array"]) {
        (Icon::Array, TEXT2())
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
        let info = if f.is_path() || f.is_mesh() { f.kind_label().to_string() } else { format!("{} · {}", f.prim.kind().label(), f.op.label()) };
        let mut cells = vec![Cell { icon: feature_icon(&f), color: feature_color(&f), name: f.name.clone(), info, op: None, on: f.visible }];
        // its items: a shape's operations, an SDF object's shapes
        for it in st.element_items(el) {
            let (icon, color) = item_look(st, it);
            let (kind, what) = st.item_info(it);
            cells.push(Cell { icon, color, name: st.item_name(it), info: format!("{kind} · {what}"), op: Some(it), on: st.item_on(it) });
        }
        return (Some(el), cells, st.item_marker(el));
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
            if st.item_marker(el) != i {
                st.set_item_marker(el, i);
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
            let content = if empty { 420.0 } else { 78.0 + 8.0 + 1.0 + 8.0 + (max_w - 140.0).min(n * (cell_size() + CELL_GAP) + 4.0) };
            ui.set_width(content.max(160.0));
            grip(ui, "history");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (tr, _) = ui.allocate_exact_size(vec2(78.0, cell_size()), Sense::hover());
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
                let (sep, _) = ui.allocate_exact_size(vec2(1.0, cell_size() - 4.0), Sense::hover());
                ui.painter().rect_filled(sep, 0.0, DIVIDER());
                if el.is_none() && cells.len() == 1 && st.doc.features.is_empty() {
                    ui.label(egui::RichText::new("Pick a shape on the right, then drag in the view").font(regular(12.5)).color(TEXT2()));
                    return;
                }
                let n = cells.len();
                let w = n as f32 * (cell_size() + CELL_GAP);
                egui::ScrollArea::horizontal().id_salt("history-scroll").max_width((max_w - 140.0).min(w + 4.0)).show(ui, |ui| {
                    let (row, resp) = ui.allocate_exact_size(vec2(w, cell_size()), Sense::click_and_drag());
                    let cell = |i: usize| Rect::from_min_size(pos2(row.min.x + i as f32 * (cell_size() + CELL_GAP), row.min.y), vec2(cell_size(), cell_size()));
                    let idx = |x: f32| ((x - row.min.x + CELL_GAP * 0.5) / (cell_size() + CELL_GAP)).floor().clamp(0.0, n as f32 - 1.0) as usize;
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
                                match cells[i].op.filter(|_| i > 0) {
                                    Some(it) => st.select_item(it),
                                    None => {
                                        st.select_tool();
                                        st.select(Some(e), false);
                                        st.sdf_sel = None;
                                    }
                                }
                            }
                        }
                    }
                    let now_at = match el {
                        Some(e) => st.item_marker(e),
                        None => st.scene_history().1,
                    };
                    let _ = at;
                    let p = ui.painter();
                    for (i, c) in cells.iter().enumerate() {
                        let r = cell(i);
                        ui.ctx().data_mut(|d| d.insert_temp(Id::new(("hist-cell", i)), r.center()));
                        let past = i > now_at;
                        let sel = c.op.map(|it| st.item_selected(it)).unwrap_or(false) || (i == 0 && el.map(|e| st.is_selected(e) && st.sdf_sel.is_none()).unwrap_or(false));
                        if i == now_at {
                            p.rect(r, 7.0, tint(accent(), 0.12), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
                        } else if sel {
                            p.rect(r, 7.0, RAISED(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
                        } else if under == Some(i) && (resp.hovered() || resp.dragged()) {
                            p.rect_filled(r, 7.0, RAISED());
                        }
                        let dim = if past { 0.32 } else if !c.on { 0.45 } else { 1.0 };
                        let ic = if i == now_at { accent() } else { tint(TEXT_ICON(), dim) };
                        let isz = if tablet() { 20.0 } else { 15.0 };
                        icons::paint(p, Rect::from_center_size(r.center(), vec2(isz, isz)), c.icon, ic);
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
                    p.rect_filled(Rect::from_center_size(pos2(m.max.x + CELL_GAP * 0.5, m.center().y), vec2(2.0, cell_size() - 6.0)), 1.0, TEXT_STRONG());
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
            if let Some(it) = c.op {
                ui.add_space(3.0);
                let at = ui.cursor().min + vec2(7.0, 10.0);
                ui.ctx().data_mut(|d| d.insert_temp(Id::new("history-tick"), at));
                if tick_row(ui, if c.on { "On" } else { "Off: left out of the build" }, c.on) {
                    st.toggle_item(it);
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
        Tool::Curve(k) => format!("Curve · {}", k.label()),
        Tool::Push(k) => k.label().into(),
        Tool::Knife => "Knife".into(),
        Tool::Point => "Point on edge".into(),
        Tool::Slot if st.slot_on_scan => "Slot · on the scan".into(),
        Tool::Slot => format!("Slot · {}", st.mode.label()),
        Tool::Form => format!("Forms · {}", st.form.brush.label()),
        Tool::Align => "Align · pick the target".into(),
        Tool::ShellPick => "Shell · pick faces to open".into(),
        Tool::Select if st.align_open() => "Align".into(),
        Tool::Select => format!("{} · {}", st.gizmo.label(), st.sel_mode.label()),
        Tool::Sculpt => format!("Sculpt · {}", st.sculpt.brush.label()),
        Tool::PolyLoop => "Insert edge loop".into(),
        Tool::PolyCut => "Cut".into(),
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
        Tool::Curve(_) => PATH,
        Tool::Push(_) => FACE_SEL,
        Tool::Knife | Tool::Point => EDGE_SEL,
        Tool::Slot => accent(),
        Tool::Form => op_color(st.mode),
        Tool::Align => TEXT2(),
        Tool::ShellPick => FACE_SEL,
        Tool::Select if st.sel_mode == SelMode::Edge => EDGE_SEL,
        Tool::Select if st.sel_mode == SelMode::Face => FACE_SEL,
        Tool::Select => st.single().map(feature_color).unwrap_or(TEXT2()),
        Tool::Sculpt | Tool::PolyLoop | Tool::PolyCut => accent(),
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
/// Where the selection's small bar sits (its bottom centre): above the
/// selected shapes' box on screen. None when it does not show.
fn context_bar_at(st: &AppState) -> Option<egui::Pos2> {
    if st.tool != Tool::Select || st.creation.is_some() || st.drag.is_some() || st.selection.is_empty() {
        return None;
    }
    // a Poly mesh's components picked or pulled: it would sit on them
    if st.poly_comp() || st.poly_pull.is_some() || st.poly_drag.is_some() || st.stroke.is_some() || st.poly_bevel.is_some() || st.tool == Tool::PolyCut {
        return None;
    }
    // an inset or extrusion, an Array: their handles are there instead
    if st.selected_push().is_some() || st.selected_array().is_some() {
        return None;
    }
    let b = st.doc.features.iter().filter(|f| st.selection.contains(&f.id)).map(|f| f.bounds()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)));
    let (lo, hi) = b?;
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
        return None;
    }
    let vr = st.view_rect;
    // below the hint line at the top of the view (a group's bar goes above it)
    let room = if st.shown_group().is_some() { 126.0 } else { 84.0 };
    Some(pos2(((x0 + x1) * 0.5).clamp(vr[0] + 120.0, vr[2] - 120.0), (top - 16.0).clamp(vr[1] + room, vr[3] - 40.0)))
}

fn context_bar(ctx: &egui::Context, st: &mut AppState) {
    let Some(at) = context_bar_at(st) else { return };
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
                    // several shapes that are not one group yet
                    if st.can_group() && st.selection.len() > 1 {
                        let r = icon_btn(ui, Icon::Group, false, "Group (Ctrl+G): these shapes in one group with its own transform, closed (a click takes them all)", 28.0);
                        ui.ctx().data_mut(|d| d.insert_temp(Id::new("ctx-group"), r.rect.center()));
                        if r.clicked() {
                            st.group_selection();
                        }
                    }
                    if icon_btn(ui, Icon::Align, st.tool == Tool::Align, "Align (Alt+A): then click the shape to align to", 28.0).clicked() {
                        st.begin_align();
                    }
                    if icon_btn(ui, Icon::Mirror, false, "Mirror (Alt+M): the part made symmetric across X, the steps after it on both sides", 28.0).clicked() {
                        st.add_mirror(0);
                    }
                    if icon_btn(ui, Icon::Array, false, "Array (Ctrl+Shift+D): copies in a line, in rows or round an axis, with their operations", 28.0).clicked() {
                        st.add_array();
                    }
                    if icon_btn(ui, Icon::Shell, false, "Shell (Alt+S): the part hollowed, 2 mm walls (Face mode: the faces picked left open)", 28.0).clicked() {
                        st.add_shell();
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

/// A group's small bar above it in the view (Romain 21 nov., 12 h 37:
/// "quand tu sélectionnes un groupe, il y a un petit pop-up au-dessus,
/// groupe est fermé"): its name, Closed or Open (a click opens it or
/// closes it again), Ungroup. Shown for the selected group, and for the
/// open group a selected shape is in.
fn group_bar(ctx: &egui::Context, st: &mut AppState) {
    if st.tool != Tool::Select || st.creation.is_some() || st.drag.is_some() || st.align.is_some() || st.marking.is_open() {
        return;
    }
    let Some(g) = st.shown_group().cloned() else { return };
    let Some(corners) = st.group_corners(&g) else { return };
    let view = st.view();
    let mut top = f32::INFINITY;
    let (mut x0, mut x1) = (f32::INFINITY, f32::NEG_INFINITY);
    for c in corners {
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
    let mut at = pos2(((x0 + x1) * 0.5).clamp(vr[0] + 130.0, vr[2] - 130.0), (top - 14.0).clamp(vr[1] + 84.0, vr[3] - 82.0));
    // above the selection's own bar when they would meet
    if let Some(b) = context_bar_at(st) {
        if (b.x - at.x).abs() < 250.0 && (b.y - at.y).abs() < 42.0 || (b.x - at.x).abs() < 250.0 && at.y > b.y {
            at.y = b.y - 42.0;
        }
    }
    egui::Area::new(Id::new("group-bar")).order(Order::Middle).pivot(Align2::CENTER_BOTTOM).fixed_pos(at).show(ctx, |ui| {
        island().inner_margin(Margin::same(3)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let n = g.members.len();
                let name = ui.painter().layout_no_wrap(g.name.clone(), semibold(12.5), TEXT());
                let count = ui.painter().layout_no_wrap(format!("{n} shape{}", if n > 1 { "s" } else { "" }), regular(11.5), TEXT2());
                let w = 30.0 + name.size().x.min(150.0) + 8.0 + count.size().x + 8.0;
                let (r, _) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
                icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 15.0, r.center().y), vec2(15.0, 15.0)), Icon::Group, TEXT());
                let nx = r.min.x + 30.0;
                ui.painter().with_clip_rect(Rect::from_min_max(pos2(nx, r.min.y), pos2(nx + 150.0, r.max.y))).galley(pos2(nx, r.center().y - name.size().y * 0.5), name.clone(), TEXT());
                ui.painter().galley(pos2(nx + name.size().x.min(150.0) + 8.0, r.center().y - count.size().y * 0.5), count, TEXT2());
                let (sr, _) = ui.allocate_exact_size(vec2(7.0, 28.0), Sense::hover());
                ui.painter().rect_filled(Rect::from_center_size(sr.center(), vec2(1.0, 18.0)), 0.0, DIVIDER());
                // Closed / Open: the state, and the switch
                let label = if g.open { "Open" } else { "Closed" };
                let lg = ui.painter().layout_no_wrap(label.to_string(), semibold(12.0), TEXT());
                let (r, resp) = ui.allocate_exact_size(vec2(34.0 + lg.size().x + 8.0, 28.0), Sense::click());
                let on = g.open;
                let bg = match (on, resp.hovered()) {
                    (true, _) => accent_bg(),
                    (false, true) => RAISED(),
                    _ => Color32::TRANSPARENT,
                };
                ui.painter().rect(r, 7.0, bg, Stroke::new(1.0, if on { accent_border() } else { DIVIDER() }), StrokeKind::Inside);
                let fg = if on { accent() } else { TEXT() };
                icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 16.0, r.center().y), vec2(15.0, 15.0)), if on { Icon::Unlock } else { Icon::Lock }, fg);
                ui.painter().galley(pos2(r.min.x + 30.0, r.center().y - lg.size().y * 0.5), lg, fg);
                ui.ctx().data_mut(|d| d.insert_temp(Id::new("group-bar-toggle"), r.center()));
                let tip = if on { "Open: its shapes are picked and moved one by one. Click: close it again" } else { "Closed: a click on one of its shapes takes the whole group. Click: open it, to pick its shapes one by one" };
                if resp.on_hover_text(tip).clicked() {
                    st.set_group_open(g.id, !g.open);
                }
                let ug = ui.painter().layout_no_wrap("Ungroup".to_string(), regular(12.0), TEXT2());
                let (r, resp) = ui.allocate_exact_size(vec2(ug.size().x + 18.0, 28.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, 7.0, RAISED());
                }
                ui.painter().galley(pos2(r.min.x + 9.0, r.center().y - ug.size().y * 0.5), ug, if resp.hovered() { TEXT() } else { TEXT2() });
                ui.ctx().data_mut(|d| d.insert_temp(Id::new("group-bar-ungroup"), r.center()));
                if resp.on_hover_text("Ungroup (Ctrl+Shift+G): the group goes, its shapes stay where they are").clicked() {
                    st.ungroup(g.id);
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
    /// Two closed curves or more picked: Union, Subtract, Intersect.
    Curves,
    /// The Poly tab's Bevel under way or just made.
    PolyBevel,
}

fn op_pop(st: &AppState) -> Option<OpPop> {
    if st.poly_bevel_valid() {
        return Some(OpPop::PolyBevel);
    }
    if st.align.is_some() {
        return None;
    }
    if (st.tool == Tool::Create(PrimKind::Surface) || st.tool == Tool::Create(PrimKind::Path) || st.tool == Tool::Form || (st.tool == Tool::Slot && st.slot_draw.is_none())) && st.creation.is_none() {
        return Some(OpPop::Tool);
    }
    // picking a Shell's faces: its window stays
    if st.tool == Tool::ShellPick {
        return st.selected_shell().map(|f| OpPop::Step(f.id));
    }
    if st.creation.is_some() || st.sketch.is_some() || st.push.is_some() || st.curve_pull.is_some() || st.tool != Tool::Select || st.poly_comp() || st.poly_pull.is_some() {
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
    // two curves or more: a 2D boolean (closed) or a loft
    if st.sel_mode == SelMode::Object && st.selection.len() >= 2 && st.selection.iter().all(|id| st.doc.feature(*id).map(|f| f.is_path()).unwrap_or(false)) {
        return Some(OpPop::Curves);
    }
    // a group (of one shape): its transform is in the panel
    if st.selected_group().is_some() {
        return None;
    }
    let f = st.single()?;
    Some(if !f.is_solid() || f.is_path() || f.loft.is_some() { OpPop::Step(f.id) } else { OpPop::Shape(f.id) })
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
                let l = st.curve_lines(f).concat();
                if l.is_empty() { f.corners().to_vec() } else { l }
            } else if let Some(t) = &f.tweak {
                vec![t.at() + t.delta]
            } else if let Some(r) = &f.round {
                r.edges.iter().map(|e| e.at).collect()
            } else if let Some((_, q)) = st.mirror_plane().filter(|_| f.mirror.is_some()) {
                q.to_vec()
            } else if let Some((_, q)) = st.array_marks().filter(|_| f.array.is_some()) {
                q
            } else {
                vec![f.origin]
            }
        }
        // an SDF object with a shape picked: by that shape (`common::Item`)
        OpPop::Shape(id) => match st.sdf_sel.filter(|(f, _)| *f == id && st.sdf_comp()) {
            Some((f, n)) => st.item_points(crate::common::Item::Shape(f, n)),
            None => vec![st.doc.feature(id)?.origin],
        },
        OpPop::PolyBevel => {
            let b = st.poly_bevel.as_ref()?;
            let f = st.doc.feature(b.id)?;
            let m = f.mesh.as_ref()?;
            let pts = crate::meshes::world_points(f);
            b.faces.iter().filter_map(|fi| m.faces.get(*fi as usize)).flat_map(|q| q.iter().filter_map(|v| pts.get(*v as usize).map(|p| p.as_dvec3()))).collect()
        }
        OpPop::Curves => st.selection.iter().filter_map(|id| st.doc.feature(*id)).flat_map(|f| st.curve_lines(f).concat()).collect(),
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

/// The Align window (3ds Max's Align, `align.rs`): what lines up with
/// what, the axes, the point of each, the orientation; the part shows it
/// live. Apply keeps it and goes on, OK keeps it, Cancel puts it back.
fn align_window(ctx: &egui::Context, st: &mut AppState) {
    if !st.align_open() || st.marking.is_open() {
        return;
    }
    let Some(a) = st.align.clone() else { return };
    const W: f32 = 236.0;
    let vr = st.view_rect;
    let full = W + 20.0;
    let h = ctx.memory(|m| m.area_rect(Id::new("align-pop"))).map(|r| r.height()).unwrap_or(300.0);
    // beside the part (as it was when the window opened), on its right,
    // else on its left, else by the click on the target
    let view = st.view();
    let mut part = Rect::from_min_max(pos2(a.at[0], a.at[1]), pos2(a.at[0], a.at[1]));
    let mut corners: Vec<DVec3> = a.boxes.iter().flat_map(|(_, b)| [b.lo, b.hi]).collect();
    if let Some(t) = &a.tbox {
        corners.extend([t.lo, t.hi]);
    }
    if let Some((lo, hi)) = st.model.as_ref().and_then(|m| m.bounds()) {
        corners.extend([DVec3::from(lo.map(|x| x as f64)), DVec3::from(hi.map(|x| x as f64))]);
    }
    for w in corners.chunks(2) {
        for i in 0..8 {
            let c = DVec3::new(if i & 1 == 0 { w[0].x } else { w[1].x }, if i & 2 == 0 { w[0].y } else { w[1].y }, if i & 4 == 0 { w[0].z } else { w[1].z });
            if let Some(q) = view.project(c.as_vec3()) {
                part = part.union(Rect::from_min_max(pos2(q[0], q[1]), pos2(q[0], q[1])));
            }
        }
    }
    let gap = 28.0;
    let (left, right) = (vr[0] + 8.0, vr[2] - 8.0);
    let x = if part.max.x + gap + full < right {
        part.max.x + gap
    } else if part.min.x - gap - full > left {
        part.min.x - gap - full
    } else if a.at[0] + 70.0 + full < right {
        a.at[0] + 70.0
    } else {
        (a.at[0] - 70.0 - full).max(left)
    };
    let top = vr[1] + 48.0;
    let mut y = (a.at[1] - 110.0).clamp(top, (vr[3] - 8.0 - h).max(top));
    // not over the help box (bottom right): above it
    if let Some(hb) = ctx.memory(|m| m.area_rect(Id::new("help-box"))) {
        if Rect::from_min_size(pos2(x, y), vec2(full, h)).intersects(hb) {
            y = (hb.min.y - 8.0 - h).max(top);
        }
    }
    let at = pos2(x, y) + panel_offset(ctx, "align-pop");
    let (what, target) = st.align_names();
    let mut changed = false;
    let mut close: Option<bool> = None;
    let mut apply = false;
    egui::Area::new(Id::new("align-pop")).order(Order::Foreground).fixed_pos(at).show(ctx, |ui| {
        island().inner_margin(Margin::same(10)).show(ui, |ui| {
            ui.set_width(W);
            grip(ui, "align-pop");
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Align").font(semibold(14.0)).color(TEXT()));
                chip_label(ui, "Alt + A", RAISED(), LABEL());
            });
            // what moves, and onto what
            let (r, _) = ui.allocate_exact_size(vec2(W, 20.0), Sense::hover());
            let p = ui.painter();
            let g1 = p.layout_no_wrap(what.clone(), medium(12.0), TEXT_STRONG());
            let g2 = p.layout_no_wrap(target.clone(), medium(12.0), accent());
            let x1 = r.min.x + g1.size().x;
            p.galley(pos2(r.min.x, r.center().y - g1.size().y / 2.0), g1, TEXT_STRONG());
            p.text(pos2(x1 + 8.0, r.center().y), Align2::LEFT_CENTER, "→", regular(12.0), LABEL());
            p.galley(pos2(x1 + 24.0, r.center().y - g2.size().y / 2.0), g2, accent());
            ui.add_space(4.0);
            let label = |ui: &mut Ui, t: &str| {
                let (r, _) = ui.allocate_exact_size(vec2(W, 16.0), Sense::hover());
                ui.painter().text(r.left_center(), Align2::LEFT_CENTER, t, mono(10.0), LABEL());
            };
            // the axes (3ds Max's X / Y / Z Position)
            label(ui, "POSITION");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let bw = (W - 12.0) / 3.0;
                for (k, (n, c)) in [("X", AXIS_X), ("Y", AXIS_Y), ("Z", AXIS_Z)].into_iter().enumerate() {
                    let on = a.axes[k];
                    let (r, resp) = ui.allocate_exact_size(vec2(bw, 24.0), Sense::click());
                    ui.ctx().data_mut(|d| d.insert_temp(Id::new(("align-axis", k)), r));
                    let p = ui.painter();
                    p.rect(r, 6.0, if on { RAISED() } else { WELL() }, Stroke::new(1.0, if on { c } else if resp.hovered() { TEXT2() } else { DIVIDER() }), StrokeKind::Inside);
                    // a tick box and the axis
                    let b = Rect::from_center_size(pos2(r.center().x - 9.0, r.center().y), vec2(11.0, 11.0));
                    p.rect(b, 3.0, if on { c } else { Color32::TRANSPARENT }, Stroke::new(1.0, if on { c } else { LABEL() }), StrokeKind::Inside);
                    if on {
                        p.line_segment([b.left_center() + vec2(2.5, 0.0), b.center_bottom() + vec2(-0.5, -2.5)], Stroke::new(1.4, Color32::BLACK));
                        p.line_segment([b.center_bottom() + vec2(-0.5, -2.5), b.right_top() + vec2(-2.5, 2.5)], Stroke::new(1.4, Color32::BLACK));
                    }
                    p.text(pos2(r.center().x + 3.0, r.center().y), Align2::LEFT_CENTER, n, semibold(12.0), if on { c } else { TEXT2() });
                    if resp.clicked() {
                        if let Some(al) = st.align.as_mut() {
                            al.axes[k] = !al.axes[k];
                        }
                        changed = true;
                    }
                }
            });
            ui.add_space(4.0);
            let items: Vec<(crate::align::AlignPoint, &str, Color32)> = crate::align::AlignPoint::ALL.iter().map(|p| (*p, p.label(), TEXT2())).collect();
            label(ui, &format!("CURRENT · {}", what.to_uppercase()));
            if let Some(v) = ui.push_id("align-cur", |ui| segmented(ui, &items, a.cur, 26.0)).inner {
                if let Some(al) = st.align.as_mut() {
                    al.cur = v;
                }
                changed = true;
            }
            ui.add_space(2.0);
            label(ui, &format!("TARGET · {}", target.to_uppercase()));
            if let Some(v) = ui.push_id("align-tgt", |ui| segmented(ui, &items, a.tgt, 26.0)).inner {
                if let Some(al) = st.align.as_mut() {
                    al.tgt = v;
                }
                changed = true;
            }
            ui.add_space(2.0);
            label(ui, "ORIENTATION");
            if toggle_row(ui, "Match the target's", a.orient) {
                if let Some(al) = st.align.as_mut() {
                    al.orient = !al.orient;
                }
                changed = true;
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let bw = (W - 12.0) / 3.0;
                let r = secondary_btn(ui, "Cancel", bw);
                ui.ctx().data_mut(|d| d.insert_temp(Id::new(("align-btn", 0usize)), r.rect));
                if r.on_hover_text("Esc: everything back as it was").clicked() {
                    close = Some(false);
                }
                let r = secondary_btn(ui, "Apply", bw);
                ui.ctx().data_mut(|d| d.insert_temp(Id::new(("align-btn", 1usize)), r.rect));
                if r.on_hover_text("Keep it and go on (the axes cleared, as in 3ds Max)").clicked() {
                    apply = true;
                }
                let r = primary_btn(ui, None, "OK", Some(bw));
                ui.ctx().data_mut(|d| d.insert_temp(Id::new(("align-btn", 2usize)), r.rect));
                if r.on_hover_text("Enter: keep it").clicked() {
                    close = Some(true);
                }
            });
        });
    });
    if changed {
        st.align_update();
    }
    if apply {
        st.align_apply();
    }
    match close {
        Some(true) => st.align_ok(),
        Some(false) => st.align_cancel(),
        None => {}
    }
}

/// What the operation window works on, as a key: a new key is a new
/// subject (a new selection, a tool run again), and the window takes a new
/// place; adding to what is picked keeps the first one, so it stays.
fn op_subject(st: &AppState, k: OpPop) -> String {
    match k {
        OpPop::Edges => format!("Edges {:?}", st.edge_sel.first().map(|e| e.faces)),
        OpPop::Faces => format!("Faces {:?}", st.face_sel.first().map(|f| f.tag)),
        OpPop::Verts => format!("Verts {:?}", st.vert_sel.first().map(|v| (*v * 100.0).round())),
        OpPop::Curves => format!("Curves {:?}", st.selection.first()),
        OpPop::Tool => format!("Tool {:?}", st.tool),
        _ => format!("{k:?}"),
    }
}

/// The operation window's width (its content; the card adds its margins).
const OP_W: f32 = 288.0;

/// Where the operation window goes for a new subject: at the component's
/// height, beside the part so it covers neither the part nor the gizmo. The
/// part is what is on screen of the kernel's mesh, the paths and the
/// subject's own points; of the places round it (its right, its left, below,
/// above, the view's sides) the one that covers least of it, of the
/// component and of the help box, then the nearest the component. A tool's
/// options go at the view's top right.
fn op_place(ctx: &egui::Context, st: &AppState, k: OpPop, anchor: Rect, h: f32) -> egui::Pos2 {
    let full = OP_W + 26.0;
    let vr = st.view_rect;
    let (left, right, top, bottom) = (vr[0] + 8.0, vr[2] - 8.0, vr[1] + 48.0, vr[3] - 8.0);
    if k == OpPop::Tool {
        return pos2(right - full, top + 60.0);
    }
    let view = st.view();
    let mut part = anchor;
    let mut add = |p: Option<[f32; 2]>| {
        if let Some(p) = p {
            part = part.union(Rect::from_min_max(pos2(p[0], p[1]), pos2(p[0], p[1])));
        }
    };
    if let Some(m) = st.model.as_ref() {
        // the mesh's points (some 4000 of them), not its box's corners: a
        // disc's box reaches far past it on screen
        let stride = (m.positions.len() / 4000).max(1);
        for q in m.positions.iter().step_by(stride) {
            add(view.project(glam::Vec3::from(*q)));
        }
    }
    // the Sculpt and Poly meshes: the kernel never sees them
    for f in st.doc.features.iter().filter(|f| f.visible && f.is_mesh()) {
        for q in f.corners() {
            add(view.project(q.as_vec3()));
        }
    }
    // the paths too: they have no mesh
    for f in st.doc.features.iter().filter(|f| f.visible && f.is_path()) {
        for q in st.curve_lines(f).concat() {
            add(view.project(q.as_vec3()));
        }
    }
    let part = part.intersect(Rect::from_min_max(pos2(vr[0], vr[1]), pos2(vr[2], vr[3])));
    let help = ctx.memory(|m| m.area_rect(Id::new("help-box")));
    // the other islands on screen (the panel, the Sub Outliner, the rail,
    // the history, the materials): not over them either
    let islands: Vec<Rect> = ctx.memory(|m| {
        m.areas()
            .visible_layer_ids()
            .into_iter()
            .filter(|l| matches!(l.order, Order::Middle | Order::Foreground) && l.id != Id::new("op-pop") && l.id != Id::new("help-box"))
            .filter_map(|l| m.area_rect(l.id))
            .filter(|r| r.is_positive() && r.area() < (vr[2] - vr[0]) * (vr[3] - vr[1]) * 0.5)
            .collect()
    });
    // the iPad's action bar, shown this frame for the first time (a new
    // area is not listed as visible on its first frame)
    let mut islands = islands;
    if let Some(r) = ctx.data(|d| d.get_temp::<Rect>(Id::new("t-actions-rect"))) {
        islands.push(r);
    }
    let gap = 28.0;
    let y_by = anchor.center().y - 60.0;
    let x_by = anchor.center().x - full * 0.5;
    let cands = [
        pos2(part.max.x + gap, y_by),
        pos2(part.min.x - gap - full, y_by),
        pos2(x_by, part.max.y + gap),
        pos2(x_by, part.min.y - gap - h),
        pos2(right - full, y_by),
        pos2(left, y_by),
        pos2(right - full, top),
        pos2(left, top),
    ];
    let area = |a: Rect, b: Rect| {
        let i = a.intersect(b);
        if i.is_positive() { i.area() } else { 0.0 }
    };
    let near = anchor.expand(36.0);
    let mut best: Option<(f32, egui::Pos2)> = None;
    for c in cands {
        let at = pos2(c.x.clamp(left, (right - full).max(left)), c.y.clamp(top, (bottom - h).max(top)));
        let r = Rect::from_min_size(at, vec2(full, h));
        let mut score = area(r, part) + 6.0 * area(r, near) + help.map(|hb| 3.0 * area(r, hb.expand(8.0))).unwrap_or(0.0);
        score += islands.iter().map(|i| 4.0 * area(r, i.expand(6.0))).sum::<f32>();
        score += 0.5 * r.distance_to_pos(anchor.center()) * 40.0;
        if best.map(|b| score < b.0).unwrap_or(true) {
            best = Some((score, at));
        }
    }
    best.map(|b| b.1).unwrap_or(pos2(right - full, top))
}

/// The settings of the operation under way, in a small window next to the
/// component it works on (Romain 18 nov., 22 h 31: "en pop-up fenêtre avec
/// grip de déplacement… comme dans Maya quand on fait un Bevel"). It takes
/// its place when its subject is new (a selection, a tool run again), then
/// stays there while the view turns or the part changes (Romain 20 nov.,
/// 19 h 01: "après elle ne suit pas en permanence"); its grip moves it
/// (double-click: back to its place).
fn op_window(ctx: &egui::Context, st: &mut AppState) {
    // the marking menu open: nothing over it
    if st.marking.is_open() {
        return;
    }
    let key = Id::new("op-pop-subject");
    let base_key = Id::new("op-pop-base");
    let shown = op_pop(st).and_then(|k| op_anchor(st, k).map(|a| (k, a)));
    let Some((k, anchor)) = shown else {
        // hidden: the next subject takes a place of its own, even the same one
        ctx.data_mut(|d| d.remove::<String>(key));
        return;
    };
    let subject = op_subject(st, k);
    let h = ctx.memory(|m| m.area_rect(Id::new("op-pop"))).map(|r| r.height()).unwrap_or(200.0);
    let known = ctx.data(|d| d.get_temp::<String>(key));
    let kept = ctx.data(|d| d.get_temp::<egui::Pos2>(base_key));
    // the window's own controls changed its subject (a fillet made from the
    // picked edges, a curve picked in a boolean's list): it stays where it is
    let over = ctx.input(|i| i.pointer.latest_pos()).zip(ctx.memory(|m| m.area_rect(Id::new("op-pop")))).map(|(p, r)| r.expand(6.0).contains(p)).unwrap_or(false);
    let base = match kept {
        Some(b) if known.as_deref() == Some(subject.as_str()) => b,
        Some(b) if known.is_some() && over => {
            ctx.data_mut(|d| d.insert_temp(key, subject));
            b
        }
        _ => {
            let at = op_place(ctx, st, k, anchor, h);
            ctx.data_mut(|d| {
                d.insert_temp(key, subject);
                d.insert_temp(base_key, at);
                d.insert_persisted(Id::new(("panel-offset", "op-pop")), egui::Vec2::ZERO);
            });
            at
        }
    };
    // kept inside the view as it grows or the window is resized
    let vr = st.view_rect;
    let full = OP_W + 26.0;
    let (left, right, top, bottom) = (vr[0] + 8.0, vr[2] - 8.0, vr[1] + 48.0, vr[3] - 8.0);
    let at = base + panel_offset(ctx, "op-pop");
    let at = pos2(at.x.clamp(left, (right - full).max(left)), at.y.clamp(top, (bottom - h).max(top)));
    set_op_style(ctx, true);
    egui::Area::new(Id::new("op-pop")).order(Order::Foreground).fixed_pos(at).show(ctx, |ui| {
        op_island().show(ui, |ui| {
            ui.set_width(OP_W);
            grip(ui, "op-pop");
            let mut body = |ui: &mut Ui| op_body(ui, st, k);
            if tablet() {
                // the iPad: a tall window (an SDF object's) scrolls in the
                // view's height, under the top islands
                egui::ScrollArea::vertical().id_salt("op-pop-scroll").max_height((bottom - top - 120.0).max(200.0)).min_scrolled_height((bottom - top - 120.0).max(200.0)).auto_shrink([false, true]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded).show(ui, |ui| {
                    ui.set_width(OP_W);
                    body(ui)
                });
            } else {
                body(ui);
            }
        });
    });
    set_op_style(ctx, false);
}

/// The operation window's content for its subject (also the Studio
/// interface's inspector).
fn op_body(ui: &mut Ui, st: &mut AppState, k: OpPop) {
    match k {
        OpPop::Edges => round_card(ui, st),
        OpPop::Faces => face_card(ui, st),
        OpPop::Verts => {
            let n = st.vert_sel.len();
            op_title(ui, &if n > 1 { format!("{n} vertices") } else { "Vertex".to_string() }, "");
            vertex_card(ui, st);
        }
        OpPop::Curves => combine_card(ui, st),
        OpPop::PolyBevel => poly_bevel_card(ui, st),
        OpPop::Step(id) => {
            let Some(f) = st.doc.feature(id).cloned() else { return };
            op_title(ui, &f.name, f.kind_label());
            // a curve of a 2D boolean: back to it
            if let Some(b) = st.curve_inside(f.id).map(|b| (b.id, b.name.clone())) {
                let r = ui.add(egui::Label::new(egui::RichText::new(format!("‹ In {}", b.1)).font(medium(12.0)).color(LABEL())).sense(Sense::click()));
                if r.on_hover_text("Select the 2D boolean it is in").clicked() {
                    st.selection = vec![b.0];
                }
            }
            if f.loft.is_some() {
                loft_card(ui, st, &f);
            } else if f.shape_bool.is_some() {
                bool_card(ui, st, &f);
                curve_make_card(ui, st, &f);
            } else if f.push.is_some() {
                push_card(ui, st);
            } else if f.tweak.is_some() {
                tweak_card(ui, st, id);
            } else if f.curve.is_some() {
                curve_card(ui, st, &f);
                curve_make_card(ui, st, &f);
            } else if f.is_path() {
                path_card(ui, st, &f);
                curve_make_card(ui, st, &f);
            } else if f.shell.is_some() {
                shell_card(ui, st, &f);
            } else if f.mirror.is_some() {
                mirror_card(ui, st, &f);
            } else if f.array.is_some() {
                array_card(ui, st, &f);
            } else {
                round_card(ui, st);
            }
        }
        OpPop::Tool if st.tool == Tool::Slot => slot_options(ui, st),
        OpPop::Tool if st.tool == Tool::Form => form_options(ui, st),
        OpPop::Tool if st.tool == Tool::Create(PrimKind::Path) => path_options(ui, st),
        OpPop::Tool => surface_options(ui, st),
        OpPop::Shape(id) => {
            let Some(f) = st.doc.feature(id).cloned() else { return };
            op_title(ui, &f.name, f.kind_label());
            shape_card(ui, st, &f);
        }
    }
}

/// The operation window's title: the name, and its kind when the name does
/// not say it already ("Array 2" is an Array).
fn op_title(ui: &mut Ui, name: &str, kind: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(name).font(semibold(15.0)).color(TEXT_STRONG()));
        if !kind.is_empty() && !name.starts_with(kind) {
            chip_label(ui, kind, RAISED(), LABEL());
        }
    });
}

/// A Shell step: the walls' thickness and their way, the faces open (each
/// closed again by its ×), Pick faces to open more.
fn shell_card(ui: &mut Ui, st: &mut AppState, f: &nurbs_kernel::Feature) {
    let any_down = ui.input(|i| i.pointer.any_down());
    let Some(sh) = f.shell.clone() else { return };
    section(ui, "Walls");
    let small = st.model.as_ref().and_then(|m| m.bounds()).map(|(lo, hi)| (0..3).map(|a| hi[a] - lo[a]).fold(f32::MAX, f32::min)).unwrap_or(40.0);
    let span = (small * 0.5).max(5.0).max(sh.thickness as f32).ceil();
    let mut t = sh.thickness as f32;
    let (ch, start) = slider_row(ui, "Thickness", &mut t, 0.1..=span, 0.1, "mm");
    if ch {
        st.set_shell(nurbs_kernel::Shell { thickness: (t as f64).max(0.05), ..sh.clone() }, start || !st.is_live());
    }
    let c = TEXT();
    if let Some(out) = ui.push_id("shell-way", |ui| segmented(ui, &[(false, "Inward", c), (true, "Outward", c)], sh.outward, 28.0)).inner {
        if out != sh.outward {
            st.set_shell(nurbs_kernel::Shell { outward: out, ..sh.clone() }, true);
            st.end_live();
        }
    }
    section(ui, "Open faces");
    if sh.faces.is_empty() {
        ui.label(egui::RichText::new("None: a closed cavity inside (Alt+X: X-ray)").font(regular(12.0)).color(TEXT2()));
    }
    for (k, r) in sh.faces.iter().enumerate() {
        let name = st.shell_face_label(r);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{}  {name}", k + 1)).font(medium(13.0)).color(TEXT()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Label::new(egui::RichText::new("×").font(medium(14.0)).color(LABEL())).sense(Sense::click())).on_hover_text("Close it again").clicked() {
                    st.shell_close_face(k);
                }
            });
        });
    }
    ui.add_space(2.0);
    let picking = st.tool == Tool::ShellPick;
    if secondary_btn(ui, if picking { "Done  Esc" } else { "Pick faces to open" }, ui.available_width()).clicked() {
        st.shell_pick_toggle();
    }
    if let Some(e) = st.feature_error(f) {
        ui.label(egui::RichText::new(e).font(regular(12.0)).color(carve()));
    }
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() && st.push.is_none() {
        st.end_live();
    }
}

/// A Mirror step: its plane (the axis it is square to, where it passes)
/// and the side kept.
fn mirror_card(ui: &mut Ui, st: &mut AppState, f: &nurbs_kernel::Feature) {
    let any_down = ui.input(|i| i.pointer.any_down());
    let Some(m) = f.mirror else { return };
    section(ui, "Plane");
    let items: Vec<(u8, &str, Color32)> = vec![(0, "X", AXIS_X), (1, "Y", AXIS_Y), (2, "Z", AXIS_Z)];
    if let Some(a) = ui.push_id("mirror-axis", |ui| segmented(ui, &items, m.axis, 28.0)).inner {
        if a != m.axis {
            let flip = st.model.as_ref().and_then(|md| md.bounds()).map(|(lo, hi)| (lo[a as usize] + hi[a as usize]) < -1e-3).unwrap_or(false);
            st.set_mirror(nurbs_kernel::Mirror { axis: a, offset: 0.0, flip }, true);
            st.end_live();
        }
    }
    let span = st.model.as_ref().and_then(|md| md.bounds()).map(|(lo, hi)| lo[m.axis as usize].abs().max(hi[m.axis as usize].abs())).unwrap_or(50.0).max(10.0).ceil();
    let mut off = m.offset as f32;
    let (ch, start) = slider_row(ui, "Offset", &mut off, -span..=span, 0.1, "mm");
    if ch {
        st.set_mirror(nurbs_kernel::Mirror { offset: off as f64, ..m }, start || !st.is_live());
    }
    if toggle_row(ui, &format!("Keep the −{} side", m.axis_name()), m.flip) {
        st.set_mirror(nurbs_kernel::Mirror { flip: !m.flip, ..m }, true);
        st.end_live();
    }
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() && st.push.is_none() {
        st.end_live();
    }
}

/// A Pattern / Array step: Line, Grid or Circle, how many and how far;
/// what it repeats.
fn array_card(ui: &mut Ui, st: &mut AppState, f: &nurbs_kernel::Feature) {
    use nurbs_kernel::ArrayKind;
    let any_down = ui.input(|i| i.pointer.any_down());
    let Some(a) = f.array.clone() else { return };
    let note = |ui: &mut Ui, t: &str| {
        ui.label(egui::RichText::new(t).font(regular(11.5)).color(LABEL()));
    };
    ui.add_space(4.0);
    let c = TEXT();
    let kinds: Vec<(ArrayKind, &str, Color32)> = vec![(ArrayKind::Line, "Line", c), (ArrayKind::Grid, "Grid", c), (ArrayKind::Circle, "Circle", c)];
    if let Some(k) = ui.push_id("array-kind", |ui| segmented(ui, &kinds, a.kind, 28.0)).inner {
        if k != a.kind {
            let mut b = a.clone();
            b.kind = k;
            if k == ArrayKind::Circle {
                b.count = b.count.max(3);
                if b.centre == DVec3::ZERO {
                    b.centre = st.circle_centre(&b.sources);
                }
            }
            st.set_array(b, true);
            st.end_live();
        }
    }
    note(ui, if a.kind == ArrayKind::Circle { "In the view: the square sets how many, the arrow the angle" } else { "In the view: the squares set the steps, the arrows how many" });
    let colors = [AXIS_X, AXIS_Y, AXIS_Z];
    // a vector as X / Y / Z fields under its name
    let vec_row = |ui: &mut Ui, st: &mut AppState, title: &str, v: DVec3, set: &dyn Fn(&mut nurbs_kernel::Array, DVec3)| {
        ui.add_space(2.0);
        ui.label(egui::RichText::new(title).font(regular(12.5)).color(TEXT2()));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for k in 0..3 {
                let mut x = v[k] as f32;
                let r = axis_field(ui, ["X", "Y", "Z"][k], colors[k], &mut x, "");
                if r.changed() {
                    let mut w = v;
                    w[k] = x as f64;
                    let mut b = a.clone();
                    set(&mut b, w);
                    st.set_array(b, r.drag_started() || !st.is_live());
                }
            }
        });
    };
    section(ui, "Copies");
    let mut count = a.count as f32;
    let (ch, start) = slider_row(ui, "Count", &mut count, 1.0..=50.0, 1.0, "");
    if ch {
        st.set_array(nurbs_kernel::Array { count: count.round().max(1.0) as u32, ..a.clone() }, start || !st.is_live());
    }
    match a.kind {
        ArrayKind::Line | ArrayKind::Grid => {
            vec_row(ui, st, "Step, mm", a.step, &|b, w| b.step = w);
            if a.kind == ArrayKind::Grid {
                section(ui, "Rows");
                let mut rows = a.count2 as f32;
                let (ch, start) = slider_row(ui, "Rows", &mut rows, 1.0..=30.0, 1.0, "");
                if ch {
                    st.set_array(nurbs_kernel::Array { count2: rows.round().max(1.0) as u32, ..a.clone() }, start || !st.is_live());
                }
                vec_row(ui, st, "Row step, mm", a.step2, &|b, w| b.step2 = w);
            }
        }
        ArrayKind::Circle => {
            let mut ang = a.angle as f32;
            let (ch, start) = slider_row(ui, "Angle", &mut ang, 1.0..=360.0, 1.0, "°");
            if ch {
                st.set_array(nurbs_kernel::Array { angle: ang as f64, ..a.clone() }, start || !st.is_live());
            }
            note(ui, if a.angle >= 359.999 { "360°: evenly all round" } else { "The first and the last at the ends of the angle" });
            section(ui, "Axis");
            let items: Vec<(u8, &str, Color32)> = vec![(0, "X", AXIS_X), (1, "Y", AXIS_Y), (2, "Z", AXIS_Z)];
            if let Some(x) = ui.push_id("array-axis", |ui| segmented(ui, &items, a.axis, 26.0)).inner {
                if x != a.axis {
                    st.set_array(nurbs_kernel::Array { axis: x, ..a.clone() }, true);
                    st.end_live();
                }
            }
            vec_row(ui, st, "Through, mm", a.centre, &|b, w| b.centre = w);
        }
    }
    section(ui, "Repeats");
    let names: Vec<String> = a.sources.iter().filter_map(|id| st.doc.feature(*id)).map(|g| g.name.clone()).collect();
    let ops = st.doc.features.iter().position(|x| x.id == f.id).map(|i| st.doc.array_pattern(i).iter().filter(|p| !p.is_solid()).count()).unwrap_or(0);
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(names.join(", ")).font(semibold(13.0)).color(TEXT()));
        if ops > 0 {
            chip_label(ui, &format!("+ {ops} operation{}", if ops == 1 { "" } else { "s" }), RAISED(), LABEL());
        }
    });
    note(ui, "An operation on any copy is made on all of them");
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() && st.push.is_none() {
        st.end_live();
    }
}

/// The Poly tab's Bevel (Maya's window): its width, segments, and Chamfer
/// (flat rows) or Bevel (rounded); each change made again from the mesh
/// before it.
fn poly_bevel_card(ui: &mut Ui, st: &mut AppState) {
    let any_down = ui.input(|i| i.pointer.any_down());
    let Some(b) = st.poly_bevel.clone() else { return };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(if b.chamfer { "Chamfer" } else { "Bevel" }).font(semibold(14.0)).color(TEXT()));
        chip_label(ui, &format!("{} edges", b.edges.len()), RAISED(), LABEL());
    });
    ui.add_space(6.0);
    // Chamfer or Bevel (rounded)
    ui.horizontal(|ui| {
        let (well, _) = ui.allocate_exact_size(vec2(2.0 * 104.0 + 6.0, 28.0), Sense::hover());
        ui.painter().rect(well, 14.0, WELL(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
        for (i, (c, name)) in [(true, "CHAMFER"), (false, "BEVEL")].into_iter().enumerate() {
            let r = Rect::from_min_size(well.min + vec2(3.0 + i as f32 * 104.0, 3.0), vec2(104.0, 22.0));
            let resp = ui.interact(r, Id::new(("poly-bevel-kind", i)), Sense::click());
            let on = b.chamfer == c;
            if on {
                ui.painter().rect_filled(r, 11.0, accent());
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 11.0, RAISED());
            }
            ui.painter().text(r.center(), Align2::CENTER_CENTER, name, semibold(10.5), if on { accent_text() } else { LABEL() });
            let tip = if c { "Chamfer: flat rows across each edge" } else { "Bevel: rows on a rounded profile, tangent to both faces" };
            if resp.on_hover_text(tip).clicked() && !on {
                st.set_poly_bevel(b.width, b.segments, c, true);
            }
        }
    });
    ui.add_space(4.0);
    let mut w = b.width;
    let span = (b.width * 2.0).max(5.0).ceil();
    let (ch, start) = slider_row(ui, "Width", &mut w, 0.0..=span.max(100.0), 0.05, "mm");
    if ch {
        st.set_poly_bevel(w, b.segments, b.chamfer, start || !st.is_live());
    }
    let mut segs = b.segments as f32;
    let (ch, start) = slider_row(ui, "Segments", &mut segs, 1.0..=12.0, 1.0, "");
    if ch {
        st.set_poly_bevel(b.width, segs.round().max(1.0) as u32, b.chamfer, start || !st.is_live());
    }
    if b.clamped {
        ui.label(egui::RichText::new(format!("Clamped at {} mm: wider, two rows would cross", fmt_mm(b.made))).font(regular(11.5)).color(LABEL()));
    }
    if b.live {
        ui.label(egui::RichText::new("Move: width · [ ]: segments · C: chamfer / bevel · Click: keep").font(regular(11.5)).color(LABEL()));
    }
    if !any_down && st.is_live() && !b.live && st.poly_drag.is_none() && st.poly_pull.is_none() && st.poly_cut.is_none() {
        st.end_live();
        if let Some(bb) = st.poly_bevel.as_mut() {
            bb.result = st.doc.feature(bb.id).cloned();
        }
    }
}

/// The Slot tool's options (Romain, 20 nov. 5 h 55: by hand by default,
/// reading the scan as an option): BY HAND or ON SCAN.
fn slot_options(ui: &mut Ui, st: &mut AppState) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Slot").font(semibold(14.0)).color(TEXT()));
        chip_label(ui, "Tool", RAISED(), LABEL());
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        // reading the scan: not in the iPad edition
        if st.manual_only {
            return;
        }
        let (well, _) = ui.allocate_exact_size(vec2(2.0 * 84.0 + 6.0, 28.0), Sense::hover());
        ui.painter().rect(well, 14.0, WELL(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
        for (i, (c, name)) in [(false, "BY HAND"), (true, "ON SCAN")].into_iter().enumerate() {
            let r = Rect::from_min_size(well.min + vec2(3.0 + i as f32 * 84.0, 3.0), vec2(84.0, 22.0));
            let resp = ui.interact(r, Id::new(("isl-slot", i)), Sense::click());
            let on = st.slot_on_scan == c;
            if on {
                ui.painter().rect_filled(r, 11.0, accent());
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 11.0, RAISED());
            }
            ui.painter().text(r.center(), Align2::CENTER_CENTER, name, mono(11.0), if on { on_accent() } else { TEXT2() });
            let tip = if c {
                "Drag along a slot of the scan (or a raised pill), end to end: the scan is read there and the slot made as read"
            } else {
                "Drag from one end of the slot to the other on a face or the ground, move to set its width, click, then its height (Add or Carve)"
            };
            let enabled = !c || st.doc.reference.is_some();
            let resp = resp.on_hover_text(if enabled { tip } else { "No scan loaded: drop an .obj on the window first" });
            if resp.clicked() && enabled && st.slot_on_scan != c {
                st.abort_gestures();
                st.slot_on_scan = c;
            }
        }
    });
}

/// The Forms tool's options: the brush, its radius (on screen) and
/// intensity, the symmetry, what a click on the ground makes; the form
/// selected: its poles and More detail.
fn form_options(ui: &mut Ui, st: &mut AppState) {
    use crate::forms::BRUSHES;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Forms").font(semibold(14.0)).color(TEXT()));
        chip_label(ui, "Brush", RAISED(), LABEL());
    });
    ui.add_space(6.0);
    let c = op_color(st.mode);
    let items: Vec<(crate::forms::Brush, &str, Color32)> = BRUSHES.iter().map(|b| (*b, b.label(), c)).collect();
    if let Some(b) = segmented(ui, &items, st.form.brush, 30.0) {
        st.form.brush = b;
    }
    ui.label(egui::RichText::new(st.form.brush.tip()).font(regular(11.5)).color(TEXT2()));
    ui.add_space(4.0);
    let mut r = st.form.radius;
    let _ = slider_row(ui, "Radius", &mut r, 8.0..=600.0, 1.0, "pt");
    st.form.radius = r.clamp(8.0, 600.0);
    let k = st.form.brush.index();
    let mut i = (st.form.intensity[k] * 100.0) as f32;
    let _ = slider_row(ui, "Intensity", &mut i, 1.0..=100.0, 1.0, "%");
    st.form.intensity[k] = (i.clamp(1.0, 100.0) / 100.0) as f64;
    section(ui, "Symmetry");
    if let Some(m) = segmented(ui, &[(false, "Off", c), (true, "Mirror X", c)], st.form.mirror, 28.0) {
        st.form.mirror = m;
    }
    section(ui, "New form (a click on the ground)");
    let kinds = [(nurbs_kernel::FormKind::Sphere, "Sphere", c), (nurbs_kernel::FormKind::Cylinder, "Cylinder", c)];
    if let Some(k) = segmented(ui, &kinds, st.form.new_kind, 28.0) {
        st.form.new_kind = k;
    }
    if let Some(f) = st.single().filter(|f| f.form.is_some()).cloned() {
        let fm = f.form.as_ref().unwrap();
        section(ui, &format!("{} · {} × {} poles", f.name, fm.nu, fm.nv));
        let r = ui.add(egui::Label::new(egui::RichText::new("More detail").font(medium(12.0)).color(LABEL())).sense(Sense::click()));
        if r.on_hover_text("Twice the poles round and up, the same surface: smaller brushes take hold").clicked() {
            st.form_detail(f.id);
        }
        if let Some(e) = st.feature_error(&f) {
            ui.label(egui::RichText::new(e).font(regular(12.0)).color(carve()));
        }
    }
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
        // reading the scan: not in the iPad edition
        if st.manual_only {
            return;
        }
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
    let top = TOP + ISLAND_H + 10.0;
    let inner_w = LEFT_W - 24.0;
    let total = screen.max.y - EDGE - top - st.touch.reserve();
    let rows = st.doc.features.len().max(1) as f32;
    // the groups' headers, and the reference's group (header and row)
    // the same groups as the outliner's (`common::kind_of`), and the
    // operations whose element cannot be told
    let fs = &st.doc.features;
    let mut groups: Vec<bool> = crate::common::Kind::ALL.iter().map(|k| fs.iter().any(|f| crate::common::kind_of(f) == Some(*k))).collect();
    groups.push(fs.iter().any(|f| !f.is_solid() && !f.is_path()));
    // a header is 22 points, a row 28, each with 1 between, a group 3 after
    let extra = groups.iter().filter(|g| **g).count() as f32 * 27.0 + if st.doc.reference.is_some() { 53.0 } else { 0.0 } + if st.doc.groups.is_empty() { 0.0 } else { 25.0 + st.doc.groups.len() as f32 * 29.0 };
    let tree_h = if st.settings.show_tree { (rows * 29.0 + 36.0 + extra).clamp(96.0, (total * 0.36).max(120.0)) } else { 0.0 };
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
                selection_props(ui, st);
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

/// The panel's lower part: what is picked or selected (its position and
/// rotation, a group's transform), else the defaults for new shapes.
fn selection_props(ui: &mut Ui, st: &mut AppState) {
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
                } else if st.selected_group().is_some() {
                    group_properties(ui, st);
                } else if st.single().is_some() {
                    properties(ui, st);
                } else if !st.selection.is_empty() {
                    ui.label(egui::RichText::new(format!("{} shapes selected", st.selection.len())).font(semibold(14.0)).color(TEXT()));
                    ui.label(egui::RichText::new("Move them with the arrows, or delete them (Del)").font(regular(12.0)).color(TEXT2()));
                    if st.can_group() {
                        ui.add_space(4.0);
                        let w = ui.available_width();
                        if secondary_btn(ui, "Group  ·  Ctrl + G", w).on_hover_text("These shapes in one group with its own transform, closed: a click takes them all").clicked() {
                            st.group_selection();
                        }
                    }
                } else {
                    defaults(ui, st);
                }
}

fn tree(ui: &mut Ui, st: &mut AppState, height: f32) {
    let n = st.doc.features.iter().filter(|f| f.is_solid()).count();
    let tab = tablet();
    ui.horizontal(|ui| {
        ui.set_height(if tab { 44.0 } else { 26.0 });
        if tab {
            // the iPad: the sidebar folds from its header (the mock-up)
            if icon_btn(ui, Icon::Sidebar, false, "Fold the scene list", 44.0).clicked() {
                st.panel_visible = false;
            }
            let (lr, _) = ui.allocate_exact_size(vec2(ui.available_width() - 2.0 * 40.0, 44.0), Sense::hover());
            let g = ui.painter().layout_no_wrap("Scene".into(), semibold(17.0), TEXT_STRONG());
            let gw = g.size().x;
            ui.painter().galley(pos2(lr.min.x + 4.0, lr.center().y - g.size().y * 0.5), g, TEXT_STRONG());
            ui.painter().text(pos2(lr.min.x + 12.0 + gw, lr.center().y + 1.0), Align2::LEFT_CENTER, format!("{n} shape{}", if n == 1 { "" } else { "s" }), regular(13.0), TEXT2());
        } else {
            let (lr, _) = ui.allocate_exact_size(vec2(ui.available_width() - 2.0 * 28.0, 26.0), Sense::hover());
            ui.painter().text(lr.left_center() + vec2(2.0, 0.0), Align2::LEFT_CENTER, "SCENE", semibold(10.5), TEXT2());
            ui.painter().text(lr.left_center() + vec2(52.0, 0.0), Align2::LEFT_CENTER, format!("{n} shape{}", if n == 1 { "" } else { "s" }), regular(11.5), pick(Color32::from_rgb(0x64, 0x6B, 0x75), Color32::from_rgb(0x8A, 0x90, 0x99)));
        }
        ui.spacing_mut().item_spacing.x = 2.0;
        // the picked SDF shape among its object's, else the selected feature
        let one = match st.sdf_sel.filter(|_| st.sdf_comp()) {
            Some((id, n)) => Some(crate::common::Item::Shape(id, n)),
            None => st.single().map(|f| crate::common::Item::Feature(f.id)),
        };
        let bs = if tab { 38.0 } else { 26.0 };
        if icon_btn(ui, Icon::Up, false, "Earlier in the history", bs).clicked() {
            if let Some(it) = one {
                st.reorder_item(it, -1);
            }
        }
        if icon_btn(ui, Icon::Down, false, "Later in the history", bs).clicked() {
            if let Some(it) = one {
                st.reorder_item(it, 1);
            }
        }
    });
    // Romain (19 nov.): the elements by kind; an element's operations in
    // the second column, beside the panel
    let feats = st.doc.features.clone();
    // the groups by kind of element (`common::Kind`, every tab's), the
    // operations whose element cannot be told after the surfaces
    use crate::common::{kind_of, Kind};
    let groups: Vec<(&str, Option<Kind>)> = vec![
        (Kind::Primitive.title(), Some(Kind::Primitive)),
        (Kind::Form.title(), Some(Kind::Form)),
        (Kind::Shape.title(), Some(Kind::Shape)),
        (Kind::Curve.title(), Some(Kind::Curve)),
        (Kind::Surface.title(), Some(Kind::Surface)),
        ("OPERATIONS", None),
        (Kind::Sculpt.title(), Some(Kind::Sculpt)),
        (Kind::Poly.title(), Some(Kind::Poly)),
        (Kind::Sdf.title(), Some(Kind::Sdf)),
    ];
    // a group's members' counts: their items (operations, an SDF object's shapes)
    let ops_of = |st: &AppState, id: FeatureId| st.element_items(id).len();
    let focus = st.outline_element();
    let head = if tab { 50.0 } else { 32.0 };
    egui::ScrollArea::vertical().id_salt("tree-scroll").max_height(height - head).min_scrolled_height(height - head).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = if tab { 2.0 } else { 1.0 };
        if feats.is_empty() && st.doc.reference.is_none() {
            ui.label(egui::RichText::new("Empty · pick a shape on the right (B, C), or drop an .obj here").font(regular(12.0)).color(TEXT2()));
        }
        // operations left by shapes deleted before 20 nov. (they fail at
        // every build): one row to take them away
        let dead = st.doc.orphans().len();
        if dead > 0 {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
            let p = ui.painter();
            p.rect(r.shrink2(vec2(1.0, 2.0)), 6.0, if resp.hovered() { RAISED() } else { Color32::TRANSPARENT }, Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
            p.text(pos2(r.min.x + 9.0, r.center().y), Align2::LEFT_CENTER, format!("{dead} leftover operation{}", if dead > 1 { "s" } else { "" }), regular(11.0), carve_light());
            p.text(pos2(r.max.x - 9.0, r.center().y), Align2::RIGHT_CENTER, "Remove", semibold(11.0), if resp.hovered() { TEXT() } else { TEXT2() });
            ui.ctx().data_mut(|d| d.insert_temp(Id::new("orphans-row"), r.center()));
            if resp.on_hover_text("Their shape was deleted: they can no longer be made, and fail at every build. Ctrl + Z brings them back.").clicked() {
                st.remove_orphans();
            }
            ui.add_space(2.0);
        }
        reference_rows(ui, st);
        group_rows(ui, st, &ops_of, focus);
        for (gi, (title, kind)) in groups.iter().enumerate() {
            // the operations whose shape cannot be told; a 2D boolean's
            // curves under it, not on their own
            let members: Vec<&Feature> = match kind {
                None => feats.iter().filter(|f| !f.is_solid() && !f.is_path() && st.owner_of(f.id).is_none()).collect(),
                Some(k) => feats.iter().filter(|f| kind_of(f) == Some(*k) && (*k != Kind::Curve || st.doc.boolean_of(f.id).is_none()) && st.doc.group_of(f.id).is_none()).collect(),
            };
            if members.is_empty() {
                continue;
            }
            let fid = Id::new(("tree-group", gi));
            let folded = ui.data(|d| d.get_temp::<bool>(fid)).unwrap_or(false);
            if group_header(ui, title, members.len(), folded) {
                ui.data_mut(|d| d.insert_temp(fid, !folded));
            }
            if folded {
                continue;
            }
            for f in members {
                // its items: a shape's operations, an SDF object's shapes
                let n = if kind.is_some() { st.element_items(f.id).len() } else { 0 };
                tree_row(ui, st, f, n, focus == Some(f.id));
                if f.shape_bool.is_some() {
                    operand_rows(ui, st, f, 1);
                }
                // the iPad: the element clicked shows its operations under
                // it (the mock-up; no Sub Outliner beside the list)
                if tablet() && focus == Some(f.id) && kind.is_some() {
                    nested_items(ui, st, f.id);
                }
            }
            ui.add_space(3.0);
        }
    });
}

/// A group's header in the outliner (PRIMITIVES, GROUPS…): its arrow, its
/// title and count; true when clicked (folded or unfolded).
fn group_header(ui: &mut Ui, title: &str, n: usize, folded: bool) -> bool {
    let tab = tablet();
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), if tab { 34.0 } else { 22.0 }), Sense::click());
    let p = ui.painter();
    let c = if resp.hovered() { TEXT() } else { TEXT2() };
    icons::paint(p, Rect::from_center_size(pos2(r.min.x + 8.0, r.center().y), if tab { vec2(12.0, 12.0) } else { vec2(10.0, 10.0) }), if folded { Icon::ChevronRight } else { Icon::Chevron }, TEXT2());
    if tab {
        p.text(pos2(r.min.x + 24.0, r.center().y), Align2::LEFT_CENTER, title, mono(11.0), c);
        p.text(pos2(r.max.x - 8.0, r.center().y), Align2::RIGHT_CENTER, n.to_string(), mono(11.0), TEXT2());
    } else {
        p.text(pos2(r.min.x + 20.0, r.center().y), Align2::LEFT_CENTER, title, semibold(10.0), c);
        p.text(pos2(r.max.x - 6.0, r.center().y), Align2::RIGHT_CENTER, n.to_string(), regular(11.0), TEXT2());
    }
    resp.clicked()
}

/// The iPad: an element's items (its operations, an SDF object's shapes)
/// under its row, indented, with the history marker.
fn nested_items(ui: &mut Ui, st: &mut AppState, id: FeatureId) {
    let items = st.element_items(id);
    if items.is_empty() {
        return;
    }
    let marker = st.item_marker(id);
    for (k, it) in items.iter().enumerate() {
        if k == marker {
            ui.horizontal(|ui| {
                ui.add_space(36.0);
                ui.vertical(|ui| history_marker_line(ui));
            });
        }
        ui.horizontal(|ui| {
            ui.add_space(36.0);
            ui.vertical(|ui| item_row(ui, st, *it));
        });
    }
}

/// The outliner's GROUPS (Romain 21 nov.: "sous la même forme que Maya,
/// une petite flèche qui dit que les 3 objets sont dans un groupe"): each
/// group a row with its arrow, its shapes under it, indented. The row
/// selects the group; its padlock opens or closes it.
fn group_rows(ui: &mut Ui, st: &mut AppState, ops_of: &dyn Fn(&AppState, FeatureId) -> usize, focus: Option<FeatureId>) {
    let groups = st.doc.groups.clone();
    if groups.is_empty() {
        return;
    }
    let fid = Id::new("tree-group-groups");
    let folded = ui.data(|d| d.get_temp::<bool>(fid)).unwrap_or(false);
    if group_header(ui, "GROUPS", groups.len(), folded) {
        ui.data_mut(|d| d.insert_temp(fid, !folded));
    }
    if folded {
        return;
    }
    let tab = tablet();
    let selected_group = st.selected_group().map(|g| g.id);
    for g in groups {
        let gid = Id::new(("tree-group-fold", g.id));
        let shut = ui.data(|d| d.get_temp::<bool>(gid)).unwrap_or(false);
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), if tab { 52.0 } else { 28.0 }), Sense::click());
        ui.ctx().data_mut(|d| d.insert_temp(Id::new(("group-row", g.id)), r.center()));
        let selected = selected_group == Some(g.id);
        let lit = !g.open && st.hover_feature.is_some_and(|h| g.members.contains(&h));
        let shown = g.members.iter().any(|m| st.doc.feature(*m).is_some_and(|f| f.visible));
        let dim = if shown { 1.0 } else { 0.4 };
        let p = ui.painter();
        if selected {
            p.rect(r, 6.0, accent_bg(), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
        } else if resp.hovered() || lit {
            p.rect_filled(r, 6.0, RAISED());
        }
        // its arrow (Maya): its shapes shown under it or not
        let ar = Rect::from_center_size(pos2(r.min.x + 8.0, r.center().y), vec2(10.0, 10.0));
        let aresp = ui.interact(ar.expand(5.0), Id::new(("group-arrow", g.id)), Sense::click());
        icons::paint(ui.painter(), ar, if shut { Icon::ChevronRight } else { Icon::Chevron }, if aresp.hovered() { TEXT() } else { TEXT2() });
        let p = ui.painter();
        let bs = if tab { 28.0 } else { 16.0 };
        let x0 = r.min.x + 18.0;
        let br = Rect::from_min_size(pos2(x0, r.center().y - bs * 0.5), vec2(bs, bs));
        p.rect_filled(br, if tab { 8.0 } else { 4.0 }, tint(TEXT(), 0.12 * dim));
        icons::paint(p, br.shrink(if tab { 6.0 } else { 2.0 }), Icon::Group, tint(TEXT(), dim));
        let name_color = if selected { TEXT_STRONG() } else { tint(TEXT(), dim) };
        let n = g.members.len();
        let (fs, fi) = if tab { (15.0, 13.0) } else { (13.0, 12.0) };
        let info_g = p.layout_no_wrap(format!("{n} shape{}", if n > 1 { "s" } else { "" }), regular(fi), tint(TEXT2(), dim));
        let info_x = r.max.x - if tab { 92.0 } else { 62.0 } - info_g.size().x;
        p.galley(pos2(info_x, r.center().y - info_g.size().y * 0.5), info_g, Color32::WHITE);
        let max_w = (info_x - 8.0 - (x0 + bs + 8.0)).max(20.0);
        let mut job = egui::text::LayoutJob::single_section(g.name.clone(), egui::TextFormat::simple(if selected { semibold(fs) } else { regular(fs) }, name_color));
        job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let name_g = ui.painter().layout_job(job);
        ui.painter().galley(pos2(x0 + bs + 8.0, r.center().y - name_g.size().y * 0.5), name_g, name_color);
        // closed or open, then the eye
        let lr = if tab { Rect::from_center_size(pos2(r.max.x - 66.0, r.center().y), vec2(20.0, 20.0)) } else { Rect::from_center_size(pos2(r.max.x - 42.0, r.center().y), vec2(15.0, 15.0)) };
        let lresp = ui.interact(lr.expand(if tab { 10.0 } else { 5.0 }), Id::new(("group-lock", g.id)), Sense::click());
        if g.open {
            ui.painter().rect_filled(lr.expand(3.0), 4.0, accent_bg());
        }
        icons::paint(ui.painter(), lr, if g.open { Icon::Unlock } else { Icon::Lock }, if g.open { accent() } else { tint(TEXT2(), if lresp.hovered() { 1.0 } else { 0.8 }) });
        let lresp = lresp.on_hover_text(if g.open { "Open: its shapes are picked one by one. Click: close it" } else { "Closed: a click takes the whole group. Click: open it" });
        let er = if tab { Rect::from_center_size(pos2(r.max.x - 28.0, r.center().y), vec2(22.0, 22.0)) } else { Rect::from_center_size(pos2(r.max.x - 18.0, r.center().y), vec2(16.0, 16.0)) };
        let eresp = ui.interact(er.expand(if tab { 10.0 } else { 6.0 }), Id::new(("group-eye", g.id)), Sense::click()).on_hover_text("Show / hide its shapes");
        icons::paint(ui.painter(), er, if shown { Icon::Eye } else { Icon::EyeOff }, tint(TEXT2(), 0.8));
        resp.context_menu(|ui| {
            ui.set_min_width(160.0);
            ui.spacing_mut().button_padding = vec2(8.0, 3.0);
            ui.spacing_mut().interact_size.y = 24.0;
            ui.style_mut().override_font_id = Some(regular(12.5));
            if ui.button(if g.open { "Close" } else { "Open" }).clicked() {
                st.set_group_open(g.id, !g.open);
                ui.close();
            }
            if ui.button("Ungroup").clicked() {
                st.ungroup(g.id);
                ui.close();
            }
            if ui.button(if shown { "Hide" } else { "Show" }).clicked() {
                st.toggle_group_visible(g.id);
                ui.close();
            }
            if ui.button("Delete").clicked() {
                st.set_sel_mode(SelMode::Object);
                st.select_group(g.id, false);
                st.delete_selected();
                ui.close();
            }
        });
        if aresp.clicked() {
            ui.data_mut(|d| d.insert_temp(gid, !shut));
        } else if lresp.clicked() {
            st.set_group_open(g.id, !g.open);
        } else if eresp.clicked() {
            st.toggle_group_visible(g.id);
        } else if resp.clicked() {
            let add = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl);
            st.set_sel_mode(SelMode::Object);
            st.select_group(g.id, add);
        }
        if shut {
            continue;
        }
        for m in &g.members {
            let Some(f) = st.doc.feature(*m).cloned() else { continue };
            let n = ops_of(st, f.id);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.vertical(|ui| tree_row(ui, st, &f, n, focus == Some(f.id)));
            });
            if f.shape_bool.is_some() {
                operand_rows(ui, st, &f, 2);
            }
        }
    }
    ui.add_space(3.0);
}

/// A 2D boolean's curves in the outliner, under it, indented.
fn operand_rows(ui: &mut Ui, st: &mut AppState, b: &Feature, depth: usize) {
    let Some(sb) = b.shape_bool.clone() else { return };
    for o in sb.operands {
        let Some(g) = st.doc.feature(o).cloned() else { continue };
        ui.horizontal(|ui| {
            ui.add_space(14.0 * depth as f32);
            ui.vertical(|ui| tree_row(ui, st, &g, 0, false));
        });
        if g.shape_bool.is_some() && depth < 8 {
            operand_rows(ui, st, &g, depth + 1);
        }
    }
}

/// The outliner's REFERENCE group: the mesh to model on (dropped on the
/// window), shown or hidden, solid or seen through (Ghost), removed.
fn reference_rows(ui: &mut Ui, st: &mut AppState) {
    if st.manual_only {
        return;
    }
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
                st.fly_frame(lo, hi);
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
    // the iPad: the mock-up's rows (52 points, a 28-point badge, a switch)
    let tab = tablet();
    let (h, rr, bs) = if tab { (52.0, 12.0, 28.0) } else { (28.0, 6.0, 16.0) };
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    ui.ctx().data_mut(|d| d.insert_temp(Id::new(("row", f.id)), r.center()));
    // a group selected: its row is the one selected (Maya), its shapes'
    // rows only lit
    let in_group = st.selected_group().is_some_and(|g| g.members.contains(&f.id));
    let selected = st.is_selected(f.id) && !in_group;
    let hovered_3d = st.hover_feature == Some(f.id);
    let p = ui.painter();
    if selected {
        p.rect(r, rr, accent_bg(), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
    } else if in_group {
        p.rect_filled(r, rr, RAISED());
    } else if focused {
        p.rect(r, rr, RAISED(), Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
    } else if resp.hovered() || hovered_3d {
        p.rect_filled(r, rr, RAISED());
    }
    let x0 = r.min.x + if tab { 8.0 } else { 6.0 };
    let is_op = !f.is_solid() && !f.is_path();
    let dim = if f.visible && !f.suspended { 1.0 } else { 0.4 };
    let c = feature_color(f);
    let br = Rect::from_min_size(pos2(x0, r.center().y - bs * 0.5), vec2(bs, bs));
    p.rect_filled(br, if tab { 8.0 } else { 4.0 }, tint(c, 0.16 * dim));
    if f.is_solid() {
        paint_op_glyph(p, br.center(), f.op, tint(c, dim), if tab { 13.0 } else { 9.0 });
    } else {
        icons::paint(p, br.shrink(if tab { 6.0 } else { 2.0 }), feature_icon(f), tint(c, dim));
    }
    let name_color = if selected { TEXT_STRONG() } else { tint(TEXT(), dim) };
    let info = if ops > 0 {
        let (one, many) = crate::common::kind_of(f).map(|k| k.items_word()).unwrap_or(("op", "ops"));
        format!("{} · {ops} {}", step_info(f), if ops > 1 { many } else { one })
    } else if f.suspended {
        "past the marker".to_string()
    } else {
        step_info(f)
    };
    let (fs, fi) = if tab { (15.0, 13.0) } else { (13.0, 12.0) };
    let info_g = p.layout_no_wrap(info, regular(fi), tint(TEXT2(), dim));
    let info_x = r.max.x - if tab { 62.0 } else { 40.0 } - info_g.size().x;
    p.galley(pos2(info_x, r.center().y - info_g.size().y * 0.5), info_g, Color32::WHITE);
    let max_w = (info_x - 8.0 - (x0 + bs + 8.0)).max(20.0);
    let mut job = egui::text::LayoutJob::single_section(f.name.clone(), egui::TextFormat::simple(if selected { semibold(fs) } else { regular(fs) }, name_color));
    job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let name_g = ui.painter().layout_job(job);
    ui.painter().galley(pos2(x0 + bs + 8.0, r.center().y - name_g.size().y * 0.5), name_g, name_color);
    let er = if tab { Rect::from_center_size(pos2(r.max.x - 28.0, r.center().y), vec2(if is_op { 42.0 } else { 22.0 }, 22.0)) } else { Rect::from_center_size(pos2(r.max.x - 18.0, r.center().y), vec2(16.0, 16.0)) };
    let eresp = ui.interact(er.expand(if tab { 10.0 } else { 6.0 }), Id::new(("eye", f.id)), Sense::click());
    if is_op && tab {
        switch(ui.painter(), er, f.visible, if f.suspended { 0.45 } else { 1.0 });
    } else if is_op {
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
        if f.is_solid() {
            // (a closed group's shape: the whole group)
            st.click_shape(Some(f.id), add);
        } else {
            st.select(Some(f.id), add);
        }
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
    } else {
        if ui.button(if f.visible { "Hide" } else { "Show" }).clicked() {
            st.toggle_visible(f.id);
            ui.close();
        }
        // every kind of element the same way (`common`): its triangles
        if ui.button("Export .obj").clicked() {
            st.export_elements(&[f.id]);
            ui.close();
        }
    }
    rename_field(ui, st, crate::common::Item::Feature(f.id));
    if ui.button("Delete").clicked() {
        st.select(Some(f.id), false);
        st.delete_selected();
        ui.close();
    }
}

/// A name field in a right-click menu (Enter or leaving it keeps it).
fn rename_field(ui: &mut Ui, st: &mut AppState, it: crate::common::Item) {
    let key = Id::new(("rename", format!("{it:?}")));
    let mut name = ui.data(|d| d.get_temp::<String>(key)).unwrap_or_else(|| st.item_name(it));
    let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(150.0).font(regular(12.5)).hint_text("Name"));
    if r.changed() {
        ui.data_mut(|d| d.insert_temp(key, name.clone()));
    }
    if r.lost_focus() {
        st.rename_item(it, &name);
        ui.data_mut(|d| d.remove::<String>(key));
    }
}

/// An item's icon and colour (`common::Item`): a feature's, or an SDF
/// shape's primitive in its operation's colour.
pub fn item_look(st: &AppState, it: crate::common::Item) -> (Icon, Color32) {
    use crate::common::Item;
    match it {
        Item::Feature(id) => st.doc.feature(id).map(|f| (feature_icon(f), feature_color(f))).unwrap_or((Icon::Frame, TEXT2())),
        Item::Shape(id, n) => {
            let nd = st.doc.feature(id).and_then(crate::sdf::scene_of).and_then(|s| s.node(n));
            let icon = nd.and_then(|x| x.shape()).map(|s| crate::sdf::prim_icon(s.kind)).unwrap_or(Icon::Box);
            let c = match nd.map(|x| x.combine.mode) {
                Some(forme_core::CombineMode::Carve) => carve(),
                Some(forme_core::CombineMode::Add) | None => Workspace::Sdf.swatch(),
                _ => TEXT2(),
            };
            (icon, c)
        }
    }
}

/// One row of an element's items (the Sub Outliner): a feature's row, or
/// an SDF shape's with the same look, switch and menu.
fn item_row(ui: &mut Ui, st: &mut AppState, it: crate::common::Item) {
    use crate::common::Item;
    let (id, n) = match it {
        Item::Feature(id) => {
            if let Some(g) = st.doc.feature(id).cloned() {
                tree_row(ui, st, &g, 0, false);
            }
            return;
        }
        Item::Shape(id, n) => (id, n),
    };
    let tab = tablet();
    let (h, rr, bs) = if tab { (44.0, 10.0, 24.0) } else { (28.0, 6.0, 16.0) };
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let selected = st.item_selected(it);
    let on = st.item_on(it);
    let past = st.item_marker(id) <= st.element_items(id).iter().position(|x| *x == it).unwrap_or(0);
    let p = ui.painter();
    if selected {
        p.rect(r, rr, accent_bg(), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(r, rr, RAISED());
    }
    let dim = if on && !past { 1.0 } else { 0.4 };
    let (icon, c) = item_look(st, it);
    let x0 = r.min.x + 6.0;
    let br = Rect::from_min_size(pos2(x0, r.center().y - bs * 0.5), vec2(bs, bs));
    p.rect_filled(br, if tab { 7.0 } else { 4.0 }, tint(c, 0.16 * dim));
    icons::paint(p, br.shrink(if tab { 5.0 } else { 2.0 }), icon, tint(c, dim));
    let name_color = if selected { TEXT_STRONG() } else { tint(TEXT(), dim) };
    let info = if past { "past the marker".to_string() } else { st.item_info(it).1 };
    let (fs, fi) = if tab { (15.0, 13.0) } else { (13.0, 12.0) };
    let info_g = p.layout_no_wrap(info, regular(fi), tint(TEXT2(), dim));
    let info_x = r.max.x - if tab { 62.0 } else { 40.0 } - info_g.size().x;
    p.galley(pos2(info_x, r.center().y - info_g.size().y * 0.5), info_g, Color32::WHITE);
    let max_w = (info_x - 8.0 - (x0 + bs + 8.0)).max(20.0);
    let mut job = egui::text::LayoutJob::single_section(st.item_name(it), egui::TextFormat::simple(if selected { semibold(fs) } else { regular(fs) }, name_color));
    job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let name_g = ui.painter().layout_job(job);
    ui.painter().galley(pos2(x0 + bs + 8.0, r.center().y - name_g.size().y * 0.5), name_g, name_color);
    // on / off, as an operation's
    let er = if tab { Rect::from_center_size(pos2(r.max.x - 28.0, r.center().y), vec2(42.0, 22.0)) } else { Rect::from_center_size(pos2(r.max.x - 18.0, r.center().y), vec2(16.0, 16.0)) };
    let eresp = ui.interact(er.expand(if tab { 10.0 } else { 6.0 }), Id::new(("item-on", id, n)), Sense::click());
    let b = Rect::from_center_size(er.center(), vec2(13.0, 13.0));
    let p = ui.painter();
    if tab {
        switch(p, er, on, if past { 0.45 } else { 1.0 });
    } else if on {
        p.rect_filled(b, 3.0, tint(accent(), if past { 0.45 } else { 1.0 }));
        p.line_segment([b.min + vec2(3.0, 6.8), b.min + vec2(5.6, 9.4)], Stroke::new(1.6, on_accent()));
        p.line_segment([b.min + vec2(5.6, 9.4), b.min + vec2(10.2, 3.8)], Stroke::new(1.6, on_accent()));
    } else {
        p.rect(b, 3.0, Color32::TRANSPARENT, Stroke::new(1.2, if eresp.hovered() { LABEL() } else { DIVIDER() }), StrokeKind::Inside);
    }
    let eresp = eresp.on_hover_text("On / off: left out of the object, the shapes after it kept");
    resp.context_menu(|ui| item_menu(ui, st, it));
    if eresp.clicked() {
        st.toggle_item(it);
    } else if resp.clicked() {
        st.select_item(it);
    }
}

/// An item's right-click menu: on / off, the marker after it, all back,
/// duplicate (a shape), delete.
fn item_menu(ui: &mut Ui, st: &mut AppState, it: crate::common::Item) {
    use crate::common::Item;
    if let Item::Feature(id) = it {
        if let Some(f) = st.doc.feature(id).cloned() {
            row_menu(ui, st, &f);
        }
        return;
    }
    ui.set_min_width(160.0);
    ui.spacing_mut().button_padding = vec2(8.0, 3.0);
    ui.spacing_mut().interact_size.y = 24.0;
    ui.style_mut().override_font_id = Some(regular(12.5));
    let on = st.item_on(it);
    if ui.button(if on { "Turn off" } else { "Turn on" }).clicked() {
        st.toggle_item(it);
        ui.close();
    }
    if let Some(el) = st.item_element(it) {
        let items = st.element_items(el);
        if ui.button("Back to here").clicked() {
            let n = items.iter().position(|x| *x == it).map(|i| i + 1).unwrap_or(0);
            st.set_item_marker(el, n);
            ui.close();
        }
        if st.item_marker(el) < items.len() && ui.button("All shapes back").clicked() {
            st.set_item_marker(el, items.len());
            ui.close();
        }
    }
    if let Item::Shape(..) = it {
        ui.horizontal(|ui| {
            if ui.button("Earlier").clicked() {
                st.reorder_item(it, -1);
            }
            if ui.button("Later").clicked() {
                st.reorder_item(it, 1);
            }
        });
        rename_field(ui, st, it);
        if ui.button("Duplicate").clicked() {
            st.select_item(it);
            st.sdf_duplicate();
            ui.close();
        }
        if ui.button("Frame").clicked() {
            let pts: Vec<glam::Vec3> = st.item_points(it).iter().map(|p| p.as_vec3()).collect();
            if let Some((lo, hi)) = pts.iter().fold(None, |b: Option<(glam::Vec3, glam::Vec3)>, p| Some(b.map(|(l, h)| (l.min(*p), h.max(*p))).unwrap_or((*p, *p)))) {
                st.fly_frame(lo, hi);
            }
            ui.close();
        }
    }
    if ui.button("Delete").clicked() {
        st.delete_item(it);
        ui.close();
    }
}

/// The Sub Outliner's content: the element's name, then its items in
/// history order (a shape's operations, an SDF object's shapes), the marker
/// (a white line) before those left out.
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
    let ops = st.element_items(id);
    if ops.is_empty() {
        let sdf = st.doc.feature(id).map(crate::sdf::is_sdf).unwrap_or(false);
        let empty = if sdf { "No shapes: add one from the SDF tab (B C O P T)" } else { "No operations yet: its fillets, insets and moves show here" };
        ui.label(egui::RichText::new(empty).font(regular(11.5)).color(TEXT2()));
        if fixed {
            ui.add_space((max_h - 46.0).max(0.0));
        }
        return;
    }
    let marker = st.item_marker(id);
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
            item_row(ui, st, *g);
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
    // the Studio interface (Romain 22 nov.: the mock-up D3 as a theme)
    ui.label(egui::RichText::new("Interface").font(regular(12.5)).color(LABEL()));
    if let Some(v) = segmented(ui, &[(false, "Classic", accent()), (true, "Studio", accent())], st.settings.studio, 28.0) {
        st.settings.studio = v;
        st.save_settings();
    }
    ui.add_space(2.0);
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
    // tab_anim
    if toggle_row(ui, "Tab animation", st.settings.tab_anim) {
        st.settings.tab_anim = !st.settings.tab_anim;
        st.save_settings();
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

/// A Poly mesh's smoothing angle set (one undo step per slider drag).
fn poly_angle_edit(st: &mut AppState, id: FeatureId, a: f32, started: bool) {
    feature_edit(st, id, "Smooth angle", started, |g| {
        if let Some(m) = g.mesh.as_mut() {
            m.make_mut().angle = a.clamp(0.0, 180.0);
        }
    });
    st.mesh_gen += 1;
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

/// The selected group in the panel: its name, closed or open, and its
/// transform properties (Maya: "il y a des propriétés de transform"):
/// position (its pivot), rotation, scale. Changing them moves, turns or
/// scales its shapes.
fn group_properties(ui: &mut Ui, st: &mut AppState) {
    let Some(g) = st.selected_group().cloned() else { return };
    let id = g.id;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.label(egui::RichText::new("Selection").font(regular(12.0)).color(TEXT2()));
        ui.horizontal(|ui| {
            let buf = Id::new("group-rename");
            if st.renaming {
                let mut name = ui.data(|d| d.get_temp::<String>(buf)).unwrap_or_else(|| g.name.clone());
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(120.0).font(semibold(15.0)));
                ui.data_mut(|d| d.insert_temp(buf, name.clone()));
                if r.lost_focus() {
                    if !name.trim().is_empty() {
                        st.rename_group(id, name.trim());
                    }
                    st.renaming = false;
                    ui.data_mut(|d| d.remove::<String>(buf));
                } else if !r.has_focus() {
                    r.request_focus();
                }
            } else {
                let r = ui.add(egui::Label::new(egui::RichText::new(&g.name).font(semibold(15.0)).color(TEXT())).sense(Sense::click()).truncate());
                if r.on_hover_text("Double-click to rename").double_clicked() {
                    st.renaming = true;
                }
            }
            chip_label(ui, "Group", RAISED(), LABEL());
            if g.open {
                chip_label(ui, "Open", accent_bg(), accent());
            } else {
                chip_label(ui, "Closed", RAISED(), LABEL());
            }
        });
    });
    let shown = g.members.iter().any(|m| st.doc.feature(*m).is_some_and(|f| f.visible));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if icon_btn(ui, if g.open { Icon::Unlock } else { Icon::Lock }, g.open, if g.open { "Open: its shapes are picked one by one. Click: close it" } else { "Closed: a click takes the whole group. Click: open it" }, 30.0).clicked() {
            st.set_group_open(id, !g.open);
        }
        if icon_btn(ui, Icon::Duplicate, false, "Duplicate (Ctrl+D): a copy of the group", 30.0).clicked() {
            st.duplicate_selected();
        }
        if icon_btn(ui, Icon::Align, st.tool == Tool::Align, "Align (Alt+A): then click the shape to align to", 30.0).clicked() {
            st.begin_align();
        }
        if icon_btn(ui, if shown { Icon::Eye } else { Icon::EyeOff }, false, "Show / hide its shapes", 30.0).clicked() {
            st.toggle_group_visible(id);
        }
        if icon_btn(ui, Icon::Frame, false, "Frame", 30.0).clicked() {
            st.frame_selection();
        }
        if icon_btn(ui, Icon::Trash, false, "Delete the group and its shapes", 30.0).clicked() {
            st.delete_selected();
        }
    });
    let Some(g) = st.selected_group().cloned() else { return };
    let w = ui.available_width();
    if secondary_btn(ui, "Ungroup  ·  Ctrl + Shift + G", w).on_hover_text("The group goes, its shapes stay where they are").clicked() {
        st.ungroup(id);
        return;
    }
    divider(ui);
    let any_down = ui.input(|i| i.pointer.any_down());
    if fold_section(ui, "Transform", "group-transform-open") {
        let colors = [AXIS_X, AXIS_Y, AXIS_Z];
        let (ex, ey, ez) = g.rot.to_euler(EulerRot::XYZ);
        let mut e = [ex.to_degrees() as f32, ey.to_degrees() as f32, ez.to_degrees() as f32];
        let row = |ui: &mut Ui, title: &str| {
            ui.label(egui::RichText::new(title).font(regular(11.0)).color(TEXT2()));
        };
        row(ui, "Position");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for k in 0..3 {
                let mut v = g.origin[k] as f32;
                let r = axis_field(ui, ["X", "Y", "Z"][k], colors[k], &mut v, "");
                if r.changed() {
                    let mut o = g.origin;
                    o[k] = v as f64;
                    st.set_group_transform(id, o, g.rot, g.scale, "Move group", r.drag_started());
                }
            }
        });
        row(ui, "Rotation");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for k in 0..3 {
                let r = axis_field(ui, ["X", "Y", "Z"][k], colors[k], &mut e[k], "°");
                if r.changed() {
                    let q = DQuat::from_euler(EulerRot::XYZ, (e[0] as f64).to_radians(), (e[1] as f64).to_radians(), (e[2] as f64).to_radians());
                    st.set_group_transform(id, g.origin, q, g.scale, "Rotate group", r.drag_started());
                }
            }
        });
        row(ui, "Scale");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for k in 0..3 {
                let mut v = g.scale[k] as f32;
                let (lr, _) = ui.allocate_exact_size(vec2(18.0, 22.0), Sense::hover());
                ui.painter().text(lr.center(), Align2::CENTER_CENTER, ["X", "Y", "Z"][k], semibold(12.0), colors[k]);
                let r = value_field(ui, &mut v, 0.01..=1000.0, 0.01, "", 66.0);
                if r.changed() {
                    let mut sc = g.scale;
                    sc[k] = (v as f64).max(0.01);
                    st.set_group_transform(id, g.origin, g.rot, sc, "Scale group", r.drag_started());
                }
            }
        });
        ui.label(egui::RichText::new("Position: the group's pivot · D + click in the view puts it on a point").font(regular(11.0)).color(TEXT2()));
        let w = ui.available_width();
        if secondary_btn(ui, "Centre pivot", w).on_hover_text("The pivot back at the middle of its shapes (they do not move)").clicked() {
            st.put_group_pivot(id, None);
        }
    }
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() {
        st.end_live();
    }
    divider(ui);
    let names: Vec<String> = g.members.iter().filter_map(|m| st.doc.feature(*m)).map(|f| f.name.clone()).collect();
    ui.label(egui::RichText::new(format!("{} shape{}: {}", names.len(), if names.len() > 1 { "s" } else { "" }, names.join(", "))).font(regular(11.5)).color(TEXT2()));
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
            if f.is_solid() && !f.is_path() {
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
        if icon_btn(ui, Icon::Align, st.tool == Tool::Align, "Align (Alt+A): then click the shape to align to", 30.0).clicked() {
            st.begin_align();
        }
        if f.is_solid() && icon_btn(ui, Icon::Mirror, false, "Mirror (Alt+M): the part made symmetric across X, the steps after it on both sides", 30.0).clicked() {
            st.add_mirror(0);
        }
        if f.is_solid() && !f.is_path() && icon_btn(ui, Icon::Array, false, "Array (Ctrl+Shift+D): copies in a line, in rows or round an axis, with their operations", 30.0).clicked() {
            st.add_array();
        }
        if f.is_solid() && !f.is_path() && icon_btn(ui, Icon::Shell, false, "Shell (Alt+S): the part hollowed, 2 mm walls (Face mode: the faces picked left open)", 30.0).clicked() {
            st.add_shell();
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

/// A linked surface's window (Romain 20 nov., 18 h 12: "un petit pop-up où
/// j'ai les trois états"): its state, what each one lets you do, what it
/// holds, the Carves that cut it.
fn linked_card(ui: &mut Ui, st: &mut AppState, f: &Feature, p: &nurbs_kernel::patch::Patch) {
    use nurbs_kernel::patch::PatchState;
    let id = f.id;
    section(ui, "State");
    let items = [(PatchState::Nurbs, "NURBS", Workspace::Forme), (PatchState::Poly, "POLY", Workspace::Poly), (PatchState::Sculpt, "SCULPT", Workspace::Sculpt)];
    if let Some(s) = state_buttons(ui, &items, p.state) {
        st.set_patch_state(id, s);
    }
    let off = p.off_surface().iter().filter(|o| **o).count();
    let clay = crate::linked::sculpt_shown(f).is_some();
    let lines: Vec<String> = match p.state {
        PatchState::Poly => vec![
            "The mesh is yours: Vertex, Edge, Face modes, Extrude, Bevel, Cut.".into(),
            "Back to NURBS its points ride on the surface.".into(),
            if p.sculpt.is_some() { "The clay's details come back on the new faces.".into() } else { String::new() },
        ],
        PatchState::Sculpt => vec![
            "The Poly layer as clay: the brushes (X C V S G I T P).".into(),
            "Back to NURBS or Poly the details are kept and ride on the contour and the Poly layer.".into(),
        ],
        PatchState::Nurbs => vec![
            "Move the contour's points: Vertex mode (1), a point or the gizmo.".into(),
            if clay {
                "The clay rides on it (shown in place of the surface).".into()
            } else if off > 0 {
                format!("The Poly layer's own shapes ({off} points) ride on it, shown, not editable here.")
            } else {
                "The Poly layer follows it.".into()
            },
        ],
    };
    for l in lines.into_iter().filter(|l| !l.is_empty()) {
        ui.label(egui::RichText::new(l).font(regular(12.0)).color(TEXT2()));
    }
    let (verts, faces) = match &p.keep {
        Some(k) => (k.verts.len(), k.faces.len()),
        None => f.mesh.as_ref().map(|m| (m.verts.len(), m.faces.len())).unwrap_or((0, 0)),
    };
    if p.count() > 1 {
        ui.label(egui::RichText::new(format!("NURBS: {} patches, {} contour points", p.count(), p.contour().len())).font(regular(12.0)).color(LABEL()));
    }
    if p.state == PatchState::Sculpt {
        if let Some(m) = f.mesh.as_ref() {
            ui.label(egui::RichText::new(format!("Clay: {} points · {} faces", m.verts.len(), m.faces.len())).font(regular(12.0)).color(LABEL()));
        }
    }
    ui.label(egui::RichText::new(format!("Poly layer: {verts} points · {faces} faces")).font(regular(12.0)).color(LABEL()));
    // the Carves after it that cut it
    let at = st.doc.features.iter().position(|g| g.id == id).unwrap_or(0);
    let cuts: Vec<String> = if st.patch_solids(id).is_empty() {
        vec![]
    } else {
        st.doc.features[at + 1..].iter().filter(|g| g.visible && g.op == nurbs_kernel::Op::Carve && g.is_solid() && !g.is_mesh() && !g.is_path()).map(|g| g.name.clone()).collect()
    };
    if !cuts.is_empty() {
        ui.label(egui::RichText::new(format!("Cut by {} (live, both states)", cuts.join(", "))).font(regular(12.0)).color(LABEL()));
    }
}

/// The linked surface's three states, each in its tab's colour and icon:
/// the one it is in filled with it (as the tabs' lit pill), the others
/// their icon and name in it.
fn state_buttons(ui: &mut Ui, items: &[(nurbs_kernel::patch::PatchState, &str, Workspace)], current: nurbs_kernel::patch::PatchState) -> Option<nurbs_kernel::patch::PatchState> {
    let height = 30.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    ui.painter().rect_filled(rect, 11.0, WELL());
    let n = items.len() as f32;
    let inner = rect.shrink(3.0);
    let w = (inner.width() - 3.0 * (n - 1.0)) / n;
    let mut picked = None;
    for (i, (v, label, ws)) in items.iter().enumerate() {
        let r = Rect::from_min_size(inner.min + vec2(i as f32 * (w + 3.0), 0.0), vec2(w, inner.height()));
        let resp = ui.interact(r, ui.id().with(("state", i)), Sense::click());
        let on = *v == current;
        // its tab's colour (darker on the light theme, where the pastels fade)
        let c = match ws {
            Workspace::Forme => pick(ws.swatch(), Color32::from_rgb(0x2A, 0x2D, 0x33)),
            Workspace::Sculpt => pick(ws.swatch(), Color32::from_rgb(0xB8, 0x69, 0x3A)),
            Workspace::Poly => pick(ws.swatch(), Color32::from_rgb(0x2F, 0x86, 0xC0)),
            Workspace::Sdf => pick(ws.swatch(), Color32::from_rgb(0x2E, 0x8B, 0x3E)),
        };
        let p = ui.painter();
        let fg = if on {
            p.rect_filled(r, 8.0, c);
            pick(Color32::from_rgb(0x15, 0x17, 0x1A), Color32::WHITE)
        } else {
            if resp.hovered() {
                p.rect_filled(r, 8.0, RAISED());
            }
            p.rect_stroke(r, 8.0, Stroke::new(1.0, tint(c, 0.35)), StrokeKind::Inside);
            if resp.hovered() { c } else { tint(c, 0.85) }
        };
        let font = semibold(11.5);
        let tw = ui.painter().layout_no_wrap(label.to_string(), font.clone(), fg).size().x;
        let x0 = r.center().x - (13.0 + 5.0 + tw) * 0.5;
        icons::paint(p, Rect::from_min_size(pos2(x0, r.center().y - 6.5), vec2(13.0, 13.0)), ws.icon(), fg);
        p.text(pos2(x0 + 18.0, r.center().y), Align2::LEFT_CENTER, *label, font, fg);
        if resp.on_hover_text(format!("{} state", label)).clicked() && !on {
            picked = Some(*v);
        }
    }
    picked
}

/// The selected shape's sizes (the operation window).
fn shape_card(ui: &mut Ui, st: &mut AppState, f: &Feature) {
    if f.is_extruded_curve() {
        extruded_card(ui, st, f);
        return;
    }
    let id = f.id;
    // a slider drag is one undo step: it ends when no pointer button is down
    let any_down = ui.input(|i| i.pointer.any_down());
    if let Some(p) = f.mesh.as_ref().and_then(|m| m.patch.clone()) {
        linked_card(ui, st, f, &p);
        if p.state != nurbs_kernel::patch::PatchState::Poly {
            return;
        }
    }
    if crate::sdf::is_sdf(f) {
        sdf_card(ui, st, f);
        return;
    }
    if let Some(m) = f.mesh.clone() {
        let sculpt = m.kind == nurbs_kernel::mesh::MeshKind::Sculpt;
        section(ui, if sculpt { "Clay" } else { "Poly mesh" });
        let g = ui.painter().layout_no_wrap(format!("{} points · {} {}", m.verts.len(), m.faces.len(), if sculpt { "triangles" } else { "faces" }), regular(12.0), TEXT2());
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
        ui.painter().galley(pos2(r.min.x, r.center().y - g.size().y * 0.5), g, TEXT2());
        if !sculpt {
            if toggle_row(ui, "Smooth preview", m.smooth) {
                st.selection = vec![id];
                st.poly_toggle_smooth();
            }
            // Maya's normal angle: edges under it shade smooth
            let mut a = m.angle;
            let (ch, start) = slider_row(ui, "Smooth angle", &mut a, 0.0..=180.0, 1.0, "°");
            if ch {
                poly_angle_edit(st, id, a.round(), start);
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for (label, v, tip) in [
                    ("Hard", 0.0, "Harden edges (Maya): every edge hard, the facets shown (angle 0°)"),
                    ("30°", nurbs_kernel::mesh::DEFAULT_ANGLE, "Maya's default normal angle: smooth under 30°, hard over it"),
                    ("Soft", 180.0, "Soften edges (Maya): every edge smooth (angle 180°)"),
                ] {
                    if secondary_btn(ui, label, 64.0).on_hover_text(tip).clicked() && m.angle != v {
                        poly_angle_edit(st, id, v, true);
                        st.end_live();
                    }
                }
            });
            ui.horizontal(|ui| {
                if secondary_btn(ui, "Subdivide", 110.0).clicked() {
                    st.selection = vec![id];
                    st.poly_subdivide();
                }
                if secondary_btn(ui, "Send to Sculpt", 120.0).clicked() {
                    st.selection = vec![id];
                    st.poly_to_sculpt();
                }
            });
        }
        section(ui, "Size");
    } else {
        section(ui, "Shape");
    }
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
    // a plain Cube or Cylinder: made a linked object (NURBS, Poly, Sculpt)
    if st.linkable_solid().map(|g| g.id) == Some(id) && !st.manual_only {
        ui.add_space(4.0);
        if primary_btn(ui, None, "Linked object", Some(ui.available_width()))
            .on_hover_text("This shape as NURBS patches (their contour edited) that are also a Poly mesh and a clay: switch its state in its window (NURBS, POLY, SCULPT). The shape is hidden.")
            .clicked()
        {
            st.make_linked_solid();
        }
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

/// A curve of the Curves set: its settings, as 3ds Max's Parameters
/// rollout (a Rectangle's width, length and corner radius, a Circle's
/// radius, a Polygon's radius, sides and corner radius, a Star's two radii,
/// points and fillets). One undo step per slider drag.
fn curve_card(ui: &mut Ui, st: &mut AppState, f: &Feature) {
    let any_down = ui.input(|i| i.pointer.any_down());
    let id = f.id;
    let (Some(c), Prim::Path { x, z } | Prim::Profile { x, z, .. }) = (f.curve, f.prim) else { return };
    section(ui, "Parameters");
    let size_rows = |ui: &mut Ui, st: &mut AppState, names: [&str; 2]| {
        for (k, label, v) in [(0, names[0], x), (2, names[1], z)] {
            let mut val = v as f32;
            let (ch, start) = slider_row(ui, label, &mut val, 0.01..=2000.0, 0.1, "mm");
            if ch {
                feature_edit(st, id, "Size", start, |g| {
                    if let Prim::Path { x, z } | Prim::Profile { x, z, .. } = &mut g.prim {
                        *(if k == 0 { x } else { z }) = val.max(0.01) as f64;
                    }
                });
            }
        }
    };
    let corner_row = |ui: &mut Ui, st: &mut AppState, label: &str, v: f64, max: f64, inner: bool| {
        let mut val = v as f32;
        let (ch, start) = slider_row(ui, label, &mut val, 0.0..=(max.max(0.0) as f32), 0.1, "mm");
        if ch {
            feature_edit(st, id, "Corner radius", start, |g| {
                if let Some(c) = g.curve.as_mut() {
                    *(if inner { &mut c.corner2 } else { &mut c.corner }) = (val as f64).clamp(0.0, max.max(0.0));
                }
            });
        }
    };
    let radius = c.radius(x, z);
    let radius_row = |ui: &mut Ui, st: &mut AppState, label: &str| {
        let mut val = radius as f32;
        let (ch, start) = slider_row(ui, label, &mut val, 0.01..=1000.0, 0.1, "mm");
        if ch {
            feature_edit(st, id, "Radius", start, |g| g.reshape_curve(c, (val as f64).max(0.01)));
        }
    };
    match c.kind {
        CurveKind::Rectangle => {
            size_rows(ui, st, ["Width", "Length"]);
            corner_row(ui, st, "Corner", c.corner, c.corner_max(x, z, false), false);
        }
        CurveKind::Circle => radius_row(ui, st, "Radius"),
        CurveKind::Ellipse => size_rows(ui, st, ["Width", "Length"]),
        CurveKind::Polygon | CurveKind::Star => {
            let star = c.kind == CurveKind::Star;
            radius_row(ui, st, if star { "Radius 1" } else { "Radius" });
            if star {
                let mut val = (radius * c.ratio()) as f32;
                let (ch, start) = slider_row(ui, "Radius 2", &mut val, 0.01..=(radius as f32 * 0.98), 0.1, "mm");
                if ch {
                    let mut n = c;
                    n.inner = (val as f64 / radius.max(1e-9)).clamp(0.02, 0.98);
                    feature_edit(st, id, "Radius 2", start, |g| g.reshape_curve(n, radius));
                }
            }
            let mut sides = c.n() as f32;
            let (ch, start) = slider_row(ui, if star { "Points" } else { "Sides" }, &mut sides, 3.0..=32.0, 1.0, "");
            if ch {
                let mut n = c;
                n.sides = sides.round().clamp(3.0, 100.0) as u32;
                if n.sides != c.sides {
                    feature_edit(st, id, if star { "Points" } else { "Sides" }, start, |g| g.reshape_curve(n, radius));
                }
            }
            corner_row(ui, st, if star { "Fillet 1" } else { "Corner" }, c.corner, c.corner_max(x, z, false), false);
            if star {
                corner_row(ui, st, "Fillet 2", c.corner2, c.corner_max(x, z, true), true);
            }
        }
    }
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() {
        st.end_live();
    }
    if f.is_path() {
        linked_button(ui, st, id);
    }
}

/// Four sharp corners, closed (a Path, a Rectangle): the button that makes
/// a linked surface on them (NURBS and Poly at once). True when made.
fn linked_button(ui: &mut Ui, st: &mut AppState, id: FeatureId) -> bool {
    if st.single().map(|f| f.id) != Some(id) || st.four_point_path().is_none() || st.manual_only {
        return false;
    }
    ui.add_space(4.0);
    if primary_btn(ui, None, "Linked surface", Some(ui.available_width()))
        .on_hover_text("A NURBS surface on these 4 points, edited by its contour, that is also a Poly mesh and a clay: switch its state in its window (NURBS, POLY, SCULPT)")
        .clicked()
    {
        return st.make_linked_surface();
    }
    false
}

/// A curve or 2D boolean pulled into a solid (Curves, step 3): its curve's
/// settings, then its Amount, Flat again.
fn extruded_card(ui: &mut Ui, st: &mut AppState, f: &Feature) {
    let id = f.id;
    let any_down = ui.input(|i| i.pointer.any_down());
    if f.shape_bool.is_some() {
        bool_card(ui, st, f);
    } else if f.curve.is_some() {
        curve_card(ui, st, f);
    }
    section(ui, "Extrude");
    let mut h = f.prim.height() as f32;
    let (ch, start) = slider_row(ui, "Amount", &mut h, 0.1..=2000.0, 0.1, "mm");
    if ch {
        feature_edit(st, id, "Amount", start, |g| {
            if let Prim::Profile { y, .. } = &mut g.prim {
                *y = h.max(0.1) as f64;
            }
        });
    }
    let r = ui.add(egui::Label::new(egui::RichText::new("Flat again").font(medium(12.0)).color(LABEL())).sense(Sense::click()));
    if r.on_hover_text("Back to a curve (its settings kept)").clicked() {
        st.flatten_curve(id);
    }
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() {
        st.end_live();
    }
}

/// Union, Subtract, Intersect as three tiles (an icon over its name); the
/// one picked.
fn bool_tiles(ui: &mut Ui, current: Option<CurveOp>) -> Option<CurveOp> {
    let items = [(CurveOp::Union, Icon::BoolUnion, "Union joins them"), (CurveOp::Subtract, Icon::BoolSubtract, "Subtract: the first curve less the others"), (CurveOp::Intersect, Icon::BoolIntersect, "Intersect: what they all share")];
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 54.0), Sense::hover());
    let w = (width - 8.0) / 3.0;
    let mut picked = None;
    for (i, (op, icon, tip)) in items.iter().enumerate() {
        let r = Rect::from_min_size(rect.min + vec2(i as f32 * (w + 4.0), 0.0), vec2(w, rect.height()));
        let resp = ui.interact(r, ui.id().with(("bool-tile", i)), Sense::click()).on_hover_text(*tip);
        let on = current == Some(*op);
        let p = ui.painter();
        if on {
            p.rect(r, 9.0, tint(PATH, 0.16), Stroke::new(1.5, PATH), StrokeKind::Inside);
        } else {
            p.rect(r.shrink(0.5), 9.0, if resp.hovered() { RAISED() } else { Color32::TRANSPARENT }, Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
        }
        let c = if on { TEXT_STRONG() } else { LABEL() };
        icons::paint(p, Rect::from_center_size(r.center() - vec2(0.0, 8.0), vec2(22.0, 22.0)), *icon, c);
        p.text(r.center() + vec2(0.0, 15.0), Align2::CENTER_CENTER, op.label(), semibold(11.5), c);
        if resp.clicked() {
            picked = Some(*op);
        }
    }
    picked
}

/// Two closed curves or more picked: made one 2D boolean (step 2 of the
/// Curves set, 3ds Max's Shape Boolean), the first picked as its base.
fn combine_card(ui: &mut Ui, st: &mut AppState) {
    let n = st.selection.len();
    let first = st.selection.first().and_then(|id| st.doc.feature(*id)).map(|f| f.name.clone()).unwrap_or_default();
    ui.label(egui::RichText::new(format!("{n} curves")).font(semibold(14.0)).color(TEXT()));
    let closed = st.selection.iter().filter(|id| st.doc.is_closed_curve(**id)).count();
    if closed == n {
        section(ui, "Combine");
        if let Some(op) = bool_tiles(ui, None) {
            st.make_curve_bool(op);
        }
        ui.label(egui::RichText::new(format!("Base: {first}")).font(regular(12.0)).color(TEXT2()));
    }
    // a loft through them, in the order picked (20 nov.)
    section(ui, "Loft");
    if closed != 0 && closed != n {
        ui.label(egui::RichText::new("A loft's curves are all closed or all open").font(regular(12.0)).color(TEXT2()));
        return;
    }
    let what = if closed == n { "a solid (Add / Carve: the top bar)" } else { "a surface" };
    let r = ui.add_sized(vec2(ui.available_width(), 30.0), egui::Button::new(egui::RichText::new("Loft").font(semibold(13.0)).color(on_accent())).fill(PATH).corner_radius(15.0));
    if r.on_hover_text(format!("A surface through the curves in the order picked: {what}")).clicked() {
        st.make_loft();
    }
    ui.label(egui::RichText::new(format!("Through {n} curves in the order picked: {what}")).font(regular(12.0)).color(TEXT2()));
}

/// The Path tool's options: straight sides or a smooth curve through the
/// points (20 nov.: "créer proprement une courbe").
fn path_options(ui: &mut Ui, st: &mut AppState) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Path").font(semibold(14.0)).color(TEXT()));
        chip_label(ui, "Tool", RAISED(), LABEL());
    });
    ui.add_space(6.0);
    let items = [(false, "LINEAR", PATH), (true, "CURVE", PATH)];
    if let Some(c) = segmented(ui, &items, st.surface_curve, 28.0) {
        st.surface_curve = c;
    }
    let tip = if st.surface_curve { "A smooth curve through the points, exact · Shift + click a point placed: sharp / smooth" } else { "Straight sides · Ctrl + drag a point placed: its fillet" };
    ui.label(egui::RichText::new(tip).font(regular(12.0)).color(TEXT2()));
}

/// What a curve makes (20 nov.): its line straight or the spline through
/// its points; nothing, a flat face (Fill), or a turn round an axis in its
/// plane (Revolve: its angle, the axis along its X or Z, at its box's
/// min side, middle or max side).
fn curve_make_card(ui: &mut Ui, st: &mut AppState, f: &Feature) {
    use crate::surfaces::CurveMake;
    let any_down = ui.input(|i| i.pointer.any_down());
    let id = f.id;
    if f.curve.is_none() && f.shape_bool.is_none() {
        if let Some(pr) = &f.profile {
            section(ui, "Line");
            let items = [(false, "Linear", PATH), (true, "Curve", PATH)];
            if let Some(c) = segmented(ui, &items, pr.spline, 36.0) {
                st.set_spline(id, c);
            }
        }
    }
    section(ui, "Surface");
    let now = if f.revolve.is_some() {
        CurveMake::Revolve
    } else if f.fill {
        CurveMake::Fill
    } else {
        CurveMake::Nothing
    };
    let items = [(CurveMake::Nothing, "None", PATH), (CurveMake::Fill, "Fill", PATH), (CurveMake::Revolve, "Revolve", PATH)];
    if let Some(k) = segmented(ui, &items, now, 36.0) {
        if k != now {
            st.set_curve_make(id, k);
        }
    }
    if let Some(r) = f.revolve {
        let mut a = r.angle as f32;
        let (ch, start) = slider_row(ui, "Angle", &mut a, 1.0..=360.0, 1.0, "°");
        if ch {
            st.set_revolve(id, nurbs_kernel::Revolve { angle: (a as f64).clamp(1.0, 360.0), ..r }, "Revolve angle", start);
        }
        let axes = [(false, "Axis Z", PATH), (true, "Axis X", PATH)];
        if let Some(x) = segmented(ui, &axes, r.along_x, 32.0) {
            st.set_revolve(id, nurbs_kernel::Revolve { along_x: x, ..r }, "Revolve axis", true);
            st.end_live();
        }
        let aligns = [(-1i8, "Min", PATH), (0, "Center", PATH), (1, "Max", PATH)];
        if let Some(al) = segmented(ui, &aligns, r.align, 32.0) {
            st.set_revolve(id, nurbs_kernel::Revolve { align: al, ..r }, "Revolve align", true);
            st.end_live();
        }
        let solid = st.doc.makes(f) == nurbs_kernel::Makes::Solid;
        let what = if solid { format!("A solid · {} (the top bar)", f.op.label()) } else { "A surface: an open curve whose ends are not both on the axis".to_string() };
        ui.label(egui::RichText::new(what).font(regular(12.0)).color(TEXT2()));
    } else if f.fill {
        ui.label(egui::RichText::new("A flat face inside the curve, no thickness").font(regular(12.0)).color(TEXT2()));
    }
    if let Some(e) = st.feature_error(f) {
        ui.label(egui::RichText::new(e).font(regular(12.0)).color(carve()));
    }
    if !any_down && st.is_live() && st.drag.is_none() && st.creation.is_none() {
        st.end_live();
    }
}

/// A loft: its curves in order (a click selects one, ↑ moves it earlier),
/// smooth or ruled, what the kernel said.
fn loft_card(ui: &mut Ui, st: &mut AppState, f: &Feature) {
    let Some(l) = f.loft.clone() else { return };
    let id = f.id;
    section(ui, "Loft");
    let items = [(false, "Smooth", PATH), (true, "Ruled", PATH)];
    if let Some(r) = segmented(ui, &items, l.ruled, 36.0) {
        if r != l.ruled {
            st.set_loft_ruled(id, r);
        }
    }
    let solid = st.doc.makes(f) == nurbs_kernel::Makes::Solid;
    let what = if solid { format!("A solid · {} (the top bar)", f.op.label()) } else { "A surface (open curves)".to_string() };
    ui.label(egui::RichText::new(what).font(regular(12.0)).color(TEXT2()));
    section(ui, "Curves");
    for (k, o) in l.sections.iter().enumerate() {
        let Some(g) = st.doc.feature(*o).cloned() else { continue };
        ui.horizontal(|ui| {
            let r = ui.add(egui::Label::new(egui::RichText::new(format!("{}  {}", k + 1, g.name)).font(medium(13.0)).color(TEXT())).sense(Sense::click()));
            if r.on_hover_text("Select this curve: its settings, its points (Vertex mode)").clicked() {
                st.selection = vec![*o];
            }
            if k > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Label::new(egui::RichText::new("↑").font(medium(14.0)).color(LABEL())).sense(Sense::click())).on_hover_text("Earlier in the loft").clicked() {
                        st.loft_up(id, k);
                    }
                });
            }
        });
    }
    if let Some(e) = st.feature_error(f) {
        ui.label(egui::RichText::new(e).font(regular(12.0)).color(carve()));
    }
}

/// A 2D boolean: its operation, its curves (a click: that curve's own
/// settings; the first is the base), what the kernel said.
fn bool_card(ui: &mut Ui, st: &mut AppState, f: &Feature) {
    let Some(b) = f.shape_bool.clone() else { return };
    let id = f.id;
    section(ui, "Operation");
    if let Some(op) = bool_tiles(ui, Some(b.op)) {
        if op != b.op {
            st.set_curve_bool_op(id, op);
        }
    }
    section(ui, "Curves");
    for (k, o) in b.operands.iter().enumerate() {
        let Some(g) = st.doc.feature(*o).cloned() else { continue };
        ui.horizontal(|ui| {
            let r = ui.add(egui::Label::new(egui::RichText::new(&g.name).font(medium(13.0)).color(TEXT())).sense(Sense::click()));
            if r.on_hover_text("Select this curve: its settings, moved alone").clicked() {
                st.selection = vec![*o];
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Label::new(egui::RichText::new("×").font(medium(14.0)).color(LABEL())).sense(Sense::click())).on_hover_text("Take it out (it stays, on its own)").clicked() {
                    st.take_out_operand(id, *o);
                }
                if k == 0 {
                    chip_label(ui, "base", RAISED(), LABEL());
                } else if b.op == CurveOp::Subtract && ui.add(egui::Label::new(egui::RichText::new("set base").font(regular(11.5)).color(TEXT2())).sense(Sense::click())).on_hover_text("Make it the base: the others are taken from it").clicked() {
                    st.make_base_operand(id, *o);
                }
            });
        });
    }
    if let Some(e) = st.feature_error(f) {
        ui.label(egui::RichText::new(e).font(regular(12.0)).color(carve()));
    }
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
    if linked_button(ui, st, id) {
        return;
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
        let cw = ((ui.available_width() - 16.0) / 5.0).max(44.0);
        for q in [0.5, 1.0, 2.0, 3.0, 5.0] {
            let on = (size - q).abs() < 1e-6;
            let g = ui.painter().layout_no_wrap(fmt_mm(q as f32), medium(12.0), if on { on_accent() } else { TEXT() });
            let (r, resp) = ui.allocate_exact_size(vec2(cw, if tablet() { 36.0 } else { 24.0 }), Sense::click());
            let p = ui.painter();
            p.rect(r, if tablet() { 9.0 } else { 6.0 }, if on { EDGE_SEL } else if resp.hovered() { RAISED() } else { WELL() }, Stroke::new(1.0, DIVIDER()), StrokeKind::Inside);
            p.galley(r.center() - g.size() * 0.5, g, TEXT());
            if resp.on_hover_text(format!("{} {} mm", if chamfer { "Chamfer" } else { "Fillet" }, fmt_mm(q as f32))).clicked() {
                st.set_round(q, chamfer, true);
                st.end_live();
            }
        }
    });
    // the picked edges alone, not those tangent to them (Romain 20 nov.,
    // 10 h 54: "Only Selected")
    ui.add_space(2.0);
    let only = existing.as_ref().map(|r| r.only).unwrap_or(st.round_only);
    if toggle_row(ui, "Only selected", only) {
        st.set_round_only(!only);
    }
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
    if let Some(r) = existing.as_ref().filter(|r| !r.chamfer && !r.only) {
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
    let label = if n > 1 { format!("Shell, these {n} open  Alt+S") } else { "Shell, this face open  Alt+S".to_string() };
    if secondary_btn(ui, &label, ui.available_width()).on_hover_text("The part hollowed (walls of 2 mm), the faces picked left open").clicked() {
        st.add_shell();
    }
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
    // a curve's points (20 nov.): moved by the gizmo in its plane; a
    // spline's made sharp or smooth
    if let Some(pts) = st.picked_path_points() {
        let n = pts.len();
        section(ui, if n > 1 { "Curve points" } else { "Curve point" });
        let spline: Vec<(FeatureId, usize, bool)> = pts.iter().filter_map(|(id, i)| st.doc.feature(*id).and_then(|f| f.profile.as_ref()).filter(|p| p.spline).map(|p| (*id, *i, !p.smooth.get(*i).copied().unwrap_or(true)))).collect();
        if !spline.is_empty() {
            let sharp = spline.iter().all(|x| x.2);
            let items = [(false, "Smooth", PATH), (true, "Sharp", PATH)];
            if let Some(s) = segmented(ui, &items, sharp, 32.0) {
                st.set_points_sharp(&pts, s);
            }
        }
        ui.label(egui::RichText::new("W: the arrows move them in the curve's plane").font(regular(12.0)).color(TEXT2()));
    }
    // a sketch's corner: cut in its outline (exact arc or chamfer), the
    // whole height follows
    if let Some((size, chamfer, max)) = st.profile_corner_state() {
        let n = st.picked_profile_corners().len();
        // (fall through to the part's corners below otherwise)
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
    // curve points only: nothing to round on the part
    if st.picked_path_points().is_some() {
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
pub fn material_ball(p: &egui::Painter, c: egui::Pos2, r: f32, m: &materials::Look) {
    // a see-through ball shows two lines behind it
    let a = 0.25 + 0.75 * m.opacity();
    if m.transparency > 0.01 {
        let st = Stroke::new(1.2, tint(Color32::WHITE, 0.35));
        p.line_segment([c + vec2(-0.9, 0.35) * r, c + vec2(0.9, -0.35) * r], st);
        p.line_segment([c + vec2(-0.6, 0.85) * r, c + vec2(0.95, 0.25) * r], st);
    }
    let steps = 18;
    for k in 0..steps {
        let t = k as f32 / (steps - 1) as f32;
        let lum = 0.45 + 0.75 * t;
        let ch = |x: f32| ((x * lum).clamp(0.0, 1.0) * 255.0) as u8;
        let al = if k == 0 { a } else { (a * 0.5).min(1.0) };
        p.circle_filled(c + vec2(-0.3, -0.34) * r * t, r * (1.0 - 0.7 * t), Color32::from_rgba_unmultiplied(ch(m.color[0]), ch(m.color[1]), ch(m.color[2]), (al * 255.0) as u8));
    }
    // the highlight follows the specular (0.5 as the balls come)
    let k = m.specular * 2.0;
    let spec = ((0.22 + 0.45 * m.metal * (1.0 - m.rough)) * k).min(1.0);
    let size = (0.24 - 0.08 * m.metal) * (1.0 - 0.25 * (k - 1.0).max(0.0));
    p.circle_filled(c + vec2(-0.36, -0.4) * r, r * size, Color32::from_white_alpha((spec * 255.0) as u8));
    p.circle_stroke(c, r, Stroke::new(1.0, tint(Color32::WHITE, 0.22)));
}

/// Three narrow value fields (X Y Z, or rX rY rZ), for the operation
/// window; the values when one changed.
fn xyz_fields(ui: &mut Ui, labels: [&str; 3], vals: [f32; 3], suffix: &str) -> Option<[f32; 3]> {
    let colors = [AXIS_X, AXIS_Y, AXIS_Z];
    let mut v = vals;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let w = ((ui.available_width() - 3.0 * 22.0 - 6.0 * 3.0) / 3.0).clamp(40.0, 66.0);
        for k in 0..3 {
            let (lr, _) = ui.allocate_exact_size(vec2(if labels[k].len() > 1 { 20.0 } else { 14.0 }, 22.0), Sense::hover());
            ui.painter().text(lr.center(), Align2::CENTER_CENTER, labels[k], semibold(11.5), colors[k]);
            if value_field(ui, &mut v[k], -100000.0..=100000.0, if suffix == "°" { 1.0 } else { 0.1 }, suffix, w).changed() {
                changed = true;
            }
        }
    });
    changed.then_some(v)
}

fn axis_field(ui: &mut Ui, label: &str, color: Color32, v: &mut f32, suffix: &str) -> egui::Response {
    let (lr, _) = ui.allocate_exact_size(vec2(18.0, 22.0), Sense::hover());
    ui.painter().text(lr.center(), Align2::CENTER_CENTER, label, semibold(12.0), color);
    // in the operation window, the three fill its width
    value_field(ui, v, -100000.0..=100000.0, if suffix == "°" { 1.0 } else { 0.1 }, suffix, if op_style(ui) { ((OP_W - 74.0) / 3.0).floor() } else { 66.0 })
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
    // Align (3ds Max's Alt + A); its window: Enter OK, Esc Cancel
    if ctx.input(|i| i.modifiers.alt && !i.modifiers.ctrl) && pressed(Key::A) {
        st.begin_align();
        return;
    }
    // Mirror and Shell are NURBS steps: the Forme tab's (Alt + S is the Poly
    // tab's smooth preview)
    let forme = st.workspace == Workspace::Forme;
    // Mirror (Alt + M): across X, the steps after it made on both sides
    if forme && ctx.input(|i| i.modifiers.alt && !i.modifiers.ctrl) && pressed(Key::M) {
        st.add_mirror(0);
        return;
    }
    // Shell (Alt + S): hollowed, the faces picked in Face mode left open
    if forme && ctx.input(|i| i.modifiers.alt && !i.modifiers.ctrl) && pressed(Key::S) {
        st.add_shell();
        return;
    }
    // picking a Shell's faces: Enter ends it (Esc too, below)
    if st.tool == Tool::ShellPick && pressed(Key::Enter) {
        st.select_tool();
        return;
    }
    if st.align.is_some() {
        if pressed(Key::Escape) {
            st.align_cancel();
            return;
        }
        if pressed(Key::Enter) && st.align_open() {
            st.align_ok();
            return;
        }
    }
    let space = st.workspace;
    // the tabs: F1 Forme, F2 Sculpt, F3 Poly, F4 SDF; Ctrl + Tab the next one
    for (k, w) in [(Key::F1, Workspace::Forme), (Key::F2, Workspace::Sculpt), (Key::F3, Workspace::Poly), (Key::F4, Workspace::Sdf)] {
        if pressed(k) {
            st.set_workspace(w);
            return;
        }
    }
    if ctrl && pressed(Key::Tab) {
        let list = st.workspaces();
        let n = list.len();
        let i = list.iter().position(|w| *w == space).unwrap_or(0);
        st.set_workspace(list[(i + if shift { n - 1 } else { 1 }) % n]);
        return;
    }
    let pointer = ctx.input(|i| i.pointer.hover_pos()).map(|p| [p.x, p.y]).unwrap_or(st.principal());
    // a bevel made earlier whose mesh changed since: its window goes
    if st.poly_bevel.is_some() && !st.poly_bevel_valid() {
        st.poly_bevel = None;
    }
    // the Poly tab's Bevel following the pointer: its own keys only
    if st.poly_bevel.as_ref().map(|b| b.live).unwrap_or(false) {
        if pressed(Key::Escape) || (ctrl && pressed(Key::Z)) {
            st.cancel_poly_bevel();
        } else if pressed(Key::Enter) {
            st.finish_poly_bevel();
        } else if pressed(Key::OpenBracket) {
            st.poly_bevel_segments(-1);
        } else if pressed(Key::CloseBracket) {
            st.poly_bevel_segments(1);
        } else if pressed(Key::C) && !ctrl {
            st.poly_bevel_toggle_chamfer();
        } else if pressed(Key::Q) || pressed(Key::W) {
            st.finish_poly_bevel();
            st.set_gizmo(if pressed(Key::Q) { GizmoMode::Select } else { GizmoMode::Move });
        }
        return;
    }
    // the Cut tool: Esc or Enter ends the cut and the tool
    if st.tool == Tool::PolyCut && (pressed(Key::Escape) || pressed(Key::Enter)) {
        st.end_poly_cut();
        st.select_tool();
        return;
    }
    // the Poly tab: Ctrl + E extrudes the picked faces, Ctrl + R inserts an
    // edge loop, Ctrl + B bevels the picked edges
    if ctrl && space == Workspace::Poly {
        if pressed(Key::B) {
            st.finish_poly_pull();
            st.begin_poly_bevel(pointer);
            return;
        }
        if pressed(Key::E) {
            // one under way is kept first
            st.finish_poly_pull();
            st.begin_poly_pull(crate::poly::PullKind::Extrude, pointer);
            return;
        }
        if pressed(Key::R) {
            st.finish_poly_pull();
            st.arm_poly_loop();
            return;
        }
    }
    // Ctrl + I: Isolate (Romain 20 nov., 10 h 57; Ctrl + 1, Maya's, too)
    if ctrl && (pressed(Key::I) || pressed(Key::Num1)) {
        st.toggle_isolate();
        return;
    }
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
            // Ctrl + Shift + D: Pattern / Array (Maya's Duplicate Special)
            if shift {
                st.add_array();
            } else {
                st.duplicate_selected();
            }
        }
        if pressed(Key::S) {
            st.files_open = true;
        }
        // Ctrl + G: group (Maya, 3ds Max); Ctrl + Shift + G: ungroup
        if pressed(Key::G) {
            if shift {
                st.ungroup_selection();
            } else {
                st.group_selection();
            }
        }
        // Ctrl + B and Ctrl + E work on the NURBS part: the Forme tab's
        if pressed(Key::B) && space == Workspace::Forme {
            st.make_bevel();
        }
        // Maya: Ctrl + E extrudes (E is the rotate gizmo)
        if pressed(Key::E) && st.push.is_none() && space == Workspace::Forme {
            let at = ctx.input(|i| i.pointer.hover_pos()).map(|p| [p.x, p.y]);
            // a closed curve selected: pulled into a solid (Curves, step 3)
            if let (Some(id), Some(p)) = (st.pullable(), at) {
                st.begin_curve_pull(id, p);
            } else {
                st.arm_push(PushKind::Extrude, at);
            }
        }
        return;
    }
    // the sketch under way: Enter closes it, Backspace removes its last point
    let enter_closed = st.sketch.is_some() && pressed(Key::Enter);
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
    if pressed(Key::Escape) && st.poly_pull.is_some() {
        st.cancel_poly_pull();
        return;
    }
    if pressed(Key::Escape) && st.poly_comp() && st.tool == Tool::Select && st.poly_sel.as_ref().map(|s| !s.is_empty()).unwrap_or(false) {
        if let Some(s) = st.poly_sel.as_mut() {
            s.verts.clear();
            s.edges.clear();
            s.faces.clear();
        }
        return;
    }
    if pressed(Key::Escape) {
        if st.knife.is_some() {
            st.knife_end();
        } else if st.push.is_some() {
            st.abort_gestures();
            st.select_tool();
        } else if st.creation.is_some() || st.drag.is_some() || st.sketch.is_some() || st.scan_round.is_some() || st.scan_slot.is_some() || st.slot_draw.is_some() || st.curve_pull.is_some() {
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
    // AZERTY (Romain, 20 nov. 6 h 45: "La touche & passe en Object Mode"):
    // the number row's & é " ' without Shift: & Object, é Edge, " Face,
    // ' Vertex (its 1 key would give Vertex through the key's place)
    let typed = |t: &str| ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(x) if x == t)));
    if space != Workspace::Forme {
        tab_keys(ctx, st, space, shift, pointer);
        return;
    }
    let azerty = !ctrl && typed("&");
    if azerty {
        st.set_sel_mode(SelMode::Object);
    } else if !ctrl && typed("'") && !shift {
        st.set_sel_mode(SelMode::Vertex);
    }
    for (k, m) in [(Key::Num1, SelMode::Vertex), (Key::Num2, SelMode::Edge), (Key::Num3, SelMode::Face), (Key::Num4, SelMode::Object), (Key::F8, SelMode::Object), (Key::F9, SelMode::Vertex), (Key::F10, SelMode::Edge), (Key::F11, SelMode::Face)] {
        if pressed(k) && !(azerty && k == Key::Num1) {
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
    if pressed(Key::M) && !shift && !ctx.input(|i| i.modifiers.alt) {
        st.arm_form();
    }
    if st.tool == Tool::Form {
        if pressed(Key::OpenBracket) {
            st.form.radius = (st.form.radius / 1.2).max(8.0);
        }
        if pressed(Key::CloseBracket) {
            st.form.radius = (st.form.radius * 1.2).min(600.0);
        }
    }
    // Enter (Romain 20 nov., 11 h 54): what is under way kept, back to
    // Select (the Knife's cut, an extrusion, a shape's height, a slot's
    // width); an outline is first closed (above), its height set next
    if pressed(Key::Enter) && !enter_closed {
        let under_way = matches!(st.tool, Tool::Knife | Tool::Point)
            || st.push.is_some()
            || st.curve_pull.is_some()
            || st.creation.as_ref().map(|c| c.phase == Phase::Height).unwrap_or(false)
            || st.slot_draw.as_ref().map(|s| s.width).unwrap_or(false);
        if under_way {
            let at = ctx.input(|i| i.pointer.hover_pos()).map(|p| [p.x, p.y]).unwrap_or(st.principal());
            st.commit_gestures(at);
        }
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
        st.frame_current();
    }
    if pressed(Key::Delete) || pressed(Key::Backspace) {
        st.delete_selected();
    }
    let _ = DVec3::ZERO;
}

/// The Sculpt and Poly tabs' keys (Ctrl's are above): what they share
/// with Forme (Q W E R, F, Del, Enter, Alt + X), the modes in Poly, their
/// own tools.
fn tab_keys(ctx: &egui::Context, st: &mut AppState, space: Workspace, shift: bool, pointer: [f32; 2]) {
    use crate::poly::{PolyShape, PullKind};
    use crate::sculpt::Brush;
    use egui::Key;
    let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
    let alt = ctx.input(|i| i.modifiers.alt);
    for (k, g) in [(Key::Q, GizmoMode::Select), (Key::W, GizmoMode::Move), (Key::E, GizmoMode::Rotate), (Key::R, GizmoMode::Scale)] {
        if pressed(k) {
            st.set_gizmo(g);
        }
    }
    if alt && pressed(Key::X) {
        st.xray = !st.xray;
        st.toast(if st.xray { "X-ray on" } else { "X-ray off" }, false);
        return;
    }
    if pressed(Key::F) {
        st.frame_current();
    }
    if pressed(Key::Enter) && (st.poly_pull.is_some() || st.tool == Tool::PolyLoop) {
        st.commit_gestures(pointer);
    }
    if pressed(Key::Delete) || pressed(Key::Backspace) {
        st.delete_selected();
    }
    match space {
        Workspace::Sculpt => {
            for b in Brush::ALL {
                if pressed(b.key()) && !alt {
                    st.set_brush(b);
                }
            }
            if pressed(Key::O) {
                st.add_sculpt(false);
            }
            if pressed(Key::B) {
                st.add_sculpt(true);
            }
            if pressed(Key::M) {
                st.sculpt.mirror = !st.sculpt.mirror;
                st.toast(if st.sculpt.mirror { "Mirror on" } else { "Mirror off" }, false);
            }
            // [ ] the brush's size, Shift: its strength
            let grow = pressed(Key::CloseBracket);
            let shrink = pressed(Key::OpenBracket);
            if grow || shrink {
                if shift {
                    st.sculpt.strength = (st.sculpt.strength + if grow { 0.1 } else { -0.1 }).clamp(0.0, 1.0);
                } else {
                    st.sculpt.radius = (st.sculpt.radius * if grow { 1.2 } else { 1.0 / 1.2 }).clamp(5.0, 300.0);
                }
            }
        }
        Workspace::Poly => {
            for (k, m) in [(Key::Num1, SelMode::Vertex), (Key::Num2, SelMode::Edge), (Key::Num3, SelMode::Face), (Key::Num4, SelMode::Object), (Key::F8, SelMode::Object), (Key::F9, SelMode::Vertex), (Key::F10, SelMode::Edge), (Key::F11, SelMode::Face)] {
                if pressed(k) {
                    st.set_sel_mode(m);
                }
            }
            for (k, shape) in [(Key::B, PolyShape::Cube), (Key::C, PolyShape::Cylinder), (Key::O, PolyShape::Sphere), (Key::P, PolyShape::Plane)] {
                if pressed(k) {
                    st.add_poly(shape);
                }
            }
            if pressed(Key::I) && st.poly_pull.is_none() {
                st.begin_poly_pull(PullKind::Inset, pointer);
            }
            if pressed(Key::K) {
                st.arm_poly_cut();
            }
            if alt && pressed(Key::S) {
                st.poly_toggle_smooth();
            }
        }
        Workspace::Sdf => {
            use crate::sdf::{prim_key, SDF_PRIMS};
            for k in SDF_PRIMS {
                let key = match prim_key(k) {
                    "B" => Key::B,
                    "C" => Key::C,
                    "O" => Key::O,
                    "P" => Key::P,
                    _ => Key::T,
                };
                if pressed(key) && !alt {
                    st.sdf_add(k);
                }
            }
            for (key, m) in [(Key::A, forme_core::CombineMode::Add), (Key::X, forme_core::CombineMode::Carve), (Key::I, forme_core::CombineMode::Intersect)] {
                if pressed(key) && !alt {
                    st.sdf.mode = m;
                    if st.sdf_comp() {
                        st.sdf_edit_node(&format!("Make it {}", m.label()), |n| n.combine.mode = m);
                    }
                }
            }
            // [ ] the picked shape's blend (or the next ones')
            let grow = pressed(Key::CloseBracket);
            let shrink = pressed(Key::OpenBracket);
            if grow || shrink {
                let cur = st.sdf_picked().and_then(|(f, n)| crate::sdf::scene_of(f).and_then(|s| s.node(n)).map(|nd| nd.combine.blend)).unwrap_or(st.sdf.blend);
                let b = if grow { (cur + 0.5).min(30.0) } else { (cur - 0.5).max(0.0) };
                st.sdf.blend = b;
                if st.sdf_comp() {
                    st.sdf_edit_node("Blend", |n| n.combine.blend = b);
                }
                st.toast(&format!("Blend {} mm", fmt_mm(b)), false);
            }
        }
        Workspace::Forme => {}
    }
}
