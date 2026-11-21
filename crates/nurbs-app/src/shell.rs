//! Shell (the tool proposed to Romain on 20 nov., 15 h 42 with Mirror and
//! Array; asked at 20 h 58: "Continue les shell"): a step that hollows what
//! is built before it to a wall thickness, the faces it names left open
//! (none: a closed cavity inside). The kernel makes it (`build::shell`,
//! `Shape::shell`: OpenCascade's MakeThickSolid); too thick, it stops at the
//! thickest that works (`Model.limits`). The faces it keeps keep their
//! tags; its inner walls and the rims round the openings carry the step's.

use crate::state::{closest_on_line, AppState, Drag, Handle, SelMode, Tool};
use glam::{DVec3, Vec3};
use nurbs_kernel::model::FaceTag;
use nurbs_kernel::{FaceRef, Feature, FeatureId, Shell};

/// The selected Shell's thickness grip: a line across the wall, from the
/// face it starts from (`from`, along `dir`), and its dot at the thickness.
#[derive(Clone, Copy, Debug)]
pub struct ShellGrip {
    pub from: DVec3,
    pub dir: DVec3,
    pub at: DVec3,
}

/// The nearest point to `p` on a polyline.
fn nearest_on(pts: &[[f32; 3]], p: DVec3) -> Option<DVec3> {
    let mut best: Option<(f64, DVec3)> = None;
    for w in pts.windows(2) {
        let (u, v) = (Vec3::from(w[0]).as_dvec3(), Vec3::from(w[1]).as_dvec3());
        let d = v - u;
        let s = ((p - u).dot(d) / d.length_squared().max(1e-12)).clamp(0.0, 1.0);
        let q = u + d * s;
        let dist = q.distance(p);
        if best.map(|b| dist < b.0).unwrap_or(true) {
            best = Some((dist, q));
        }
    }
    best.map(|b| b.1)
}

impl AppState {
    /// The selected Shell step.
    pub fn selected_shell(&self) -> Option<&Feature> {
        self.single().filter(|f| f.shell.is_some())
    }

    /// Adds a Shell at the end (Alt + S, the icon by Array, the Face
    /// window's Shell): the faces picked in Face mode left open, else a
    /// closed cavity inside; walls of 2 mm (less on a small part). It is
    /// selected, its window open.
    pub fn add_shell(&mut self) -> Option<FeatureId> {
        if !self.doc.features.iter().any(|f| f.visible && !f.suspended && f.is_solid() && !f.is_path() && !f.is_sheet()) {
            self.toast("Nothing to hollow yet", true);
            return None;
        }
        self.abort_gestures();
        let faces: Vec<FaceRef> = if self.sel_mode == SelMode::Face { self.face_sel.clone() } else { vec![] };
        // where its grip starts: the first opening, else the top of the
        // selected shape (or of the part)
        let top = |lo: DVec3, hi: DVec3| DVec3::new((lo.x + hi.x) * 0.5, hi.y, (lo.z + hi.z) * 0.5);
        let bounds = self.model.as_ref().and_then(|m| m.bounds()).map(|(lo, hi)| (DVec3::from_array(lo.map(|x| x as f64)), DVec3::from_array(hi.map(|x| x as f64))));
        let origin = faces
            .first()
            .map(|r| r.at)
            .or_else(|| self.single().filter(|f| f.is_solid() && !f.is_path()).map(|f| f.matrix().transform_point3(DVec3::new(0.0, f.prim.height(), 0.0))))
            .or_else(|| bounds.map(|(lo, hi)| top(lo, hi)))
            .unwrap_or(DVec3::ZERO);
        let small = bounds.map(|(lo, hi)| (hi - lo).min_element()).unwrap_or(40.0);
        let thickness = if small >= 25.0 { 2.0 } else { ((small * 0.08) * 10.0).round().max(2.0) / 10.0 };
        let sh = Shell { faces: faces.clone(), thickness, outward: false };
        let mut id = None;
        self.edit("Shell", |d| id = Some(d.add_shell(sh, origin)));
        let id = id?;
        self.tool = Tool::Select;
        self.face_sel.clear();
        self.edge_sel.clear();
        self.vert_sel.clear();
        self.selection = vec![id];
        let mm = crate::widgets::fmt_mm(thickness as f32);
        if faces.is_empty() {
            self.toast(&format!("Shell {mm} mm: a closed cavity inside (X-ray shows it) · Pick faces to open them"), false);
        } else {
            let n = faces.len();
            self.toast(&format!("Shell {mm} mm, {n} face{} open · drag the dot to change the walls", if n > 1 { "s" } else { "" }), false);
        }
        Some(id)
    }

    /// Sets the selected Shell (live while a slider is dragged: `start`
    /// begins a new undo step).
    pub fn set_shell(&mut self, sh: Shell, start: bool) {
        let Some(id) = self.selected_shell().map(|f| f.id) else { return };
        if start {
            if self.is_live() {
                self.end_live();
            }
            self.begin_live("Shell");
        } else if !self.is_live() {
            self.begin_live("Shell");
        }
        self.live(|d| {
            if let Some(f) = d.feature_mut(id) {
                f.shell = Some(sh);
            }
        });
    }

    /// Picking the faces to open on the selected Shell (its window's Pick
    /// faces); again, back to Select.
    pub fn shell_pick_toggle(&mut self) {
        if self.tool == Tool::ShellPick {
            self.tool = Tool::Select;
            return;
        }
        if self.selected_shell().is_none() {
            return;
        }
        self.abort_gestures();
        self.tool = Tool::ShellPick;
        self.toast("Click the faces to open · Esc or Enter: done", false);
    }

    /// A click while picking: the face under the pointer opened (one undo
    /// step). The faces the Shell kept carry the tags they had before it,
    /// so the step finds them again on what is built before it.
    pub fn shell_pick(&mut self, p: [f32; 2]) {
        let Some(f) = self.selected_shell() else {
            self.tool = Tool::Select;
            return;
        };
        let (id, mut sh, name) = (f.id, f.shell.clone().unwrap(), f.name.clone());
        let Some(h) = self.pick_at(p) else { return };
        let Some(tag) = self.model.as_ref().and_then(|m| m.faces.get(h.face)).and_then(|x| x.tag) else { return };
        if tag.feature == id {
            self.toast(&format!("That face is {name}'s own (a wall or a rim): click an outer face"), true);
            return;
        }
        let at = h.point.as_dvec3();
        if sh.faces.iter().any(|r| r.tag == tag.pack() && r.at.distance(at) < 1e-6) {
            return;
        }
        sh.faces.push(FaceRef { tag: tag.pack(), at });
        let first = sh.faces.len() == 1;
        self.edit("Open a face", |d| {
            if let Some(g) = d.feature_mut(id) {
                g.shell = Some(sh);
                // the grip goes to the first opening
                if first {
                    g.origin = at;
                }
            }
        });
        self.selection = vec![id];
    }

    /// The selected Shell's opening `k` closed again (one undo step).
    pub fn shell_close_face(&mut self, k: usize) {
        let Some(f) = self.selected_shell() else { return };
        let (id, mut sh) = (f.id, f.shell.clone().unwrap());
        if k >= sh.faces.len() {
            return;
        }
        sh.faces.remove(k);
        let at = sh.faces.first().map(|r| r.at);
        self.edit("Close a face", |d| {
            if let Some(g) = d.feature_mut(id) {
                g.shell = Some(sh);
                if let Some(a) = at {
                    g.origin = a;
                }
            }
        });
        self.selection = vec![id];
    }

    /// What an opening of the selected Shell is: "Top of Case" (a box's
    /// or cylinder's own side, in its frame), else "Face of …".
    pub fn shell_face_label(&self, r: &FaceRef) -> String {
        let Some(t) = FaceTag::unpack(r.tag) else { return "Face".into() };
        let Some(g) = self.doc.feature(t.feature) else { return "Face".into() };
        let side = match g.prim {
            _ if !g.is_solid() => None,
            nurbs_kernel::Prim::Box { .. } => ["Right", "Left", "Top", "Bottom", "Front", "Back"].get(t.sub as usize).copied(),
            nurbs_kernel::Prim::Cylinder { .. } => ["Side", "Top", "Bottom"].get(t.sub as usize).copied(),
            nurbs_kernel::Prim::Profile { .. } => Some(match t.sub {
                1 => "Top",
                2 => "Bottom",
                _ => "Side",
            }),
            _ => None,
        };
        match side {
            Some(s) => format!("{s} of {}", g.name),
            None => format!("Face of {}", g.name),
        }
    }

    /// The selected Shell's grip (with the Select tool): across the wall
    /// at the rim of the opening nearest its point, from the faces it kept
    /// to its own (inwards, or outwards); with no opening, from the part's
    /// face at its point along the normal.
    pub fn shell_grip(&self) -> Option<ShellGrip> {
        if self.tool != Tool::Select || self.push.is_some() {
            return None;
        }
        let f = self.selected_shell()?;
        let sh = f.shell.as_ref()?;
        let m = self.model.as_ref()?;
        let tag_of = |i: i32| (i >= 0).then(|| m.faces.get(i as usize).and_then(|x| x.tag)).flatten();
        let rim = |i: i32| tag_of(i).map(|t| t.feature == f.id && t.sub == 0).unwrap_or(false);
        let wall = |i: i32| tag_of(i).map(|t| t.feature == f.id && t.sub != 0).unwrap_or(false);
        let kept = |i: i32| i >= 0 && tag_of(i).map(|t| t.feature != f.id).unwrap_or(true);
        if !sh.faces.is_empty() {
            // the edges between a rim and a face kept, and between a rim
            // and the step's walls
            let (mut from, mut to): (Vec<&[[f32; 3]]>, Vec<&[[f32; 3]]>) = (vec![], vec![]);
            for e in m.edges.iter().filter(|e| !e.seam) {
                let [a, b] = e.faces;
                if (rim(a) && kept(b)) || (rim(b) && kept(a)) {
                    from.push(&e.pts);
                } else if (rim(a) && wall(b)) || (rim(b) && wall(a)) {
                    to.push(&e.pts);
                }
            }
            let near = |set: &[&[[f32; 3]]], p: DVec3| set.iter().filter_map(|l| nearest_on(l, p)).min_by(|x, y| x.distance(p).total_cmp(&y.distance(p)));
            if let Some(q) = near(&from, f.origin) {
                if let Some(r) = near(&to, q) {
                    if let Some(dir) = (r - q).try_normalize() {
                        return Some(ShellGrip { from: q, dir, at: q + dir * sh.thickness });
                    }
                }
            }
        }
        // closed (or no rim found): the face nearest its point
        let p = |i: u32| Vec3::from(m.positions[i as usize]).as_dvec3();
        let (fi, q) = m
            .triangles
            .iter()
            .zip(&m.tri_face)
            .filter(|(_, fi)| kept(**fi as i32))
            .map(|(t, fi)| (*fi as usize, (p(t[0]) + p(t[1]) + p(t[2])) / 3.0))
            .min_by(|a, b| a.1.distance_squared(f.origin).total_cmp(&b.1.distance_squared(f.origin)))?;
        let n = self.normal_near(fi, f.origin)?;
        let from = if q.distance(f.origin) < 2.0 { f.origin } else { q };
        let dir = if sh.outward { n } else { -n };
        Some(ShellGrip { from, dir, at: from + dir * sh.thickness })
    }

    pub(crate) fn begin_shell_drag(&mut self, p: [f32; 2]) {
        let Some(g) = self.shell_grip() else { return };
        let Some(f) = self.selected_shell().cloned() else { return };
        let (ro, rd) = self.ray(p);
        let Some(t) = closest_on_line(g.from, g.dir, ro, rd) else { return };
        self.begin_live("Shell");
        self.drag = Some(Drag { handle: Handle::ShellThick, grab: g.from + g.dir * t, start: vec![f], line: Some((g.from, g.dir)), ops: vec![], carried: Default::default(), pivot: g.from, custom: false, keep: vec![], cage: None, pts: vec![], groups: vec![] });
    }

    pub(crate) fn update_shell_drag(&mut self, d: &Drag, p: [f32; 2]) {
        let (Some((o, dir)), Some(f)) = (d.line, d.start.first()) else { return };
        let Some(s0) = f.shell.clone() else { return };
        let (ro, rd) = self.ray(p);
        let Some(t) = closest_on_line(o, dir, ro, rd) else { return };
        let amount = t - (d.grab - o).dot(dir);
        // walls are thin: 0.1 mm steps while Snap is on (not the grid's)
        let raw = s0.thickness + amount;
        let thickness = if self.settings.snap { (raw * 10.0).round() / 10.0 } else { raw }.max(0.05);
        let mm = crate::widgets::fmt_mm(thickness as f32);
        self.gizmo_readout = Some(format!("Thickness {mm} mm"));
        if self.doc.feature(f.id).and_then(|g| g.shell.as_ref()).map(|s| (s.thickness - thickness).abs() < 1e-9).unwrap_or(true) {
            return;
        }
        let id = f.id;
        self.live(|doc| {
            if let Some(s) = doc.feature_mut(id).and_then(|g| g.shell.as_mut()) {
                s.thickness = thickness;
            }
        });
    }
}
