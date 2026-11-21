//! Groups (Romain, 21 nov. 2026, 12 h 37: "des groupes avec le meilleur de
//! Maya et de 3ds Max"). A group is shapes put in a transform of their own
//! (Maya): it has a pivot, a rotation and a scale, shown as its transform
//! properties, and moving, turning or scaling it carries its shapes. It is
//! closed or open (3ds Max): closed, as it is made, a click on one of its
//! shapes takes them all; open, they are picked and moved one by one;
//! closed again, it is one thing again. The outliner shows it as a row with
//! its arrow and its shapes under it; a small bar above it in the view says
//! "Closed" or "Open".
//!
//! In the document a group is `Doc.groups` (`Group`): the kernel does not
//! use it, so making, opening, closing or naming one does not build the
//! part again (`AppState::edit_quiet`). Its shapes keep their own places
//! in the document; a group selected is its shapes selected
//! (`selected_group`), so the gizmo, Delete, Duplicate, Align, Array work
//! on it as on any selection.

use crate::state::{carried, from_norm, scaled, to_norm, AppState, SelMode};
use glam::{DQuat, DVec3};
use nurbs_kernel::{Feature, FeatureId, Group};

impl AppState {
    /// The group that is selected: all its shapes, and nothing else. A
    /// closed group's shapes are only ever selected together; an open one
    /// is the group when its row was clicked (`group_sel`), not when its
    /// shapes were picked one by one.
    pub fn selected_group(&self) -> Option<&Group> {
        if self.sel_mode != SelMode::Object {
            return None;
        }
        let g = self.doc.group_of(*self.selection.first()?)?;
        let whole = g.members.len() == self.selection.len() && g.members.iter().all(|m| self.selection.contains(m));
        (whole && (!g.open || self.group_sel == Some(g.id))).then_some(g)
    }

    /// The group whose bar shows in the view: the selected one, else the
    /// open group a selected shape is in (so it can be closed from there).
    pub fn shown_group(&self) -> Option<&Group> {
        if self.sel_mode != SelMode::Object {
            return None;
        }
        self.selected_group().or_else(|| self.selection.iter().find_map(|id| self.doc.group_of(self.element_of(*id)).filter(|g| g.open)))
    }

    /// What a click on a shape takes in Object mode: its closed group's
    /// shapes, else the shape alone.
    pub fn pick_ids(&self, id: FeatureId) -> Vec<FeatureId> {
        match self.doc.group_of(id) {
            Some(g) if !g.open => g.members.clone(),
            _ => vec![id],
        }
    }

    /// A click on a shape in Object mode (the view, the outliner): a closed
    /// group's shape takes the group; Shift / Ctrl adds or takes away, the
    /// whole group at once.
    pub fn click_shape(&mut self, id: Option<FeatureId>, add: bool) {
        let ids = id.map(|id| self.pick_ids(id)).unwrap_or_default();
        if ids.len() < 2 {
            self.select(id, add);
            return;
        }
        if !add {
            self.selection = ids;
        } else if ids.iter().all(|i| self.selection.contains(i)) {
            self.selection.retain(|s| !ids.contains(s));
        } else {
            for i in ids {
                if !self.selection.contains(&i) {
                    self.selection.push(i);
                }
            }
        }
        self.group_sel = None;
        self.renaming = false;
    }

    /// Selects a group as a group (its row in the outliner, just made or
    /// closed), open or not.
    pub fn select_group(&mut self, id: FeatureId, add: bool) {
        let Some(g) = self.doc.group(id) else { return };
        let ids = g.members.clone();
        if add {
            if ids.iter().all(|i| self.selection.contains(i)) {
                self.selection.retain(|s| !ids.contains(s));
            } else {
                for i in ids {
                    if !self.selection.contains(&i) {
                        self.selection.push(i);
                    }
                }
            }
            self.group_sel = None;
        } else {
            self.selection = ids;
            self.group_sel = Some(id);
        }
        self.edge_sel.clear();
        self.face_sel.clear();
        self.vert_sel.clear();
        self.renaming = false;
    }

    /// The groups a move of these shapes carries: those whose shapes are
    /// all among them, as groups (an open group's shapes moved one by one
    /// leave its transform where it is, as children do in Maya).
    pub fn whole_groups(&self, ids: &[FeatureId]) -> Vec<Group> {
        self.doc.groups.iter().filter(|g| g.members.iter().all(|m| ids.contains(m)) && (!g.open || self.group_sel == Some(g.id))).cloned().collect()
    }

    /// The shapes Ctrl + G would group: the elements selected in Object
    /// mode.
    fn groupable(&self) -> Vec<FeatureId> {
        let mut ids: Vec<FeatureId> = vec![];
        if self.sel_mode != SelMode::Object {
            return ids;
        }
        for id in &self.selection {
            let el = self.element_of(*id);
            if self.doc.feature(el).is_some_and(|f| f.is_solid()) && !ids.contains(&el) {
                ids.push(el);
            }
        }
        ids
    }

    /// Whether Group has something to do: shapes selected that are not
    /// one group already.
    pub fn can_group(&self) -> bool {
        !self.groupable().is_empty() && self.selected_group().is_none()
    }

    /// Ctrl + G: the selected shapes made a group, closed, selected.
    pub fn group_selection(&mut self) {
        if self.selected_group().is_some() {
            self.toast("Already a group · Ctrl + Shift + G: ungroup", false);
            return;
        }
        let ids = self.groupable();
        if ids.is_empty() {
            self.toast("Group: select the shapes to group first (Object mode)", true);
            return;
        }
        let mut made = None;
        self.edit_quiet("Group", |d| made = d.add_group(&ids));
        let Some(id) = made else { return };
        self.select_group(id, false);
        let n = ids.len();
        self.toast(&format!("Group of {n} shape{} · closed: a click takes {} · Ctrl + Shift + G: ungroup", if n > 1 { "s" } else { "" }, if n > 1 { "them all" } else { "it" }), false);
    }

    /// Ctrl + Shift + G: the group taken away, its shapes left where they
    /// are, selected.
    pub fn ungroup_selection(&mut self) {
        let Some(g) = self.shown_group().cloned() else {
            self.toast("Ungroup: select a group first", true);
            return;
        };
        self.ungroup(g.id);
    }

    pub fn ungroup(&mut self, id: FeatureId) {
        let Some(g) = self.doc.group(id).cloned() else { return };
        self.edit_quiet("Ungroup", |d| {
            d.ungroup(id);
        });
        self.selection = g.members.iter().copied().filter(|m| self.doc.feature(*m).is_some()).collect();
        self.group_sel = None;
        let n = g.members.len();
        self.toast(&format!("{} ungrouped: its {n} shape{} stay where they are", g.name, if n > 1 { "s" } else { "" }), false);
    }

    /// Opens a group (its shapes picked one by one) or closes it again (one
    /// thing, selected, as in 3ds Max).
    pub fn set_group_open(&mut self, id: FeatureId, open: bool) {
        let Some(g) = self.doc.group(id).cloned() else { return };
        if g.open == open {
            return;
        }
        let was_selected = self.selected_group().map(|x| x.id) == Some(id);
        self.edit_quiet(if open { "Open group" } else { "Close group" }, |d| {
            if let Some(x) = d.group_mut(id) {
                x.open = open;
            }
        });
        if !open {
            self.set_sel_mode(SelMode::Object);
            self.select_group(id, false);
            self.toast(&format!("{} closed: a click takes all its shapes", g.name), false);
        } else {
            if was_selected {
                self.group_sel = Some(id);
            }
            self.toast(&format!("{} open: its shapes are picked one by one · close it from its bar", g.name), false);
        }
    }

    pub fn rename_group(&mut self, id: FeatureId, name: &str) {
        let name = name.to_string();
        self.edit_quiet("Rename", |d| {
            if let Some(g) = d.group_mut(id) {
                g.name = name;
            }
        });
    }

    /// A group's shapes all hidden, or all shown again.
    pub fn toggle_group_visible(&mut self, id: FeatureId) {
        let Some(g) = self.doc.group(id).cloned() else { return };
        let any = g.members.iter().any(|m| self.doc.feature(*m).is_some_and(|f| f.visible));
        self.edit(if any { "Hide" } else { "Show" }, |d| {
            for m in &g.members {
                if let Some(f) = d.feature_mut(*m) {
                    f.visible = !any;
                }
            }
        });
    }

    /// The pivot of a group put on a point (D), or back at the middle of
    /// its shapes' box (None). Its shapes do not move.
    pub fn put_group_pivot(&mut self, id: FeatureId, at: Option<DVec3>) {
        let Some(g) = self.doc.group(id).cloned() else { return };
        let Some(at) = at.or_else(|| self.doc.group_box(&g).map(|(lo, hi)| g.matrix().transform_point3((lo + hi) * 0.5))) else { return };
        self.edit_quiet("Group pivot", |d| {
            if let Some(x) = d.group_mut(id) {
                x.origin = at;
            }
        });
    }

    /// The 8 corners of a group's box in the world: round its shapes, in
    /// its own frame, a little larger so it does not lie on their faces.
    pub fn group_corners(&self, g: &Group) -> Option<[DVec3; 8]> {
        let (lo, hi) = self.doc.group_box(g)?;
        let pad = ((hi - lo).length() * 0.02).max(0.3);
        let (lo, hi) = (lo - DVec3::splat(pad), hi + DVec3::splat(pad));
        let m = g.matrix();
        let mut out = [DVec3::ZERO; 8];
        for (i, c) in out.iter_mut().enumerate() {
            *c = m.transform_point3(DVec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z }));
        }
        Some(out)
    }

    /// What the view draws of the groups, as lines with their strength
    /// (0 to 255): the selected group's box by its corners (3ds Max's
    /// selection brackets), an open group's whole box, fainter, so one
    /// sees that it is open and what it holds.
    pub fn group_lines(&self) -> Vec<(Vec<Vec<DVec3>>, u8)> {
        let mut out = vec![];
        if self.sel_mode != SelMode::Object || self.doc.groups.is_empty() {
            return out;
        }
        let selected = self.selected_group().map(|g| g.id);
        let shown = self.shown_group().map(|g| g.id);
        for g in &self.doc.groups {
            if !g.open && selected != Some(g.id) {
                continue;
            }
            if !g.members.iter().any(|m| self.doc.feature(*m).is_some_and(|f| f.visible)) {
                continue;
            }
            let Some(c) = self.group_corners(g) else { continue };
            let mut lines: Vec<Vec<DVec3>> = vec![];
            for i in 0..8usize {
                for a in 0..3 {
                    let j = i ^ (1 << a);
                    if g.open {
                        // the whole box, each edge once
                        if i < j {
                            lines.push(vec![c[i], c[j]]);
                        }
                    } else {
                        // a bracket at each corner
                        lines.push(vec![c[i], c[i] + (c[j] - c[i]) * 0.16]);
                    }
                }
            }
            let alpha = match (g.open, shown == Some(g.id)) {
                (false, _) => 215,
                (true, true) => 150,
                (true, false) => 70,
            };
            out.push((lines, alpha));
        }
        out
    }

    /// A group's transform set from its properties (position, rotation,
    /// scale): its shapes are scaled in its frame about its pivot, turned
    /// round it and moved with it, their operations' points carried along.
    /// A slider or field drag is one undo step (as `ui::feature_edit`).
    pub fn set_group_transform(&mut self, id: FeatureId, origin: DVec3, rot: DQuat, scale: DVec3, label: &str, started: bool) {
        let Some(g0) = self.doc.group(id).cloned() else { return };
        let scale = scale.map(|s| if s.is_finite() { s.clamp(0.01, 1000.0) } else { 1.0 });
        if !origin.is_finite() || !rot.is_finite() {
            return;
        }
        let rot = rot.normalize();
        if started {
            self.end_live();
            self.begin_live(label);
        } else if !self.is_live() {
            self.begin_live(label);
        }
        let k = scale / g0.scale;
        let dq = (rot * g0.rot.inverse()).normalize();
        let ids = self.doc.with_operands(&g0.members);
        let olds: Vec<Feature> = self.doc.features.iter().filter(|f| ids.contains(&f.id) && f.is_solid()).cloned().collect();
        let mut out: Vec<Feature> = vec![];
        for old in &olds {
            let mut f = if (k - DVec3::ONE).abs().max_element() > 1e-12 { scaled(old, g0.origin, g0.rot, k) } else { old.clone() };
            f.rot = (dq * f.rot).normalize();
            f.origin = origin + dq * (f.origin - g0.origin);
            // its operations' points, from its frame and sizes before to those now
            for o in self.element_ops(old.id) {
                if let Some(op) = self.doc.feature(o) {
                    let m = |p: DVec3| from_norm(&f, to_norm(old, p));
                    out.push(carried(op, &m));
                }
            }
            out.push(f);
        }
        self.live(|d| {
            for f in &out {
                if let Some(x) = d.feature_mut(f.id) {
                    *x = f.clone();
                }
            }
            if let Some(g) = d.group_mut(id) {
                g.origin = origin;
                g.rot = rot;
                g.scale = scale;
            }
        });
        self.custom_pivot = None;
    }
}
