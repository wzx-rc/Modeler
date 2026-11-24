//! Mirror (the tool proposed to Romain on 20 nov., 15 h 42, when he asked
//! for one that was missing): a step that makes what is built before it
//! symmetric across a plane square to a world axis; every step after it is
//! made on both sides by the kernel (`build::mirrored_doc`, `twin_tool`).
//! A click on either side names the same shape or face: both carry the
//! same tags.

use crate::state::{AppState, ToastAct};
use glam::DVec3;
use nurbs_kernel::{Feature, FeatureId, Mirror, Sym};

impl AppState {
    /// The selected Mirror step.
    pub fn selected_mirror(&self) -> Option<&Feature> {
        self.single().filter(|f| f.mirror.is_some())
    }

    /// Adds a Mirror at the end (Alt + M, the icon by Align, the marking
    /// menu on empty space), across the world's `axis` through the origin,
    /// keeping the side most of the part is on; it is selected.
    pub fn add_mirror(&mut self, axis: u8) -> Option<FeatureId> {
        if self.doc.features.iter().all(|f| !f.visible || f.is_path()) {
            self.toast("Nothing to mirror yet", true);
            return None;
        }
        self.abort_gestures();
        let a = axis.min(2) as usize;
        let flip = self.model.as_ref().and_then(|m| m.bounds()).map(|(lo, hi)| (lo[a] + hi[a]) < -1e-3).unwrap_or(false);
        let m = Mirror { axis: axis.min(2), offset: 0.0, flip, single: false };
        let mut id = None;
        self.edit("Mirror", |d| id = Some(d.add_mirror(m)));
        if let Some(id) = id {
            self.selection = vec![id];
            self.toast(&format!("Mirror across {}: the steps after it are made on both sides", m.axis_name()), false);
        }
        id
    }

    /// The shapes whose own symmetry the Mirror bar sets (Romain 23 nov.,
    /// 21 h 03: "Chaque objet doit avoir sa symétrie en local"): the
    /// elements selected or picked from (a pocket's: the shape it cuts),
    /// solids only. Empty: the bar sets the scene's Mirror.
    pub fn sym_targets(&self) -> Vec<FeatureId> {
        let mut out: Vec<FeatureId> = self.picked_elements().into_iter().map(|id| self.paint_owner(id)).filter(|id| self.doc.feature(*id).is_some_and(|f| f.is_solid() && !f.is_path())).collect();
        out.dedup();
        out
    }

    /// The first target's own symmetry (`sym`).
    pub fn target_sym(&self) -> Option<Sym> {
        self.sym_targets().first().and_then(|id| self.doc.feature(*id)).and_then(|f| f.sym)
    }

    /// Sets the targets' own symmetry (None: off). Its operations are then
    /// made on both sides of the shape's own plane, through its centre,
    /// which moves and turns with it.
    pub fn set_local_sym(&mut self, sym: Option<Sym>) {
        let ids = self.sym_targets();
        if ids.is_empty() {
            return;
        }
        let label = if sym.is_some() { "Symmetry" } else { "Symmetry off" };
        self.edit(label, |d| {
            for id in &ids {
                if let Some(f) = d.feature_mut(*id) {
                    f.sym = sym;
                }
            }
        });
        let name = self.doc.feature(ids[0]).map(|f| f.name.clone()).unwrap_or_default();
        match sym {
            Some(s) => self.toast(&format!("{name}: its own symmetry across its {}, its operations made on both sides", s.axis_name()), false),
            None => self.toast(&format!("{name}: no symmetry of its own"), false),
        }
    }

    /// Alt + M, the Mirror icon: with shapes selected (or picked from), their
    /// own symmetry across their X on or off, the Mirror bar opened; with
    /// nothing, the scene's Mirror as before.
    pub fn mirror_selected(&mut self) {
        if self.sym_targets().is_empty() {
            self.add_mirror(0);
            return;
        }
        let on = self.target_sym().is_some();
        self.set_local_sym(if on { None } else { Some(Sym { axis: 0, flip: false }) });
        if !self.settings.mirror_bar {
            self.settings.mirror_bar = true;
            self.save_settings();
        }
    }

    /// The selected shape's own symmetry plane to draw: its axis, the side
    /// kept, the pane's corners (a little past its box), the normal.
    pub fn local_sym_plane(&self) -> Option<(Sym, [DVec3; 4], DVec3)> {
        let id = *self.sym_targets().first()?;
        let f = self.doc.feature(id)?;
        let s = f.sym?;
        let (c, n) = f.sym_plane()?;
        let (hx, hz) = f.prim.half_xz();
        let half = glam::DVec3::new(hx, f.prim.height() * 0.5, hz) * 1.15 + DVec3::splat(2.0);
        let a = s.axis.min(2) as usize;
        let (u, v) = ((a + 1) % 3, (a + 2) % 3);
        let ax = [DVec3::X, DVec3::Y, DVec3::Z];
        let (du, dv) = (f.rot * ax[u] * half[u], f.rot * ax[v] * half[v]);
        Some((s, [c - du - dv, c + du - dv, c + du + dv, c - du + dv], n))
    }

    /// Sets the selected Mirror (live while a slider is dragged: `start`
    /// begins a new undo step).
    pub fn set_mirror(&mut self, m: Mirror, start: bool) {
        let Some(id) = self.selected_mirror().map(|f| f.id) else { return };
        if start {
            if self.is_live() {
                self.end_live();
            }
            self.begin_live("Mirror");
        } else if !self.is_live() {
            self.begin_live("Mirror");
        }
        self.live(|d| {
            if let Some(f) = d.feature_mut(id) {
                f.mirror = Some(m);
                f.origin = m.normal() * m.offset;
            }
        });
    }

    /// The scene's Mirror, as the Mirror bar shows it: the last Mirror in
    /// the history, on or off.
    pub fn scene_mirror(&self) -> Option<&Feature> {
        self.doc.features.iter().rev().find(|f| f.mirror.is_some())
    }

    /// The Mirrors the bar sets together: the last one and those across the
    /// same plane (his scene of 21 nov. had three across X, one after the
    /// other; set apart they would undo each other).
    pub fn scene_mirror_ids(&self) -> Vec<FeatureId> {
        let Some(m) = self.scene_mirror().and_then(|f| f.mirror) else { return vec![] };
        self.doc.features.iter().filter(|f| f.mirror.is_some_and(|g| g.axis == m.axis && (g.offset - m.offset).abs() < 1e-9)).map(|f| f.id).collect()
    }

    /// Whether the scene's Mirror is on (one of its Mirrors at least).
    pub fn scene_mirror_on(&self) -> bool {
        let ids = self.scene_mirror_ids();
        self.doc.features.iter().any(|f| ids.contains(&f.id) && f.visible && !f.suspended)
    }

    /// The Mirror bar's On: its Mirrors turned on or off (none yet: one
    /// added across `axis`, the selection kept).
    pub fn set_scene_mirror_on(&mut self, on: bool, axis: u8) {
        let ids = self.scene_mirror_ids();
        if ids.is_empty() {
            if on {
                let sel = self.selection.clone();
                self.add_mirror(axis);
                self.selection = sel;
            }
            return;
        }
        self.edit(if on { "Mirror on" } else { "Mirror off" }, |d| {
            for id in &ids {
                if let Some(f) = d.feature_mut(*id) {
                    f.visible = on;
                }
            }
        });
        let m = self.scene_mirror().and_then(|f| f.mirror);
        if let (Some(m), true) = (m, on) {
            self.toast(&format!("Mirror across {} on", m.axis_name()), false);
        } else {
            self.toast("Mirror off: each side is made as it is drawn", false);
        }
    }

    /// The Mirror bar's settings (the plane, the side kept, the steps
    /// after it) given to the scene's Mirrors; live while a value is
    /// dragged (`start` begins a new undo step). No Mirror yet: one added
    /// with them.
    pub fn set_scene_mirror(&mut self, m: Mirror, start: bool) {
        let ids = self.scene_mirror_ids();
        if ids.is_empty() {
            let sel = self.selection.clone();
            if let Some(id) = self.add_mirror(m.axis) {
                self.edit("Mirror", |d| {
                    if let Some(f) = d.feature_mut(id) {
                        f.mirror = Some(Mirror { flip: f.mirror.map(|g| g.flip).unwrap_or(m.flip), ..m });
                        f.origin = m.normal() * m.offset;
                    }
                });
            }
            self.selection = sel;
            return;
        }
        if start {
            if self.is_live() {
                self.end_live();
            }
            self.begin_live("Mirror");
        } else if !self.is_live() {
            self.begin_live("Mirror");
        }
        self.live(|d| {
            for id in &ids {
                if let Some(f) = d.feature_mut(*id) {
                    f.mirror = Some(m);
                    f.origin = m.normal() * m.offset;
                }
            }
        });
    }

    /// What the selection is to the Mirror, for the Mirror bar: a line to
    /// read and, for a step that could be made on one side only, that step
    /// and whether it is.
    pub fn mirror_state(&self) -> Option<(String, Option<(FeatureId, bool)>)> {
        let id = *self.selection.first()?;
        let i = self.doc.features.iter().position(|f| f.id == id)?;
        let f = &self.doc.features[i];
        if f.mirror.is_some() {
            return None;
        }
        let name = if self.selection.len() > 1 { format!("{} and {} more", f.name, self.selection.len() - 1) } else { f.name.clone() };
        let on = |g: &Feature| g.visible && !g.suspended;
        if let Some(m) = self.doc.features[i + 1..].iter().find(|g| on(g) && g.mirror.is_some()).and_then(|g| g.mirror) {
            return Some((format!("{name} · mirrored across {}", m.axis_name()), None));
        }
        if let Some(m) = self.mirrored_by(id) {
            return Some((format!("{name} · both sides of {}", m.axis_name()), Some((id, false))));
        }
        if self.after_mirror(id) {
            let own = !self.mirror_single_before(i);
            return Some((format!("{name} · one side"), own.then_some((id, true))));
        }
        Some((format!("{name} · not mirrored"), None))
    }

    /// The plane of the selected Mirror to draw (else of the scene's Mirror
    /// while it is on, its bar open and its Plane set): its corners round the
    /// part (a little past it) and the Mirror.
    pub fn mirror_plane(&self) -> Option<(Mirror, [DVec3; 4])> {
        let m = match self.selected_mirror().and_then(|f| f.mirror) {
            Some(m) => m,
            None if self.settings.mirror_bar && self.settings.mirror_plane && self.scene_mirror_on() => self.scene_mirror()?.mirror?,
            None => return None,
        };
        let (lo, hi) = self.model.as_ref().and_then(|md| md.bounds()).map(|(lo, hi)| (DVec3::from_array(lo.map(|x| x as f64)), DVec3::from_array(hi.map(|x| x as f64)))).unwrap_or((DVec3::splat(-20.0), DVec3::splat(20.0)));
        let pad = (hi - lo).max_element().max(10.0) * 0.12;
        let (lo, hi) = (lo - DVec3::splat(pad), hi + DVec3::splat(pad));
        let a = m.axis.min(2) as usize;
        let (u, v) = ((a + 1) % 3, (a + 2) % 3);
        let corner = |su: bool, sv: bool| {
            let mut p = DVec3::ZERO;
            p[a] = m.offset;
            p[u] = if su { hi[u] } else { lo[u] };
            p[v] = if sv { hi[v] } else { lo[v] };
            p
        };
        Some((m, [corner(false, false), corner(true, false), corner(true, true), corner(false, true)]))
    }

    /// The Mirror a step is made on both sides of: the last one on before
    /// it, when the step is of a kind the kernel doubles (a shape, a
    /// fillet, an inset or extrusion, a move, the Knife, a Shell's
    /// openings, a Bridge) and not made on this side only.
    pub fn mirrored_by(&self, id: FeatureId) -> Option<Mirror> {
        let i = self.doc.features.iter().position(|f| f.id == id)?;
        let f = &self.doc.features[i];
        let doubled = f.is_solid() || f.round.is_some() || f.push.is_some() || f.tweak.is_some() || f.cut.is_some() || f.shell.is_some() || f.bridge.is_some();
        if !doubled || f.mirror.is_some() || f.array.is_some() || f.one_side || self.mirror_single_before(i) {
            return None;
        }
        self.doc.features[..i].iter().rev().find(|g| g.mirror.is_some() && g.visible && !g.suspended).and_then(|g| g.mirror)
    }

    /// Whether the Mirror just before step i makes its later steps on one
    /// side only (`Mirror.single`, up to the next Mirror).
    pub fn mirror_single_before(&self, i: usize) -> bool {
        self.doc.features[..i.min(self.doc.features.len())].iter().rev().find(|g| g.visible && !g.suspended && g.mirror.is_some()).is_some_and(|g| g.mirror.is_some_and(|m| m.single))
    }

    /// Whether a step comes after a Mirror and could be made on both sides
    /// (its window then shows Both sides / This side only).
    pub fn after_mirror(&self, id: FeatureId) -> bool {
        let Some(i) = self.doc.features.iter().position(|f| f.id == id) else { return false };
        let f = &self.doc.features[i];
        let doubled = f.is_solid() || f.round.is_some() || f.push.is_some() || f.tweak.is_some() || f.cut.is_some() || f.shell.is_some() || f.bridge.is_some();
        doubled && f.mirror.is_none() && self.doc.features[..i].iter().any(|g| g.mirror.is_some() && g.visible && !g.suspended)
    }

    /// Sets a step made after a Mirror on this side only, or on both
    /// (Romain 23 nov., 7 h 07: an Extrude on the left leg made on the
    /// right one too, "sans raison").
    pub fn set_one_side(&mut self, id: FeatureId, on: bool) {
        self.edit(if on { "This side only" } else { "Both sides" }, |d| {
            if let Some(f) = d.feature_mut(id) {
                f.one_side = on;
            }
        });
    }

    /// A step just made after a Mirror: said once, so the image across
    /// the plane is no surprise.
    pub(crate) fn tell_mirror_side(&mut self) {
        let newest = self.doc.features.iter().map(|f| f.id).max().unwrap_or(0);
        if newest <= self.mirror_told {
            return;
        }
        let fresh: Vec<FeatureId> = self.doc.features.iter().map(|f| f.id).filter(|id| *id > self.mirror_told).collect();
        self.mirror_told = newest;
        if let Some((id, m)) = fresh.iter().find_map(|id| self.mirrored_by(*id).map(|m| (*id, m))) {
            self.toast(&format!("Made on both sides of the Mirror across {}", m.axis_name()), false);
            if let Some(t) = self.toast.as_mut() {
                t.until = self.now + 8.0;
                t.acts = vec![("This side only".into(), ToastAct::OneSide(id)), ("Always this side".into(), ToastAct::MirrorSingle(id))];
            }
        }
    }

    /// A Mirror's later steps (up to the next Mirror) made on their own
    /// side only, or on both again (its window, the toast's "Always this
    /// side" for the Mirror just before that step).
    pub fn set_mirror_single(&mut self, mirror: FeatureId, on: bool) {
        self.edit(if on { "Steps after the Mirror on one side" } else { "Steps after the Mirror on both sides" }, |d| {
            if let Some(m) = d.feature_mut(mirror).and_then(|f| f.mirror.as_mut()) {
                m.single = on;
            }
        });
    }

    /// The active Mirror just before a step.
    pub fn mirror_before(&self, id: FeatureId) -> Option<FeatureId> {
        let i = self.doc.features.iter().position(|f| f.id == id)?;
        self.doc.features[..i].iter().rev().find(|g| g.visible && !g.suspended && g.mirror.is_some()).map(|g| g.id)
    }

    /// A toast's button.
    pub fn toast_act(&mut self, a: ToastAct) {
        self.toast = None;
        match a {
            ToastAct::OneSide(id) => {
                self.set_one_side(id, true);
                self.toast("Made on this side only", false);
            }
            ToastAct::FillDeleted(id) => {
                // each deleted face's hole, by the point it was picked at
                let at: Vec<nurbs_kernel::EdgeRef> = self.doc.feature(id).and_then(|f| f.cut.as_ref()).map(|k| k.remove_faces.iter().map(|r| nurbs_kernel::EdgeRef { faces: [r.tag, -1], at: r.at }).collect()).unwrap_or_default();
                self.fill_holes(at);
            }
            ToastAct::FoldAll => self.fold_all_history(),
            ToastAct::MirrorSingle(id) => {
                let Some(m) = self.mirror_before(id) else { return };
                self.set_mirror_single(m, true);
                self.toast("From now on, steps after the Mirror are made on their own side only (its window to undo)", false);
            }
        }
    }
}

