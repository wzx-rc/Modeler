//! Application state and every editing action (independent of egui and wgpu).

use crate::camera::OrbitCamera;
use crate::files;
use crate::kit::{self, KitPiece};
use forme_core::edges::{EdgeIndex, EdgeKind, EdgePick};
use forme_core::*;
use forme_render::{SelMode, View};
use glam::{Mat3, Mat4, Quat, Vec2, Vec3};
use std::path::PathBuf;
use std::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Place(PrimKind),
    Profile,
    Kit(KitPiece),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    Mouse,
    Finger,
    Pencil,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Gesture {
    #[default]
    None,
    Orbit,
    Pan,
    Place,
    Height,
    Handle,
    ProfileDrag,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacePhase {
    /// Dragging the base (radius / width).
    Base,
    /// Base done, waiting for the height gesture.
    AwaitHeight,
    /// Dragging the height.
    Height,
}

#[derive(Clone, Debug)]
pub struct Placement {
    pub node: NodeId,
    pub kind: PrimKind,
    pub base: Vec3,
    pub normal: Vec3,
    pub rot: Quat,
    pub radius: f32,
    pub hx: f32,
    pub hz: f32,
    pub height: f32,
    pub phase: PlacePhase,
    pub mode: CombineMode,
    pub snapped: Option<&'static str>,
    pub started_on_surface: bool,
    /// Screen anchor and starting height of the height gesture.
    pub anchor: Option<([f32; 2], f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    MoveX,
    MoveY,
    MoveZ,
    /// Plane handles ("équerres"): move inside the plane, `axis` = plane normal.
    PlaneXY,
    PlaneYZ,
    PlaneXZ,
    Radius,
    Height,
    Depth,
    Round,
    /// Fillet of the selected edge (junction blend or shape rounding).
    Fillet,
}

impl Handle {
    pub fn is_plane(self) -> bool {
        matches!(self, Handle::PlaneXY | Handle::PlaneYZ | Handle::PlaneXZ)
    }
    pub fn is_move(self) -> bool {
        matches!(self, Handle::MoveX | Handle::MoveY | Handle::MoveZ) || self.is_plane()
    }
    /// The two in-plane axes of a plane handle.
    pub fn plane_axes(self) -> (Vec3, Vec3) {
        match self {
            Handle::PlaneXY => (Vec3::X, Vec3::Y),
            Handle::PlaneYZ => (Vec3::Y, Vec3::Z),
            _ => (Vec3::X, Vec3::Z),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Handle::MoveX | Handle::MoveY | Handle::MoveZ => "Position",
            Handle::PlaneXY | Handle::PlaneYZ | Handle::PlaneXZ => "Position (plan)",
            Handle::Radius => "Rayon",
            Handle::Height => "Hauteur",
            Handle::Depth => "Profondeur",
            Handle::Round => "Bevel",
            Handle::Fillet => "Fillet",
        }
    }
}

#[derive(Clone, Debug)]
pub struct HandleInfo {
    pub handle: Handle,
    pub world: Vec3,
    pub origin: Vec3,
    pub axis: Vec3,
}

#[derive(Clone, Debug)]
pub struct HandleDrag {
    pub node: NodeId,
    pub handle: Handle,
    pub origin: Vec3,
    pub axis: Vec3,
    pub t0: f32,
    pub start: Node,
    pub scale: f32,
    /// Plane handles: where the drag started on the plane.
    pub start_hit: Vec3,
}

#[derive(Clone, Debug)]
pub struct ProfileDraft {
    pub origin: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub normal: Vec3,
    pub points: Vec<ProfilePoint>,
    pub mode: ProfileMode,
    pub thickness: f32,
    pub round: f32,
    pub symmetric: bool,
}

impl ProfileDraft {
    pub fn to_world(&self, p: Vec2) -> Vec3 {
        self.origin + self.right * p.x + self.up * p.y
    }
    pub fn to_plane(&self, ro: Vec3, rd: Vec3) -> Option<Vec2> {
        let den = rd.dot(self.normal);
        if den.abs() < 1e-6 {
            return None;
        }
        let t = (self.origin - ro).dot(self.normal) / den;
        if t < 0.0 {
            return None;
        }
        let q = ro + rd * t - self.origin;
        Some(Vec2::new(q.dot(self.right), q.dot(self.up)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportRes {
    Draft,
    Normal,
    Fine,
}

impl ExportRes {
    pub fn label(self) -> &'static str {
        match self {
            ExportRes::Draft => "Brouillon",
            ExportRes::Normal => "Normal",
            ExportRes::Fine => "Fin",
        }
    }
    /// Cells along the largest dimension.
    pub fn cells(self) -> f32 {
        match self {
            ExportRes::Draft => 140.0,
            ExportRes::Normal => 260.0,
            ExportRes::Fine => 420.0,
        }
    }
}

pub struct ExportResult {
    pub triangles: usize,
    pub paths: Vec<PathBuf>,
    pub seconds: f32,
}

/// A bake running in the background.
pub struct BakeJob {
    pub progress: std::sync::Arc<forme_core::bake::Progress>,
    pub rx: mpsc::Receiver<Option<forme_core::bake::Baked>>,
    pub started: f64,
}

/// Parametric history → clean mesh. One shot: the result keeps no link with
/// the scene; any edit makes it stale and needs a full new bake.
pub struct BakeState {
    pub open: bool,
    pub quality: forme_core::bake::Quality,
    pub job: Option<BakeJob>,
    pub result: Option<std::sync::Arc<forme_core::bake::Baked>>,
    /// Bumped on every new result (GPU upload key).
    pub version: u64,
    /// Scene version the result was baked from.
    pub scene_version: u64,
    pub show: bool,
    pub wire: bool,
    pub last_export: Option<String>,
    /// Cached constructive plan: (scene version, quality, quad size).
    pub plan: Option<(u64, forme_core::bake::Quality, Option<f64>)>,
}

impl BakeState {
    pub fn stale(&self, scene_version: u64) -> bool {
        self.result.is_some() && self.scene_version != scene_version
    }
}

pub struct Toast {
    pub text: String,
    pub until: f64,
    pub warn: bool,
}

/// Visibility of the points of the hovered / selected edge curves.
#[derive(Default)]
pub struct EdgeVis {
    key: (u64, u64, usize, usize),
    pub hover: Vec<Vec<bool>>,
    pub sel: Vec<Vec<Vec<bool>>>,
}

pub struct AppState {
    pub scene: Scene,
    pub history: History,
    pub selection: Option<NodeId>,
    pub tool: Tool,
    pub mode: CombineMode,
    pub carve_held: bool,
    /// Alt key (desktop) acts like the thumb button.
    pub alt_carve: bool,
    pub camera: OrbitCamera,
    pub doc_name: String,
    pub sym_x: bool,
    pub snap: bool,
    pub grid: bool,
    pub show_tree: bool,
    pub placement: Option<Placement>,
    pub handle_drag: Option<HandleDrag>,
    pub profile: Option<ProfileDraft>,
    pub pointer_kind: PointerKind,
    pub screen: [f32; 2],
    pub panel_visible: bool,
    pub scene_version: u64,
    pub view_version: u64,
    pub interacting: bool,
    pub toast: Option<Toast>,
    pub export_open: bool,
    pub export_res: ExportRes,
    pub export_obj: bool,
    pub export_glb: bool,
    pub export_job: Option<(mpsc::Receiver<Result<ExportResult, String>>, f64)>,
    pub bake: BakeState,
    pub files_open: bool,
    pub kit_open: bool,
    pub material: u32,
    pub now: f64,
    pub last_change: f64,
    pub saved_version: u64,
    pub autosave: bool,
    pub renaming: bool,
    pub gesture: Gesture,
    pub hover: Option<[f32; 2]>,
    /// Edge under the pointer (Select tool) and the selected edge.
    pub edge_hover: Option<EdgePick>,
    pub edge_sel: Vec<EdgePick>,
    /// HUD "CTRL" button held (iPad), or Ctrl / Cmd on a keyboard: taps add
    /// to the selection instead of replacing it.
    pub ctrl_held: bool,
    pub key_ctrl: bool,
    /// Which points of the shown edges are visible (not behind the model).
    pub edge_vis: EdgeVis,
    edge_hover_at: Option<[f32; 2]>,
    edge_index: Option<(u64, std::sync::Arc<EdgeIndex>)>,
    edge_sel_version: u64,
    program: Program,
    program_version: u64,
    sel_program: Program,
    sel_key: (u64, Option<NodeId>),
}

/// Fillet size and kind held by a node for an edge.
pub fn fillet_of(n: &Node, kind: EdgeKind) -> (f32, bool) {
    match kind {
        EdgeKind::Junction { .. } => (n.combine.blend.max(0.0), n.combine.kind == BlendKind::Chamfer),
        EdgeKind::Own { edge, .. } => match n.shape() {
            Some(sh) => {
                let r = sh.edge_radius(edge as usize);
                (r.abs(), r < 0.0)
            }
            None => (0.0, false),
        },
    }
}

/// Writes a fillet (or chamfer) on the node that holds it.
pub fn apply_fillet(n: &mut Node, kind: EdgeKind, r: f32, chamfer: bool) {
    match kind {
        EdgeKind::Junction { .. } => {
            n.combine.blend = r;
            n.combine.kind = if chamfer { BlendKind::Chamfer } else { BlendKind::Smooth };
        }
        EdgeKind::Own { edge, .. } => {
            if let Some(sh) = n.shape_mut() {
                sh.set_edge(edge as usize, if chamfer { -r } else { r });
            }
        }
    }
}

fn snap_to(v: f32, step: f32, on: bool) -> f32 {
    if on {
        (v / step).round() * step
    } else {
        v
    }
}

fn snap_vec(v: Vec3, step: f32, on: bool) -> Vec3 {
    Vec3::new(snap_to(v.x, step, on), snap_to(v.y, step, on), snap_to(v.z, step, on))
}

pub fn ray_plane(ro: Vec3, rd: Vec3, o: Vec3, n: Vec3) -> Option<Vec3> {
    let den = rd.dot(n);
    if den.abs() < 1e-6 {
        return None;
    }
    let t = (o - ro).dot(n) / den;
    (t > 0.0).then(|| ro + rd * t)
}

/// Closest point parameter on the line (o, d) to the ray (ro, rd).
pub fn line_param(o: Vec3, d: Vec3, ro: Vec3, rd: Vec3) -> f32 {
    let w0 = o - ro;
    let b = d.dot(rd);
    let den = 1.0 - b * b;
    if den.abs() < 1e-5 {
        return -(w0.dot(d));
    }
    let d0 = d.dot(w0);
    let e = rd.dot(w0);
    (b * e - d0) / den
}

fn new_version_base() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) << 32
}

impl Default for AppState {
    fn default() -> Self {
        let mut s = Self::new(demo::grenade(), "Grenade sci-fi");
        s.frame_all();
        s
    }
}

impl AppState {
    pub fn new(scene: Scene, name: &str) -> Self {
        AppState {
            scene,
            history: History::new(),
            selection: None,
            tool: Tool::Select,
            mode: CombineMode::Add,
            carve_held: false,
            alt_carve: false,
            camera: OrbitCamera::default(),
            doc_name: name.to_string(),
            sym_x: false,
            snap: true,
            grid: true,
            show_tree: true,
            placement: None,
            handle_drag: None,
            profile: None,
            pointer_kind: PointerKind::Mouse,
            screen: [1194.0, 834.0],
            panel_visible: true,
            // unique per document instance: renderer caches never mistake two documents
            scene_version: new_version_base() + 1,
            view_version: new_version_base() + 1,
            interacting: false,
            toast: None,
            export_open: false,
            export_res: ExportRes::Normal,
            export_obj: true,
            export_glb: true,
            export_job: None,
            bake: BakeState {
                open: false,
                quality: forme_core::bake::Quality::Standard,
                job: None,
                result: None,
                version: 0,
                scene_version: 0,
                show: false,
                wire: true,
                last_export: None,
                plan: None,
            },
            files_open: false,
            kit_open: false,
            material: 0,
            now: 0.0,
            last_change: 0.0,
            saved_version: 1,
            autosave: true,
            renaming: false,
            gesture: Gesture::None,
            hover: None,
            edge_hover: None,
            edge_sel: vec![],
            ctrl_held: false,
            key_ctrl: false,
            edge_vis: EdgeVis::default(),
            edge_hover_at: None,
            edge_index: None,
            edge_sel_version: 0,
            program: Program::default(),
            program_version: 0,
            sel_program: Program::default(),
            sel_key: (0, None),
        }
    }

    // ----------------------------------------------------------------- basics

    pub fn changed(&mut self) {
        self.scene_version += 1;
        self.last_change = self.now;
    }

    pub fn view_changed(&mut self) {
        self.view_version += 1;
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), until: self.now + 2.6, warn: false });
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), until: self.now + 3.5, warn: true });
    }

    pub fn effective_mode(&self) -> CombineMode {
        if self.carve_held || self.alt_carve {
            CombineMode::Carve
        } else {
            self.mode
        }
    }

    pub fn program(&mut self) -> &Program {
        if self.program_version != self.scene_version {
            self.program = compile(&self.scene);
            self.program_version = self.scene_version;
        }
        &self.program
    }

    pub fn selection_program(&mut self) -> (&Program, SelMode) {
        let sel = self.selection.filter(|id| self.scene.node(*id).is_some());
        let key = (self.scene_version, sel);
        if self.sel_key != key {
            self.sel_program = match sel {
                Some(id) => compile_single(&self.scene, id),
                None => Program::default(),
            };
            self.sel_key = key;
        }
        let mode = match sel.and_then(|id| self.scene.node(id)) {
            // an edge is selected: its highlight says it all
            _ if !self.edge_sel.is_empty() => SelMode::None,
            None => SelMode::None,
            Some(n) if n.combine.mode.is_cutter() => SelMode::Ghost,
            Some(n) if n.is_group() => SelMode::Tint,
            Some(_) => SelMode::Tint,
        };
        (&self.sel_program, mode)
    }

    /// Free viewport area (between the rail and the right panel), in points.
    pub fn principal(&self) -> [f32; 2] {
        let left = 84.0;
        let right = if self.panel_visible { self.screen[0] - 308.0 } else { self.screen[0] - 12.0 };
        [(left + right) * 0.5, self.screen[1] * 0.5 + 8.0]
    }

    pub fn view(&self) -> View {
        self.camera.view(self.screen, self.principal())
    }

    pub fn ray(&self, pos: [f32; 2]) -> (Vec3, Vec3) {
        self.view().ray(pos[0], pos[1])
    }

    /// Surface hit (point, normal) under a screen position.
    pub fn hit(&mut self, pos: [f32; 2]) -> Option<(Vec3, Vec3)> {
        let (ro, rd) = self.ray(pos);
        let prog = self.program().clone();
        let t = prog.raycast(ro, rd, 20000.0)?;
        let p = ro + rd * t;
        let n = prog.normal(p, 0.02 + t * 1e-4);
        Some((p, n))
    }

    pub fn select(&mut self, id: Option<NodeId>) {
        self.selection = id;
        self.edge_sel.clear();
        self.renaming = false;
        self.view_changed();
    }

    pub fn tap_select(&mut self, pos: [f32; 2]) {
        // an edge near the tap: junction or shape edge, ready to fillet
        let px = if self.pointer_kind == PointerKind::Finger { 22.0 } else { 12.0 };
        let hovered = if self.pointer_kind == PointerKind::Finger { None } else { self.edge_hover.clone() };
        let add = self.multi();
        if let Some(e) = hovered.or_else(|| self.pick_edge(pos, px)) {
            self.select_edge(e, add);
            return;
        }
        if add {
            // CTRL held: a tap away from the edges keeps the selection
            return;
        }
        match self.hit(pos) {
            Some((p, _)) => {
                let tol = 1.0 + self.view().pixel_size_at((p - self.camera.eye()).length()) * 6.0;
                let id = pick_node(&self.scene, p, tol);
                self.select(id);
            }
            None => self.select(None),
        }
    }

    // ------------------------------------------------------------------ edges

    fn edges(&mut self) -> std::sync::Arc<EdgeIndex> {
        if let Some((v, idx)) = &self.edge_index {
            if *v == self.scene_version {
                return idx.clone();
            }
        }
        let idx = std::sync::Arc::new(EdgeIndex::new(&self.scene));
        self.edge_index = Some((self.scene_version, idx.clone()));
        idx
    }

    /// The edge (junction or shape edge) within `px` points of a screen position.
    pub fn pick_edge(&mut self, pos: [f32; 2], px: f32) -> Option<EdgePick> {
        let view = self.view();
        let p = match self.hit(pos) {
            Some((p, _)) => p,
            None => {
                // silhouette edges: the ray grazes the model, take its closest pass
                let (ro, rd) = self.ray(pos);
                let prog = self.program().clone();
                let mut t = 0.0f32;
                let mut best = (f32::INFINITY, 0.0f32);
                for _ in 0..400 {
                    let d = prog.dist(ro + rd * t);
                    if d < best.0 {
                        best = (d, t);
                    }
                    t += d.max(view.pixel_size_at(t.max(1.0)) * 0.5);
                    if t > 20000.0 {
                        break;
                    }
                }
                let tol = view.pixel_size_at(best.1) * px;
                if best.0 > tol {
                    return None;
                }
                let q = ro + rd * best.1;
                q - prog.normal(q, 0.02 + best.1 * 1e-4) * best.0
            }
        };
        let tol = view.pixel_size_at((p - view.eye).length()) * px;
        let idx = self.edges();
        idx.pick(&self.scene, p, tol)
    }

    /// Hover highlight with the Select tool (pencil hover, trackpad, mouse).
    pub fn update_edge_hover(&mut self) {
        let active = self.tool == Tool::Select
            && self.gesture == Gesture::None
            && self.placement.is_none()
            && self.handle_drag.is_none()
            && self.pointer_kind != PointerKind::Finger
            && !self.bake.show;
        let pos = if active { self.hover } else { None };
        if pos.is_none() {
            self.edge_hover = None;
            self.edge_hover_at = None;
            return;
        }
        if pos == self.edge_hover_at {
            return;
        }
        self.edge_hover_at = pos;
        self.edge_hover = self.pick_edge(pos.unwrap(), 12.0);
    }

    /// CTRL held (HUD button or keyboard): add to the selection.
    pub fn multi(&self) -> bool {
        self.ctrl_held || self.key_ctrl
    }

    /// Selects an edge; with `add`, toggles it in the current edge selection.
    pub fn select_edge(&mut self, e: EdgePick, add: bool) {
        if add {
            if let Some(i) = self.edge_sel.iter().position(|x| x.kind.same(&e.kind)) {
                self.edge_sel.remove(i);
            } else {
                self.edge_sel.push(e);
            }
        } else {
            self.edge_sel = vec![e];
        }
        self.selection = self.edge_sel.last().map(|e| e.owner).or(self.selection);
        self.renaming = false;
        self.edge_sel_version = self.scene_version;
        self.edge_hover = None;
        self.edge_hover_at = None;
        self.view_changed();
    }

    pub fn clear_edge(&mut self) {
        self.edge_sel.clear();
        self.view_changed();
    }

    /// Is this edge in the selection?
    pub fn edge_selected(&self, e: &EdgePick) -> bool {
        self.edge_sel.iter().any(|x| x.kind.same(&e.kind))
    }

    /// Largest fillet that makes sense for the selected edge.
    pub fn fillet_max(&self) -> f32 {
        let one = |e: &EdgePick| -> f32 {
            match e.kind {
                EdgeKind::Own { node, edge } => match self.scene.node(node).and_then(|n| n.shape()) {
                    Some(sh) if sh.kind == PrimKind::Box => {
                        let a = edge as usize / 4;
                        sh.size[(a + 1) % 3].min(sh.size[(a + 2) % 3])
                    }
                    Some(sh) => sh.size.x.min(sh.half_height()),
                    None => 0.0,
                },
                EdgeKind::Junction { .. } => {
                    let (lo, hi) = self.scene.bounds();
                    ((hi - lo).max_element() * 0.25).clamp(1.0, 50.0)
                }
            }
        };
        self.edge_sel.iter().map(one).fold(f32::INFINITY, f32::min).min(1e6).max(0.0)
    }

    /// Sets the fillet of the selected edge (live: one history entry per gesture).
    /// Same fillet on every selected edge (live: one history entry per gesture).
    pub fn set_fillet(&mut self, r: f32, chamfer: bool, start: bool) {
        if self.edge_sel.is_empty() {
            return;
        }
        let r = r.clamp(0.0, self.fillet_max());
        if start {
            self.history.record(if chamfer { "Chanfrein" } else { "Fillet" }, &self.scene);
        }
        for e in self.edge_sel.clone() {
            if let Some(n) = self.scene.node_mut(e.owner) {
                apply_fillet(n, e.kind, r, chamfer);
            }
        }
        self.changed();
    }

    /// Keeps the selected edge in step with the scene (after edits, undo…).
    pub fn refresh_edge(&mut self) {
        if self.edge_sel.is_empty() || self.edge_sel_version == self.scene_version {
            return;
        }
        self.edge_sel_version = self.scene_version;
        let idx = self.edges();
        let old = std::mem::take(&mut self.edge_sel);
        self.edge_sel = old.iter().filter_map(|e| idx.refresh(&self.scene, e)).collect();
        if self.selection.is_none() {
            self.edge_sel.clear();
        }
    }

    /// Which points of the shown edges are in front of the model.
    pub fn update_edge_vis(&mut self) {
        let count = |e: &Option<EdgePick>| e.as_ref().map(|e| e.curves.iter().map(|c| c.len()).sum::<usize>() + (e.at.x.to_bits() as usize & 0xffff)).unwrap_or(0);
        let sel_key: usize = self.edge_sel.iter().map(|e| count(&Some(e.clone()))).sum::<usize>() + self.edge_sel.len() * 7919;
        let key = (self.view_version, self.scene_version, count(&self.edge_hover), sel_key);
        if self.edge_vis.key == key {
            return;
        }
        let prog = self.program().clone();
        let view = self.view();
        let vis = |e: &Option<EdgePick>| -> Vec<Vec<bool>> {
            let Some(e) = e else { return vec![] };
            e.curves
                .iter()
                .map(|c| {
                    c.iter()
                        .map(|p| {
                            let Some(s) = view.project(*p) else { return false };
                            let (ro, rd) = view.ray(s[0], s[1]);
                            let tp = (*p - ro).dot(rd);
                            let eps = view.pixel_size_at(tp.abs()) * 4.0 + 1e-3;
                            match prog.raycast(ro, rd, tp + eps) {
                                Some(t) => t >= tp - eps,
                                None => true,
                            }
                        })
                        .collect()
                })
                .collect()
        };
        self.edge_vis.hover = vis(&self.edge_hover);
        self.edge_vis.sel = self.edge_sel.iter().map(|e| vis(&Some(e.clone()))).collect();
        self.edge_vis.key = key;
    }

    /// New shapes go next to the selection (same group), or at the root.
    /// To work inside a group, select one of its children first.
    fn insertion_parent(&self) -> NodeId {
        match self.selection.and_then(|id| self.scene.node(id)) {
            Some(n) => self.scene.parent_of(n.id).unwrap_or(self.scene.root),
            None => self.scene.root,
        }
    }

    fn parent_frame(&self, parent: NodeId) -> Mat4 {
        if parent == self.scene.root {
            self.scene.node(parent).map(|n| n.transform.matrix()).unwrap_or(Mat4::IDENTITY)
        } else {
            self.scene.world_matrix(parent)
        }
    }

    /// Sets a node's transform from a world position / rotation.
    fn set_world(&mut self, id: NodeId, pos: Vec3, rot: Quat) {
        let parent = self.scene.parent_of(id).unwrap_or(self.scene.root);
        let pm = self.parent_frame(parent);
        let (ps, pr, _) = pm.to_scale_rotation_translation();
        let inv = pm.inverse();
        if let Some(n) = self.scene.node_mut(id) {
            n.transform.pos = inv.transform_point3(pos);
            n.transform.rot = (pr.inverse() * rot).normalize();
            n.transform.scale = 1.0 / ps.x.max(1e-4);
        }
    }

    pub fn undo(&mut self) {
        self.cancel_gestures();
        if let Some(label) = self.history.undo(&mut self.scene) {
            self.validate_selection();
            self.changed();
            self.toast(format!("Annulé : {label}"));
        }
    }

    pub fn redo(&mut self) {
        self.cancel_gestures();
        if let Some(label) = self.history.redo(&mut self.scene) {
            self.validate_selection();
            self.changed();
            self.toast(format!("Rétabli : {label}"));
        }
    }

    fn validate_selection(&mut self) {
        if let Some(id) = self.selection {
            if self.scene.node(id).is_none() {
                self.selection = None;
            }
        }
    }

    pub fn cancel_gestures(&mut self) {
        if let Some(p) = self.placement.take() {
            if p.phase == PlacePhase::Base {
                // the node is removed by restoring the snapshot taken at the start
                self.history.undo(&mut self.scene);
                self.selection = None;
                self.changed();
            }
        }
        self.handle_drag = None;
    }

    // ------------------------------------------------------------- placement

    pub fn begin_placement(&mut self, pos: [f32; 2]) {
        let Tool::Place(kind) = self.tool else { return };
        self.commit_placement();
        let (ro, rd) = self.ray(pos);
        let mut snapped = None;
        let (mut base, normal0, on_surface) = match self.hit(pos) {
            Some((p, n)) => (p, n, true),
            None => {
                // ground plane, else a plane through the target facing the camera
                if rd.y < -1e-3 {
                    let t = (0.0 - ro.y) / rd.y;
                    (ro + rd * t, Vec3::Y, false)
                } else {
                    let n = -self.view().basis().0;
                    let t = (self.camera.target - ro).dot(n) / rd.dot(n);
                    (ro + rd * t.max(0.0), n, false)
                }
            }
        };
        // snap to the centre of a picked round shape (boss, hole...), aligned on its axis
        let mut normal = normal0;
        if on_surface {
            let tol = 1.0 + self.view().pixel_size_at((base - ro).length()) * 6.0;
            if let Some(id) = pick_node(&self.scene, base, tol) {
                let n = &self.scene.nodes[&id];
                let round = n.shape().map(|s| !matches!(s.kind, PrimKind::Box | PrimKind::Profile)).unwrap_or(false);
                if let (Some(sh), Repeat::None, true) = (n.shape(), n.repeat, round) {
                    let wm = self.scene.world_matrix(id);
                    let (sc, _, _) = wm.to_scale_rotation_translation();
                    let c = wm.transform_point3(Vec3::ZERO);
                    let mut axis = wm.transform_vector3(Vec3::Y).normalize_or(Vec3::Y);
                    if axis.dot(normal) < 0.0 {
                        axis = -axis;
                    }
                    let lateral = (base - c) - axis * (base - c).dot(axis);
                    // anywhere inside a hole (walls, bottom) counts; near the centre on a boss
                    let reach = if n.combine.mode == CombineMode::Carve { 1.05 } else { 0.45 };
                    if lateral.length() < sh.radius() * sc.x * reach {
                        if n.combine.mode == CombineMode::Carve {
                            // inside a hole: centre on its mouth
                            base = c + axis * (sh.half_height() * sc.x - 0.4);
                            snapped = Some("centre du perçage");
                        } else {
                            base -= lateral;
                            snapped = Some("centre de la forme");
                        }
                        normal = axis;
                    }
                }
            }
        } else if self.snap {
            base = snap_vec(base, 1.0, true);
            snapped = Some("grille");
        }
        let rot = Quat::from_rotation_arc(Vec3::Y, normal.normalize());
        let mode = self.effective_mode();
        let parent = self.insertion_parent();
        let count = self.scene.nodes.values().filter(|n| n.shape().map(|s| s.kind == kind).unwrap_or(false)).count() + 1;
        let mut shape = Shape::new(kind);
        shape.size = Vec3::splat(0.2);
        let mut spec = NodeSpec::shape(&format!("{} {}", kind.label(), count), shape).mode(mode).mat(self.material);
        if self.sym_x {
            spec.mirror.x = true;
        }
        self.history.record(&format!("Ajouter {}", kind.label()), &self.scene);
        let id = self.scene.add(parent, spec);
        self.set_world(id, base, rot);
        let px = self.view().pixel_size_at((base - ro).length());
        self.placement = Some(Placement {
            node: id,
            kind,
            base,
            normal,
            rot,
            radius: px * 2.0,
            hx: px * 2.0,
            hz: px * 2.0,
            height: px * 4.0,
            phase: PlacePhase::Base,
            mode,
            snapped,
            started_on_surface: on_surface,
            anchor: None,
        });
        self.selection = Some(id);
        self.apply_placement();
        self.changed();
    }

    fn apply_placement(&mut self) {
        let Some(p) = self.placement.clone() else { return };
        let r = p.radius.max(0.2);
        let hh = (p.height * 0.5).max(0.2);
        let size = match p.kind {
            PrimKind::Box => Vec3::new(p.hx.max(0.2), hh, p.hz.max(0.2)),
            PrimKind::Sphere => Vec3::splat(r),
            PrimKind::Capsule => Vec3::new(r, (hh - r).max(0.0), 0.0),
            PrimKind::Torus => Vec3::new(r, 0.0, (p.height * 0.5).clamp(0.2, r * 0.9)),
            PrimKind::Cone => Vec3::new(r, hh, r * 0.35),
            PrimKind::Tube => Vec3::new(r, hh, r * 0.7),
            _ => Vec3::new(r, hh, 0.0),
        };
        // Add grows outwards, cutters go into the matter (top slightly proud)
        let along = match p.kind {
            PrimKind::Sphere => 0.0,
            PrimKind::Torus => size.z * 0.5,
            _ => hh,
        };
        let embed = (along * 0.15).min(0.6);
        let center = match p.mode {
            // grows outwards, slightly embedded so it fuses
            CombineMode::Add => p.base + p.normal * (along - embed),
            // cutters go into the matter, top slightly proud of the surface
            CombineMode::Carve | CombineMode::Intersect => p.base - p.normal * (along - 0.4),
            // line operators work where their surface crosses the matter
            CombineMode::Groove | CombineMode::Tongue => p.base,
        };
        if let Some(n) = self.scene.node_mut(p.node) {
            if let Some(s) = n.shape_mut() {
                s.size = size;
                if p.kind == PrimKind::Box || p.kind == PrimKind::Cylinder {
                    s.round = s.round.min(size.min_element() * 0.5);
                }
            }
        }
        self.set_world(p.node, center, p.rot);
        self.changed();
    }

    pub fn drag_placement(&mut self, pos: [f32; 2]) {
        let Some(mut p) = self.placement.clone() else { return };
        let (ro, rd) = self.ray(pos);
        match p.phase {
            PlacePhase::Base => {
                let den = rd.dot(p.normal);
                if den.abs() < 1e-5 {
                    return;
                }
                let t = (p.base - ro).dot(p.normal) / den;
                let q = ro + rd * t - p.base;
                let tx = p.rot * Vec3::X;
                let tz = p.rot * Vec3::Z;
                p.radius = snap_to(q.length(), 0.5, self.snap).max(0.5);
                p.hx = snap_to(q.dot(tx).abs(), 0.5, self.snap).max(0.5);
                p.hz = snap_to(q.dot(tz).abs(), 0.5, self.snap).max(0.5);
                if p.kind == PrimKind::Box {
                    p.radius = p.hx.max(p.hz);
                    p.height = p.hx.min(p.hz).max(1.0);
                } else {
                    p.height = (p.radius * 1.0).max(1.0);
                }
                if p.kind == PrimKind::Torus {
                    p.height = p.radius * 0.4;
                }
            }
            PlacePhase::Height => {
                // vertical screen drag, one pixel = one pixel of surface at the base
                let (a, h0) = p.anchor.unwrap_or((pos, p.height));
                let px = self.view().pixel_size_at((p.base - ro).length());
                let h = h0 + (a[1] - pos[1]) * px;
                p.height = snap_to(h.max(0.5), 0.5, self.snap).max(0.5);
                let _ = rd;
            }
            PlacePhase::AwaitHeight => {}
        }
        self.placement = Some(p);
        self.apply_placement();
    }

    /// Pointer released. Returns true when the shape is finished.
    pub fn release_placement(&mut self) -> bool {
        let Some(mut p) = self.placement.clone() else { return true };
        match p.phase {
            PlacePhase::Base => {
                if p.kind == PrimKind::Sphere {
                    self.placement = None;
                    self.finish_placement_toast(&p);
                    return true;
                }
                p.phase = PlacePhase::AwaitHeight;
                self.placement = Some(p);
                false
            }
            PlacePhase::Height | PlacePhase::AwaitHeight => {
                self.placement = None;
                self.finish_placement_toast(&p);
                true
            }
        }
    }

    /// Tap with a placement tool: a shape of default size (40 px) ready for its height.
    pub fn tap_place(&mut self, pos: [f32; 2]) {
        self.begin_placement(pos);
        if let Some(mut p) = self.placement.clone() {
            let px = self.view().pixel_size_at((p.base - self.camera.eye()).length());
            p.radius = snap_to(px * 40.0, 0.5, self.snap).max(1.0);
            p.hx = p.radius;
            p.hz = p.radius;
            p.height = p.radius * 2.0;
            if p.kind == PrimKind::Torus {
                p.height = p.radius * 0.4;
            }
            self.placement = Some(p);
            self.apply_placement();
            self.release_placement();
        }
    }

    pub fn begin_height(&mut self, pos: [f32; 2]) {
        if let Some(p) = self.placement.as_mut() {
            p.phase = PlacePhase::Height;
            p.anchor = Some((pos, p.height));
        }
    }

    pub fn commit_placement(&mut self) {
        if let Some(p) = self.placement.take() {
            self.finish_placement_toast(&p);
        }
    }

    fn finish_placement_toast(&mut self, p: &Placement) {
        let name = self.scene.node(p.node).map(|n| n.name.clone()).unwrap_or_default();
        self.toast(format!("{name} · {}", p.mode.label()));
        self.changed();
    }

    // --------------------------------------------------------------- handles

    pub fn handles(&self) -> Vec<HandleInfo> {
        let Some(id) = self.selection else { return vec![] };
        if let Some(e) = self.edge_sel.last() {
            let view = self.view();
            let len = view.pixel_size_at((e.at - view.eye).length()) * 64.0;
            let n = if e.normal.length_squared() > 0.5 { e.normal } else { Vec3::Y };
            return vec![HandleInfo { handle: Handle::Fillet, world: e.at + n * len, origin: e.at, axis: n }];
        }
        let Some(n) = self.scene.node(id) else { return vec![] };
        let m = self.scene.world_matrix(id);
        let (s, r, o) = m.to_scale_rotation_translation();
        let s = s.x;
        let x = r * Vec3::X;
        let y = r * Vec3::Y;
        let z = r * Vec3::Z;
        let view = self.view();
        let len = view.pixel_size_at((o - view.eye).length()) * 110.0;
        let mut out = vec![
            HandleInfo { handle: Handle::MoveX, world: o + Vec3::X * len, origin: o, axis: Vec3::X },
            HandleInfo { handle: Handle::MoveY, world: o + Vec3::Y * len, origin: o, axis: Vec3::Y },
            HandleInfo { handle: Handle::MoveZ, world: o + Vec3::Z * len, origin: o, axis: Vec3::Z },
            HandleInfo { handle: Handle::PlaneXY, world: o + (Vec3::X + Vec3::Y) * len * 0.3, origin: o, axis: Vec3::Z },
            HandleInfo { handle: Handle::PlaneYZ, world: o + (Vec3::Y + Vec3::Z) * len * 0.3, origin: o, axis: Vec3::X },
            HandleInfo { handle: Handle::PlaneXZ, world: o + (Vec3::X + Vec3::Z) * len * 0.3, origin: o, axis: Vec3::Y },
        ];
        if let Some(sh) = n.shape() {
            let sz = sh.size;
            match sh.kind {
                PrimKind::Profile => {
                    out.push(HandleInfo { handle: Handle::Depth, world: o + z * sz.z * s, origin: o, axis: z });
                }
                PrimKind::Torus => {
                    out.push(HandleInfo { handle: Handle::Radius, world: o + x * sz.x * s, origin: o, axis: x });
                    out.push(HandleInfo {
                        handle: Handle::Height,
                        world: o + x * sz.x * s + y * sz.z * s,
                        origin: o + x * sz.x * s,
                        axis: y,
                    });
                }
                _ => {
                    let hh = sh.half_height();
                    out.push(HandleInfo { handle: Handle::Radius, world: o + x * sz.x * s, origin: o, axis: x });
                    out.push(HandleInfo { handle: Handle::Height, world: o + y * hh * s, origin: o, axis: y });
                    if sh.kind == PrimKind::Box {
                        out.push(HandleInfo { handle: Handle::Depth, world: o + z * sz.z * s, origin: o, axis: z });
                    }
                    if matches!(sh.kind, PrimKind::Box | PrimKind::Cylinder | PrimKind::Prism | PrimKind::Tube | PrimKind::Cone) {
                        let d = (x * sz.x + y * hh).normalize_or(x);
                        let reach = (sz.x * sz.x + hh * hh).sqrt() * s;
                        out.push(HandleInfo {
                            handle: Handle::Round,
                            world: o + d * (reach - sh.round * s * 1.2).max(reach * 0.5),
                            origin: o,
                            axis: d,
                        });
                    }
                }
            }
        }
        out
    }

    pub fn begin_handle(&mut self, info: &HandleInfo, pos: [f32; 2]) {
        let Some(id) = self.selection else { return };
        let Some(start) = self.scene.node(id).cloned() else { return };
        let (ro, rd) = self.ray(pos);
        let t0 = line_param(info.origin, info.axis, ro, rd);
        let start_hit = ray_plane(ro, rd, info.origin, info.axis).unwrap_or(info.origin);
        let s = self.scene.world_matrix(id).to_scale_rotation_translation().0.x;
        self.history.record(info.handle.label(), &self.scene);
        self.handle_drag =
            Some(HandleDrag { node: id, handle: info.handle, origin: info.origin, axis: info.axis, t0, start, scale: s, start_hit });
    }

    pub fn drag_handle(&mut self, pos: [f32; 2]) {
        let Some(d) = self.handle_drag.clone() else { return };
        let (ro, rd) = self.ray(pos);
        let t = line_param(d.origin, d.axis, ro, rd);
        let snap = self.snap;
        match d.handle {
            Handle::PlaneXY | Handle::PlaneYZ | Handle::PlaneXZ => {
                let Some(hit) = ray_plane(ro, rd, d.origin, d.axis) else { return };
                let start_world = self.scene.parent_matrix(d.node) * d.start.transform.matrix();
                let (_, rot, o) = start_world.to_scale_rotation_translation();
                let (a1, a2) = d.handle.plane_axes();
                let mut np = o + (hit - d.start_hit);
                if snap {
                    for a in [a1, a2] {
                        let i = a.abs().max_position();
                        np[i] = snap_to(np[i], 0.5, true);
                    }
                }
                self.set_world(d.node, np, rot);
                if let Some(n) = self.scene.node_mut(d.node) {
                    n.transform.scale = d.start.transform.scale;
                }
            }
            Handle::Fillet => {
                // every selected edge takes the handle's value
                let Some(e) = self.edge_sel.last().cloned() else { return };
                let (r0, chamfer) = fillet_of(&d.start, e.kind);
                let r = snap_to((r0 + (t - d.t0) * 0.6).max(0.0), 0.1, snap).clamp(0.0, self.fillet_max());
                for e in self.edge_sel.clone() {
                    if let Some(n) = self.scene.node_mut(e.owner) {
                        apply_fillet(n, e.kind, r, chamfer);
                    }
                }
            }
            Handle::MoveX | Handle::MoveY | Handle::MoveZ => {
                let start_world = self.scene.parent_matrix(d.node) * d.start.transform.matrix();
                let (_, rot, o) = start_world.to_scale_rotation_translation();
                let mut np = o + d.axis * (t - d.t0);
                if snap {
                    let a = d.axis.abs().max_position();
                    np[a] = snap_to(np[a], 0.5, true);
                }
                self.set_world(d.node, np, rot);
                if let Some(n) = self.scene.node_mut(d.node) {
                    n.transform.scale = d.start.transform.scale;
                }
            }
            _ => {
                let Some(sh0) = d.start.shape().cloned() else { return };
                let Some(n) = self.scene.node_mut(d.node) else { return };
                let Some(sh) = n.shape_mut() else { return };
                let v = |x: f32| snap_to((x / d.scale).max(0.2), 0.5, snap).max(0.2);
                match (d.handle, sh0.kind) {
                    (Handle::Radius, PrimKind::Sphere) => {
                        let r = v(t);
                        sh.size.x = r;
                        sh.size.z = r * sh0.size.z / sh0.size.x.max(1e-3);
                    }
                    (Handle::Radius, _) => sh.size.x = v(t),
                    (Handle::Height, PrimKind::Torus) => sh.size.z = v(t).min(sh.size.x * 0.95),
                    (Handle::Height, PrimKind::Capsule) => sh.size.y = (v(t) - sh.size.x).max(0.0),
                    (Handle::Height, _) => sh.size.y = v(t),
                    (Handle::Depth, _) => sh.size.z = v(t),
                    (Handle::Round, _) => {
                        let delta = (d.t0 - t) / d.scale;
                        let maxr = sh0.size.x.min(sh0.half_height());
                        sh.round = snap_to((sh0.round + delta * 0.8).clamp(0.0, maxr), 0.1, snap);
                        sh.edges.clear();
                    }
                    _ => {}
                }
            }
        }
        self.changed();
    }

    pub fn end_handle(&mut self) {
        self.handle_drag = None;
    }

    // --------------------------------------------------------------- profile

    pub fn ensure_profile(&mut self) -> &mut ProfileDraft {
        if self.profile.is_none() {
            let view = self.view();
            let (fwd, _, cam_up) = view.basis();
            let a = fwd.abs().max_position();
            let mut normal = Vec3::ZERO;
            normal[a] = -fwd[a].signum();
            let mut up = cam_up - normal * cam_up.dot(normal);
            let ua = up.abs().max_position();
            let s = up[ua].signum();
            up = Vec3::ZERO;
            up[ua] = s;
            let right = up.cross(normal);
            let origin = match self.selection.and_then(|id| self.scene.node(id)) {
                Some(n) => self.scene.world_matrix(n.id).transform_point3(Vec3::ZERO),
                None => self.camera.target,
            };
            // plane through the origin, snapped to the grid
            let origin = snap_vec(origin, 1.0, self.snap);
            self.profile = Some(ProfileDraft {
                origin,
                right,
                up,
                normal,
                points: vec![],
                mode: ProfileMode::Extrude,
                thickness: 3.0,
                round: 0.6,
                symmetric: true,
            });
        }
        self.profile.as_mut().unwrap()
    }

    /// Adds a point (corner on tap, curve point on drag). Closes on the first point.
    pub fn profile_point(&mut self, pos: [f32; 2], smooth: bool) {
        let (ro, rd) = self.ray(pos);
        let view = self.view();
        let snap = self.snap;
        let draft = self.ensure_profile();
        let Some(q) = draft.to_plane(ro, rd) else { return };
        if draft.points.len() >= 3 {
            let first = draft.to_world(draft.points[0].pos);
            if let Some(sp) = view.project(first) {
                if (sp[0] - pos[0]).hypot(sp[1] - pos[1]) < 22.0 {
                    self.close_profile();
                    return;
                }
            }
        }
        let q = Vec2::new(snap_to(q.x, 0.5, snap), snap_to(q.y, 0.5, snap));
        draft.points.push(ProfilePoint { pos: q, smooth });
        self.view_changed();
    }

    pub fn profile_undo_point(&mut self) {
        if let Some(d) = self.profile.as_mut() {
            d.points.pop();
        }
        self.view_changed();
    }

    pub fn cancel_profile(&mut self) {
        self.profile = None;
        self.view_changed();
    }

    pub fn close_profile(&mut self) {
        let Some(d) = self.profile.take() else { return };
        if d.points.len() < 3 {
            self.warn("Il faut au moins 3 points pour fermer le contour");
            self.profile = Some(d);
            return;
        }
        let mut shape = Shape::new(PrimKind::Profile);
        shape.size = Vec3::new(0.0, 0.0, d.thickness);
        shape.round = d.round.min(d.thickness * 0.9);
        shape.profile = Some(Profile { points: d.points.clone(), mode: d.mode });
        let mode = self.effective_mode();
        let parent = self.insertion_parent();
        let count = self.scene.nodes.values().filter(|n| n.shape().map(|s| s.kind == PrimKind::Profile).unwrap_or(false)).count() + 1;
        self.history.record("Profil", &self.scene);
        let spec = NodeSpec::shape(&format!("Profil {count}"), shape).mode(mode).mat(self.material);
        let id = self.scene.add(parent, spec);
        let rot = Quat::from_mat3(&Mat3::from_cols(d.right, d.up, d.normal)).normalize();
        let origin = if d.symmetric { d.origin } else { d.origin + d.normal * d.thickness };
        self.set_world(id, origin, rot);
        self.selection = Some(id);
        self.tool = Tool::Select;
        self.changed();
        self.toast(format!("Profil {} extrudé · {} points", count, d.points.len()));
    }

    // -------------------------------------------------------------------- kit

    pub fn place_kit(&mut self, piece: KitPiece, pos: [f32; 2]) {
        let Some((p, n)) = self.hit(pos) else {
            self.warn("Pose le kit sur une surface");
            return;
        };
        let rot = Quat::from_rotation_arc(Vec3::Y, n.normalize());
        let parent = self.insertion_parent();
        self.history.record(piece.label(), &self.scene);
        let pm = self.parent_frame(parent);
        let (ps, pr, _) = pm.to_scale_rotation_translation();
        let local = pm.inverse().transform_point3(p);
        let id = kit::insert(&mut self.scene, parent, piece, local, pr.inverse() * rot, 1.0 / ps.x.max(1e-4));
        self.selection = Some(id);
        self.changed();
        self.toast(format!("{} posé", piece.label()));
    }

    // -------------------------------------------------------- tree / commands

    pub fn edit<F: FnOnce(&mut Node)>(&mut self, label: &str, id: NodeId, f: F) {
        let before = self.scene.clone();
        if let Some(n) = self.scene.node_mut(id) {
            f(n);
        }
        if self.scene != before {
            self.history.record(label, &before);
            self.changed();
        }
    }

    /// For continuous edits (sliders): one history entry per gesture.
    pub fn edit_live<F: FnOnce(&mut Node)>(&mut self, label: &str, id: NodeId, start: bool, f: F) {
        if start {
            self.history.record(label, &self.scene);
        }
        if let Some(n) = self.scene.node_mut(id) {
            f(n);
        }
        self.changed();
    }

    /// Mode bar: sets the mode of the next shapes; with the Select tool it also
    /// changes the selected shape.
    pub fn set_mode(&mut self, m: CombineMode) {
        self.mode = m;
        if self.tool != Tool::Select {
            return;
        }
        if let Some(id) = self.selection {
            if self.scene.node(id).map(|n| n.shape().is_some() || matches!(n.kind, NodeKind::Group { combined: true, .. })).unwrap_or(false) {
                self.edit("Mode", id, |n| n.combine.mode = m);
            }
        }
    }

    pub fn delete_selected(&mut self) {
        if let Some(id) = self.selection {
            self.history.record("Supprimer", &self.scene);
            self.scene.remove(id);
            self.selection = None;
            self.changed();
        }
    }

    pub fn duplicate_selected(&mut self) {
        if let Some(id) = self.selection {
            self.history.record("Dupliquer", &self.scene);
            if let Some(c) = self.scene.duplicate(id) {
                if let Some(n) = self.scene.node_mut(c) {
                    n.name = format!("{} copie", n.name);
                    n.transform.pos += Vec3::new(8.0, 0.0, 0.0);
                }
                self.selection = Some(c);
            }
            self.changed();
        }
    }

    pub fn toggle_visible(&mut self, id: NodeId) {
        self.edit("Visibilité", id, |n| n.visible = !n.visible);
    }

    pub fn move_in_parent(&mut self, id: NodeId, delta: i32) {
        let Some(parent) = self.scene.parent_of(id) else { return };
        let idx = self.scene.node(parent).and_then(|p| p.children().iter().position(|c| *c == id)).unwrap_or(0) as i32;
        let len = self.scene.node(parent).map(|p| p.children().len()).unwrap_or(0) as i32;
        let ni = (idx + delta).clamp(0, len - 1);
        if ni != idx {
            self.history.record("Réordonner", &self.scene);
            self.scene.reparent(id, parent, ni as usize);
            self.changed();
        }
    }

    pub fn group_selected(&mut self) {
        let Some(id) = self.selection else { return };
        let Some(parent) = self.scene.parent_of(id) else { return };
        let idx = self.scene.node(parent).and_then(|p| p.children().iter().position(|c| *c == id)).unwrap_or(0);
        self.history.record("Grouper", &self.scene);
        let g = self.scene.insert(parent, NodeSpec::group("Groupe", false), Some(idx));
        self.scene.reparent(id, g, 0);
        if let Some(n) = self.scene.node_mut(g) {
            n.expanded = true;
        }
        self.selection = Some(g);
        self.changed();
    }

    pub fn toggle_repeat(&mut self) {
        let Some(id) = self.selection else {
            self.warn("Sélectionne une forme à répéter");
            return;
        };
        let cur = self.scene.node(id).map(|n| n.repeat).unwrap_or_default();
        let next = match cur {
            Repeat::None => Repeat::Radial { count: 4, axis: Axis::Y },
            _ => Repeat::None,
        };
        self.edit("Répétition", id, |n| n.repeat = next);
        self.toast(match next {
            Repeat::None => "Répétition retirée".to_string(),
            _ => "Répétition radiale ×4 autour de Y".to_string(),
        });
    }

    pub fn frame_all(&mut self) {
        let (lo, hi) = self.scene.bounds();
        self.camera.frame(lo, hi);
        self.view_changed();
    }

    pub fn frame_selection(&mut self) {
        let Some(id) = self.selection else { return self.frame_all() };
        let bb = match self.scene.node(id).map(|n| n.is_group()) {
            Some(false) => self.scene.shape_world_aabb(id),
            _ => {
                let o = self.scene.world_matrix(id).transform_point3(Vec3::ZERO);
                Some((o - Vec3::splat(20.0), o + Vec3::splat(20.0)))
            }
        };
        if let Some((lo, hi)) = bb {
            self.camera.frame(lo, hi);
            self.view_changed();
        }
    }

    // ------------------------------------------------------------- documents

    pub fn new_document(&mut self, scene: Scene, name: &str) {
        self.scene = scene;
        self.doc_name = name.to_string();
        self.history = History::new();
        self.selection = None;
        self.placement = None;
        self.profile = None;
        self.handle_drag = None;
        self.frame_all();
        self.changed();
        self.saved_version = self.scene_version;
    }

    pub fn save(&mut self) {
        match files::save(&self.scene, &self.doc_name) {
            Ok(p) => {
                self.saved_version = self.scene_version;
                self.toast(format!("Enregistré : {}", p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()));
            }
            Err(e) => self.warn(format!("Échec de l'enregistrement : {e}")),
        }
    }

    pub fn open(&mut self, path: &std::path::Path) {
        match files::load(path) {
            Ok(s) => {
                let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                self.new_document(s, &name);
                self.toast(format!("Ouvert : {name}"));
            }
            Err(e) => self.warn(format!("Impossible d'ouvrir : {e}")),
        }
    }

    pub fn start_export(&mut self) {
        if self.export_job.is_some() {
            return;
        }
        let prog = compile(&self.scene);
        if prog.is_empty() {
            self.warn("Rien à exporter");
            return;
        }
        let (lo, hi) = self.scene.bounds();
        let cell = ((hi - lo).max_element() / self.export_res.cells()).max(0.05);
        let materials = self.scene.materials.clone();
        let name = files::sanitize(&self.doc_name);
        let (obj, glb) = (self.export_obj, self.export_glb);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            let mesh = mesh::dual_contour(&prog, lo, hi, cell);
            let dir = files::documents_dir();
            let mut paths = vec![];
            let mut res = Ok(());
            if glb {
                let p = dir.join(format!("{name}.glb"));
                res = res.and(std::fs::write(&p, export::to_glb(&mesh, &materials, &name)));
                paths.push(p);
            }
            if obj {
                let p = dir.join(format!("{name}.obj"));
                res = res.and(std::fs::write(&p, export::to_obj(&mesh, &materials, &name)));
                paths.push(p);
            }
            let out = res
                .map(|_| ExportResult { triangles: mesh.triangle_count(), paths, seconds: t0.elapsed().as_secs_f32() })
                .map_err(|e| e.to_string());
            let _ = tx.send(out);
        });
        self.export_job = Some((rx, self.now));
    }

    /// Quad size of the bake par opération for the current scene, or None
    /// when the history needs the classic bake (cached per scene version).
    pub fn bake_plan(&mut self) -> Option<f64> {
        let q = self.bake.quality;
        if let Some((v, pq, p)) = self.bake.plan {
            if v == self.scene_version && pq == q {
                return p;
            }
        }
        let p = forme_core::bake::build::plan(&self.scene, &forme_core::bake::BakeSettings::preset(q));
        self.bake.plan = Some((self.scene_version, q, p));
        p
    }

    pub fn start_bake(&mut self) {
        if self.bake.job.is_some() {
            return;
        }
        if compile(&self.scene).is_empty() {
            self.warn("Rien à baker");
            return;
        }
        let scene = self.scene.clone();
        let settings = forme_core::bake::BakeSettings::preset(self.bake.quality);
        let progress = std::sync::Arc::new(forme_core::bake::Progress::default());
        let p2 = progress.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(forme_core::bake::bake(&scene, &settings, &p2));
        });
        self.bake.job = Some(BakeJob { progress, rx, started: self.now });
        self.bake.scene_version = self.scene_version;
        self.bake.last_export = None;
    }

    pub fn cancel_bake(&mut self) {
        if let Some(j) = &self.bake.job {
            j.progress.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Writes the baked mesh (OBJ with quads and n-gons, GLB triangulated).
    pub fn export_bake(&mut self, obj: bool, glb: bool) {
        let Some(b) = self.bake.result.clone() else { return };
        let name = format!("{} bake", files::sanitize(&self.doc_name));
        let dir = files::documents_dir();
        let mut done = vec![];
        let mut err = None;
        if obj {
            let p = dir.join(format!("{name}.obj"));
            match std::fs::write(&p, b.to_obj(&self.scene.materials, &name)) {
                Ok(_) => done.push(format!("{name}.obj")),
                Err(e) => err = Some(e.to_string()),
            }
        }
        if glb {
            let p = dir.join(format!("{name}.glb"));
            match std::fs::write(&p, export::to_glb(&b.to_mesh(), &self.scene.materials, &name)) {
                Ok(_) => done.push(format!("{name}.glb")),
                Err(e) => err = Some(e.to_string()),
            }
        }
        match err {
            Some(e) => self.warn(format!("Export impossible : {e}")),
            None => {
                let msg = format!("Exporté : {}", done.join(", "));
                self.bake.last_export = Some(msg.clone());
                self.toast(msg);
            }
        }
    }

    /// Called every frame.
    pub fn tick(&mut self, now: f64) {
        self.now = now;
        let mut finished = None;
        if let Some(j) = &self.bake.job {
            if let Ok(r) = j.rx.try_recv() {
                finished = Some((r, now - j.started));
            }
        }
        if let Some((r, _)) = finished {
            self.bake.job = None;
            match r {
                Some(b) => {
                    let secs = if b.stats.seconds < 1.0 { format!("{:.0} ms", b.stats.seconds * 1000.0) } else { format!("{:.1} s", b.stats.seconds) };
                    let quads = if b.stats.quads == b.faces.len() { " · 100 % quads" } else { "" };
                    let msg = format!("Bake terminé · {} faces{quads} · {secs}", fmt_int(b.faces.len()));
                    self.bake.result = Some(std::sync::Arc::new(b));
                    self.bake.version += 1;
                    self.bake.show = true;
                    self.toast(msg);
                }
                None => self.toast("Bake annulé".to_string()),
            }
        }
        if self.bake.show && self.bake.stale(self.scene_version) {
            // the baked mesh no longer matches the model: back to the live view
            self.bake.show = false;
        }
        if let Some((rx, _)) = &self.export_job {
            if let Ok(res) = rx.try_recv() {
                self.export_job = None;
                match res {
                    Ok(r) => {
                        let names: Vec<String> =
                            r.paths.iter().filter_map(|p| p.file_name().map(|f| f.to_string_lossy().to_string())).collect();
                        self.toast(format!(
                            "Exporté : {} · {} triangles · {:.1} s",
                            names.join(", "),
                            fmt_int(r.triangles),
                            r.seconds
                        ));
                        self.export_open = false;
                    }
                    Err(e) => self.warn(format!("Export impossible : {e}")),
                }
            }
        }
        if let Some(t) = &self.toast {
            if now > t.until {
                self.toast = None;
            }
        }
        if self.autosave
            && self.scene_version != self.saved_version
            && now - self.last_change > 2.5
            && self.placement.is_none()
            && self.handle_drag.is_none()
        {
            let _ = files::save(&self.scene, "Autosauvegarde");
            self.saved_version = self.scene_version;
        }
    }

    /// One-line hint for the HUD.
    pub fn hint(&self) -> String {
        if self.bake.show {
            if let Some(b) = &self.bake.result {
                return format!("Maillage baké · {} faces · {} quads · aperçu sans lien avec le modèle", fmt_int(b.faces.len()), fmt_int(b.stats.quads));
            }
        }
        if let Some(p) = &self.placement {
            return match p.phase {
                PlacePhase::Base => format!("{} · {} — étire la base", p.kind.label(), p.mode.label()),
                PlacePhase::AwaitHeight => format!("{} · {} — glisse pour la hauteur, tap pour valider", p.kind.label(), p.mode.label()),
                PlacePhase::Height => format!("{} · {} — relâche pour valider", p.kind.label(), p.mode.label()),
            };
        }
        match self.tool {
            Tool::Select => match self.selection.and_then(|id| self.scene.node(id)) {
                Some(n) => {
                    let mut s = format!("{} · {}", n.name, n.type_label());
                    if n.shape().is_some() {
                        s += &format!(" · {}", n.combine.mode.label());
                    }
                    match n.repeat {
                        Repeat::Radial { count, .. } => s += &format!(" · Radiale ×{count}"),
                        Repeat::Linear { count, .. } => s += &format!(" · Linéaire ×{count}"),
                        Repeat::None => {}
                    }
                    s
                }
                None => format!("{} formes · touche une forme pour la sélectionner", self.scene.shape_count()),
            },
            Tool::Place(k) => format!("{} · {} — pose au Pencil sur une surface", k.label(), self.effective_mode().label()),
            Tool::Profile => "Profil · tap = angle · glisser = courbe".to_string(),
            Tool::Kit(k) => format!("Kit · {} — touche une surface", k.label()),
        }
    }
}

pub fn fmt_int(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('\u{202F}');
        }
        out.push(c);
    }
    out
}

pub fn fmt_mm(v: f32) -> String {
    format!("{:.1}", v).replace('.', ",")
}
