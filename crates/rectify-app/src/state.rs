//! The app's state: the mesh being put right, the hand edits made on it
//! (undo / redo), the tool in hand, the view.

use crate::camera::OrbitCamera;
use crate::view::View;
use glam::{DVec3, Vec3};
use rayon::prelude::*;
use rectify_core::circle::Circle;
use rectify_core::line::{self, LineFit, LineParams};
use rectify_core::sym::{self, Axis, Found, SymMode};
use rectify_core::{Mesh, Moves, Surface, Topo};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub light: bool,
    pub grid: bool,
    /// Zebra stripes: shows how true the surfaces are.
    pub zebra: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { light: false, grid: true, zebra: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// No tool: the left button orbits.
    Navigate,
    /// Drag a segment along an edge: straight and sharp there.
    Line,
    /// The mirror plane: both sides made mirror images.
    Mirror,
    /// Drag across a round part: its wall made truly round, ring by ring.
    Circle,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Navigate => "Navigate",
            Tool::Line => "Line",
            Tool::Mirror => "Mirror",
            Tool::Circle => "Circle",
        }
    }
}

/// The mesh open: as it came, as it is now, and what does not change.
pub struct Doc {
    pub original: Mesh,
    pub mesh: Mesh,
    pub topo: Topo,
    /// Median edge length (mm).
    pub edge: f64,
}

/// One hand edit, in the history.
pub struct Step {
    pub tool: Tool,
    pub what: String,
    pub moves: Moves,
}

/// A segment being dragged with the Line tool.
pub struct LineDrag {
    pub a: DVec3,
    pub b: DVec3,
    /// The vertices of the triangles under both ends.
    pub seeds: Vec<u32>,
    pub seeds_b: Vec<u32>,
    /// The edge it would fix (computed again as the pointer moves).
    pub fit: Option<Result<LineFit, String>>,
    pub fitted_at: Option<DVec3>,
}

/// A Line placed and not applied yet: shown on the mesh at the strength
/// set, which can change live (as can the width and the hold: fitted again).
pub struct LineEdit {
    pub a: DVec3,
    pub b: DVec3,
    pub seeds: Vec<u32>,
    pub fit: LineFit,
}

/// The round part found by the Circle tool, and its settings (applied with
/// Enter).
pub struct CircleEdit {
    pub circle: Circle,
    pub height: f64,
    pub levels: usize,
    pub reach: f64,
    /// (height, radius) of each ring, read on the mesh.
    pub rings: Vec<(f64, f64)>,
    /// How far the wall is from those rings now (mm, root mean square).
    pub off: f64,
}

/// A Circle drag: from the press on the rim to the pointer, on the plane
/// under the press.
#[derive(Clone)]
pub struct CircleDrag {
    pub a: DVec3,
    pub b: DVec3,
    pub seeds: Vec<u32>,
    /// The plane's normal at the press.
    pub n: DVec3,
}

/// The Mirror tool's settings and the plane found.
pub struct SymState {
    pub axis: Axis,
    pub mode: SymMode,
    /// How far a vertex's image may be from the surface (mm).
    pub reach: f64,
    pub found: Option<Found>,
}

pub enum Msg {
    Loaded(Result<Box<Doc>, String>, PathBuf),
    Exported(Result<PathBuf, String>),
    /// An edit computed in the background: its tool, what it did, the moves.
    Edit(Tool, String, Moves, String),
}

pub struct Job {
    pub rx: Receiver<Msg>,
    pub what: String,
}

pub struct Toast {
    pub text: String,
    pub warn: bool,
    pub until: f64,
}

pub struct AppState {
    pub camera: OrbitCamera,
    /// Window size (points) and the free area between the panels.
    pub screen: [f32; 2],
    pub view_rect: [f32; 4],
    pub settings: Settings,
    pub doc: Option<Doc>,
    pub file: Option<PathBuf>,
    pub name: String,
    pub job: Option<Job>,
    /// Space held: the mesh as it came.
    pub compare: bool,
    /// Bumped when the whole mesh must be uploaded again (opened).
    pub gen: u64,
    /// Vertices moved since the last frame (their triangles are uploaded).
    pub dirty: Vec<u32>,
    /// Bumped when what is drawn over the mesh changes.
    pub overlay_gen: u64,
    pub toast: Option<Toast>,
    pub time: f64,
    pub interacting: bool,
    pub files_open: bool,
    pub panel_visible: bool,
    /// No interface, the view only (captures).
    pub bare: bool,
    pub tool: Tool,
    pub line: LineParams,
    pub line_drag: Option<LineDrag>,
    pub line_edit: Option<LineEdit>,
    /// How much of the fix is applied (0..1): the "sharp" strength.
    pub line_strength: f64,
    pub sym: SymState,
    /// The Circle drag (rim to rim) and the part found.
    pub circle_drag: Option<CircleDrag>,
    pub circle: Option<CircleEdit>,
    /// The Circle tool's levels, kept from one part to the next.
    pub circle_levels: usize,
    /// The edits made, in order, and the ones undone.
    pub history: Vec<Step>,
    pub undone: Vec<Step>,
}

impl Default for AppState {
    fn default() -> Self {
        AppState::new()
    }
}

impl AppState {
    pub fn new() -> AppState {
        AppState {
            camera: OrbitCamera::default(),
            screen: [1600.0, 900.0],
            view_rect: [0.0, 0.0, 1600.0, 900.0],
            settings: Settings::default(),
            doc: None,
            file: None,
            name: String::new(),
            job: None,
            compare: false,
            gen: 1,
            dirty: vec![],
            overlay_gen: 1,
            toast: None,
            time: 0.0,
            interacting: false,
            files_open: false,
            panel_visible: true,
            bare: false,
            tool: Tool::Navigate,
            line: LineParams::default(),
            line_drag: None,
            line_edit: None,
            line_strength: 1.0,
            sym: SymState { axis: Axis::Z, mode: SymMode::Average, reach: 1.0, found: None },
            circle_drag: None,
            circle: None,
            circle_levels: 4,
            history: vec![],
            undone: vec![],
        }
    }

    pub fn principal(&self) -> [f32; 2] {
        [(self.view_rect[0] + self.view_rect[2]) * 0.5, (self.view_rect[1] + self.view_rect[3]) * 0.5]
    }

    pub fn view(&self) -> View {
        self.camera.view(self.screen, self.principal())
    }

    pub fn toast(&mut self, text: impl Into<String>, warn: bool) {
        self.toast = Some(Toast { text: text.into(), warn, until: self.time + 3.5 });
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    // ------------------------------------------------------------ files

    fn make_doc(m: Mesh) -> Doc {
        let topo = Topo::new(&m);
        let edge = m.edge_median();
        Doc { original: m.clone(), mesh: m, topo, edge }
    }

    /// Reads an .obj in the background.
    pub fn open(&mut self, path: PathBuf) {
        if self.busy() {
            self.toast("Busy: wait a moment", true);
            return;
        }
        let (tx, rx) = channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let r = Mesh::load_obj(&p).map(|m| Box::new(Self::make_doc(m)));
            let _ = tx.send(Msg::Loaded(r, p));
        });
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        self.job = Some(Job { rx, what: format!("Reading {name}") });
    }

    /// Opens a mesh already in memory (the capture harness).
    pub fn set_mesh(&mut self, m: Mesh, name: &str) {
        self.doc = Some(Self::make_doc(m));
        self.name = name.to_string();
        self.after_load();
    }

    fn after_load(&mut self) {
        self.history.clear();
        self.undone.clear();
        self.line_drag = None;
        self.line_edit = None;
        self.circle_drag = None;
        self.circle = None;
        self.sym.found = None;
        self.dirty.clear();
        self.gen += 1;
        self.overlay_gen += 1;
        self.frame_all();
    }

    pub fn export(&mut self) {
        self.line_apply();
        let Some(d) = &self.doc else { return };
        let m = d.mesh.clone();
        let path = crate::files::documents_dir().join(format!("{} rectified.obj", crate::files::sanitize(&self.name)));
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = std::fs::write(&path, m.obj(None)).map(|_| path.clone()).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Exported(r));
        });
        self.job = Some(Job { rx, what: "Exporting".into() });
    }

    /// Waits for the background job (capture harness, tests).
    pub fn wait(&mut self) {
        while self.job.is_some() {
            self.tick(self.time);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Reads what the background job sent.
    pub fn tick(&mut self, now: f64) {
        self.time = now;
        if self.toast.as_ref().map(|t| now > t.until).unwrap_or(false) {
            self.toast = None;
        }
        let Some(job) = &mut self.job else { return };
        let Ok(m) = job.rx.try_recv() else { return };
        self.job = None;
        match m {
            Msg::Loaded(Ok(d), path) => {
                self.doc = Some(*d);
                self.name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                self.file = Some(path);
                self.after_load();
            }
            Msg::Loaded(Err(e), _) => self.toast(format!("Could not read the file: {e}"), true),
            Msg::Exported(Ok(path)) => self.toast(format!("Exported to {}", path.display()), false),
            Msg::Exported(Err(e)) => self.toast(format!("Export failed: {e}"), true),
            Msg::Edit(tool, what, moves, note) => {
                self.commit(tool, what, moves);
                self.toast(note, false);
                self.overlay_gen += 1;
            }
        }
    }

    // ------------------------------------------------------------ view

    pub fn frame_all(&mut self) {
        match self.doc.as_ref().map(|d| d.original.bounds()) {
            Some((lo, hi)) => self.camera.frame(lo.as_vec3(), hi.as_vec3()),
            None => self.camera.frame(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 20.0, 20.0)),
        }
    }

    /// The triangle under a screen point and where the ray hits it.
    pub fn pick_hit(&self, at: [f32; 2]) -> Option<(usize, DVec3)> {
        let m = &self.doc.as_ref()?.mesh;
        let (o, d) = self.view().ray(at[0], at[1]);
        let (o, d) = (o.as_dvec3(), d.as_dvec3());
        (0..m.tris.len())
            .into_par_iter()
            .filter_map(|t| {
                let [a, b, c] = m.tri_pts(t);
                ray_tri(o, d, a, b, c).map(|s| (s, t))
            })
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
            .map(|(s, t)| (t, o + d * s))
    }

    pub fn set_tool(&mut self, t: Tool) {
        if t != Tool::Navigate && self.doc.is_none() {
            self.toast("Open a mesh first", true);
            return;
        }
        // a Line placed is kept
        self.line_apply();
        self.tool = t;
        self.line_drag = None;
        self.circle_drag = None;
        if t != Tool::Circle {
            self.circle = None;
        }
        self.overlay_gen += 1;
        if t == Tool::Mirror && self.sym.found.is_none() {
            self.find_mirror(None);
        }
    }

    // ------------------------------------------------------------ Mirror

    /// The mirror plane square to an axis (None: the axis that fits best).
    pub fn find_mirror(&mut self, axis: Option<Axis>) {
        let Some(d) = &self.doc else { return };
        let reach = self.sym.reach;
        let best = match axis {
            Some(a) => (a, sym::find(&d.mesh, a, reach)),
            None => Axis::ALL.into_iter().map(|a| (a, sym::find(&d.mesh, a, reach))).min_by(|x, y| x.1.gap.partial_cmp(&y.1.gap).unwrap()).unwrap(),
        };
        self.sym.axis = best.0;
        self.sym.found = Some(best.1);
        self.overlay_gen += 1;
    }

    /// Both sides made mirror images (in the background).
    pub fn apply_mirror(&mut self) {
        if self.busy() {
            return;
        }
        let (Some(d), Some(f)) = (&self.doc, self.sym.found) else { return };
        let m = d.mesh.clone();
        let topo = Topo::new(&m);
        let (mode, reach) = (self.sym.mode, self.sym.reach);
        let axis = self.sym.axis;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let surf = Surface::new(&m);
            let (moves, share) = sym::apply(&m, &topo, &surf, &f.mirror, mode, reach);
            let how = match mode {
                SymMode::Average => "both sides averaged",
                SymMode::PlusToMinus => "+ side copied",
                SymMode::MinusToPlus => "− side copied",
            };
            let what = format!("{} · {how}", axis.label());
            let note = format!("Mirror {}: {how} · {:.0} % of the vertices found their partner · {} mm at most", axis.label(), share * 100.0, fmt(moves.max_move(), 2));
            let _ = tx.send(Msg::Edit(Tool::Mirror, what, moves, note));
        });
        self.job = Some(Job { rx, what: "Mirroring".into() });
    }

    // ------------------------------------------------------------ edits

    /// Applies an edit and keeps it in the history.
    pub fn commit(&mut self, tool: Tool, what: String, moves: Moves) {
        let Some(d) = &mut self.doc else { return };
        if moves.is_empty() {
            self.toast("Nothing moved", true);
            return;
        }
        moves.apply(&mut d.mesh);
        self.dirty.extend_from_slice(&moves.idx);
        self.history.push(Step { tool, what, moves });
        self.undone.clear();
    }

    pub fn undo(&mut self) {
        // a Line placed and not applied goes first
        if self.line_edit.is_some() {
            self.line_cancel();
            self.toast("Line removed", false);
            return;
        }
        let Some(d) = &mut self.doc else { return };
        let Some(s) = self.history.pop() else {
            self.toast("Nothing to undo", false);
            return;
        };
        s.moves.undo(&mut d.mesh);
        self.dirty.extend_from_slice(&s.moves.idx);
        self.toast(format!("Undone: {}", s.what), false);
        self.undone.push(s);
        self.line_drag = None;
        self.overlay_gen += 1;
    }

    pub fn redo(&mut self) {
        let Some(d) = &mut self.doc else { return };
        let Some(s) = self.undone.pop() else {
            self.toast("Nothing to redo", false);
            return;
        };
        s.moves.apply(&mut d.mesh);
        self.dirty.extend_from_slice(&s.moves.idx);
        self.toast(format!("Redone: {}", s.what), false);
        self.history.push(s);
        self.overlay_gen += 1;
    }

    /// Back to the mesh as it came (undoable step by step).
    pub fn revert_all(&mut self) {
        self.line_cancel();
        while !self.history.is_empty() {
            self.undo();
        }
        self.toast("Back to the mesh as it came (Ctrl+Shift+Z to redo)", false);
    }

    // ------------------------------------------------------------ Line

    /// The vertices of a triangle.
    pub fn tri_verts(&self, t: usize) -> Vec<u32> {
        self.doc.as_ref().map(|d| d.mesh.tris[t].to_vec()).unwrap_or_default()
    }

    pub fn line_press(&mut self, at: [f32; 2]) {
        let Some((t, x)) = self.pick_hit(at) else { return };
        // a new stroke keeps the one placed before
        self.line_apply();
        let seeds = self.tri_verts(t);
        self.line_drag = Some(LineDrag { a: x, b: x, seeds_b: seeds.clone(), seeds, fit: None, fitted_at: None });
        self.overlay_gen += 1;
    }

    pub fn line_move(&mut self, at: [f32; 2]) {
        if self.line_drag.is_none() {
            return;
        }
        let Some((t, x)) = self.pick_hit(at) else { return };
        let sb = self.tri_verts(t);
        let ld = self.line_drag.as_mut().unwrap();
        if ld.b == x {
            return;
        }
        ld.b = x;
        ld.seeds_b = sb;
        self.overlay_gen += 1;
        // the edge it would fix, again when the end moved a little
        let h = self.doc.as_ref().map(|d| d.edge).unwrap_or(0.1);
        let moved = ld.fitted_at.map(|p| (p - x).length() > h * 2.0).unwrap_or(true);
        if moved {
            self.refit_line();
        }
    }

    fn refit_line(&mut self) {
        let (Some(d), Some(ld)) = (&self.doc, &mut self.line_drag) else { return };
        let mut seeds = ld.seeds.clone();
        seeds.extend_from_slice(&ld.seeds_b);
        ld.fit = Some(line::fix(&d.mesh, &d.topo, &seeds, ld.a, ld.b, d.edge, &self.line));
        ld.fitted_at = Some(ld.b);
        self.overlay_gen += 1;
    }

    pub fn line_release(&mut self) {
        let Some(ld) = &self.line_drag else { return };
        if ld.fitted_at != Some(ld.b) {
            self.refit_line();
        }
        let ld = self.line_drag.take().unwrap();
        self.overlay_gen += 1;
        match ld.fit {
            Some(Ok(fit)) => {
                let mut seeds = ld.seeds;
                seeds.extend_from_slice(&ld.seeds_b);
                self.line_edit = Some(LineEdit { a: ld.a, b: ld.b, seeds, fit });
                self.line_show();
            }
            Some(Err(e)) => self.toast(e, true),
            None => {}
        }
    }

    /// The Line placed, on the mesh at the strength set.
    fn line_show(&mut self) {
        let (Some(d), Some(le)) = (&mut self.doc, &self.line_edit) else { return };
        let s = self.line_strength;
        let mv = &le.fit.moves;
        for k in 0..mv.idx.len() {
            d.mesh.pos[mv.idx[k] as usize] = mv.from[k] + (mv.to[k] - mv.from[k]) * s;
        }
        self.dirty.extend_from_slice(&mv.idx);
        self.overlay_gen += 1;
    }

    /// The mesh as it was before the Line placed.
    fn line_hide(&mut self) {
        let (Some(d), Some(le)) = (&mut self.doc, &self.line_edit) else { return };
        let mv = &le.fit.moves;
        for k in 0..mv.idx.len() {
            d.mesh.pos[mv.idx[k] as usize] = mv.from[k];
        }
        self.dirty.extend_from_slice(&mv.idx);
        self.overlay_gen += 1;
    }

    /// The strength changed: shown again at once.
    pub fn line_set_strength(&mut self, s: f64) {
        self.line_strength = s.clamp(0.0, 1.0);
        self.line_show();
    }

    /// The width or the hold changed: the Line placed fitted again.
    pub fn line_refit_edit(&mut self) {
        if self.line_edit.is_none() {
            return;
        }
        self.line_hide();
        let (Some(d), Some(le)) = (&self.doc, &mut self.line_edit) else { return };
        match line::fix(&d.mesh, &d.topo, &le.seeds, le.a, le.b, d.edge, &self.line) {
            Ok(f) => le.fit = f,
            Err(e) => self.toast(e, true),
        }
        self.line_show();
    }

    /// The Line placed goes into the history, at the strength set.
    pub fn line_apply(&mut self) {
        if self.line_edit.is_none() {
            return;
        }
        self.line_hide();
        let Some(le) = self.line_edit.take() else { return };
        let s = self.line_strength;
        let Some(d) = &self.doc else { return };
        let f = le.fit;
        let targets: Vec<(u32, DVec3)> = f.moves.idx.iter().zip(f.moves.from.iter().zip(&f.moves.to)).map(|(&v, (a, b))| (v, *a + (*b - *a) * s)).collect();
        let moves = Moves::new(&d.mesh, targets);
        let what = format!("Edge {:.0}° · {} mm · {:.0} %", f.angle, fmt((f.b - f.a).length(), 1), s * 100.0);
        let n = moves.idx.len();
        let mx = moves.max_move();
        self.commit(Tool::Line, what, moves);
        self.toast(format!("Edge fixed at {:.0} %: the sides turn {:.0}° · {n} vertices moved, {} mm at most", s * 100.0, f.angle, fmt(mx, 2)), false);
        self.overlay_gen += 1;
    }

    /// The Line placed taken away.
    pub fn line_cancel(&mut self) {
        self.line_hide();
        self.line_edit = None;
        self.overlay_gen += 1;
    }
}

impl AppState {
    // ------------------------------------------------------------ Circle

    pub fn circle_press(&mut self, at: [f32; 2]) {
        let Some((t, x)) = self.pick_hit(at) else { return };
        let seeds = self.tri_verts(t);
        let Some(d) = &self.doc else { return };
        let n = seeds.iter().flat_map(|&v| d.topo.tris_of(v as usize).iter()).map(|&t| d.mesh.tri_cross(t as usize).normalize_or_zero()).sum::<DVec3>().normalize_or_zero();
        self.circle_drag = Some(CircleDrag { a: x, b: x, seeds, n });
        self.circle = None;
        self.overlay_gen += 1;
    }

    pub fn circle_move(&mut self, at: [f32; 2]) {
        if self.circle_drag.is_none() {
            return;
        }
        // on the plane under the press (else the mesh under the pointer)
        let (o, dir) = self.view().ray(at[0], at[1]);
        let (o, dir) = (o.as_dvec3(), dir.as_dvec3());
        let hit = self.pick_hit(at).map(|h| h.1);
        let Some(cd) = &mut self.circle_drag else { return };
        let dn = dir.dot(cd.n);
        let on_plane = (dn.abs() > 0.05).then(|| o + dir * ((cd.a - o).dot(cd.n) / dn)).filter(|x| (*x - o).dot(dir) > 0.0);
        if let Some(x) = on_plane.or(hit) {
            cd.b = x;
        }
        self.overlay_gen += 1;
    }

    pub fn circle_release(&mut self) {
        let Some(CircleDrag { a, b, seeds, .. }) = self.circle_drag.take() else { return };
        self.overlay_gen += 1;
        let Some(d) = &self.doc else { return };
        match Circle::detect(&d.mesh, &d.topo, &seeds, a, b, d.edge) {
            Ok(c) => {
                let height = c.height;
                let reach = (c.r * 0.3).max(d.edge * 4.0);
                let levels = self.circle_levels;
                let rings = c.rings(&d.mesh, height, levels);
                let off = c.out_of_round(&d.mesh, &rings);
                self.circle = Some(CircleEdit { circle: c, height, levels, reach, rings, off });
            }
            Err(e) => self.toast(e, true),
        }
    }

    /// The rings again after the height or the levels changed.
    pub fn circle_refit(&mut self) {
        let (Some(d), Some(ce)) = (&self.doc, &mut self.circle) else { return };
        ce.rings = ce.circle.rings(&d.mesh, ce.height, ce.levels);
        ce.off = ce.circle.out_of_round(&d.mesh, &ce.rings);
        self.circle_levels = ce.levels;
        self.overlay_gen += 1;
    }

    pub fn circle_apply(&mut self) {
        let Some(ce) = self.circle.take() else { return };
        let Some(d) = &self.doc else { return };
        let moves = ce.circle.apply(&d.mesh, &d.topo, &ce.rings, ce.reach);
        let what = format!("Ø {} mm · {} mm high · {} rings", fmt(ce.circle.r * 2.0, 2), fmt(ce.height, 1), ce.rings.len());
        let n = moves.idx.len();
        let mx = moves.max_move();
        self.commit(Tool::Circle, what, moves);
        let after = self.doc.as_ref().map(|d| ce.circle.out_of_round(&d.mesh, &ce.rings)).unwrap_or(0.0);
        self.toast(format!("Made round: {n} vertices moved, {} mm at most · out of round {} → {} mm", fmt(mx, 2), fmt(ce.off, 3), fmt(after, 3)), false);
        self.overlay_gen += 1;
    }
}

/// A number with a decimal comma.
pub fn fmt(v: f64, digits: usize) -> String {
    format!("{:.*}", digits, v).replace('.', ",")
}

/// Möller–Trumbore: distance along the ray, if it hits.
fn ray_tri(o: DVec3, d: DVec3, a: DVec3, b: DVec3, c: DVec3) -> Option<f64> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-14 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}
