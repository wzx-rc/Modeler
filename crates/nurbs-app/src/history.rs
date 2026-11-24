//! An element's own history (Romain, 19 nov.: "une history qui est propre
//! à chaque objet… je peux cliquer et ça revient en arrière sur une
//! opération… comme dans ZBrush"): its operations in order and a marker
//! that goes back (those past it are `suspended`, left out of the build
//! but kept); operations turned off and on by hand (`visible`); the
//! scene's undo history, which the strip shows when nothing is selected.

use crate::state::AppState;
use nurbs_kernel::{Doc, FaceTag, FeatureId};
use std::collections::HashMap;

impl AppState {
    /// The element whose history the strip shows: the selected one (for
    /// an operation, its element), else the one of the first picked face
    /// or edge. None: the scene.
    #[allow(dead_code)]
    pub fn history_element(&self) -> Option<FeatureId> {
        if let Some(f) = self.single() {
            let id = self.element_of(f.id);
            return self.doc.feature(id).filter(|g| g.is_solid() || g.is_path()).map(|_| id);
        }
        if !self.selection.is_empty() {
            return None;
        }
        let tag = self.face_sel.first().map(|r| r.tag).or_else(|| self.edge_sel.first().map(|e| e.faces[0]))?;
        let id = self.element_of(FaceTag::unpack(tag)?.feature);
        self.doc.feature(id).filter(|g| g.is_solid()).map(|_| id)
    }

    /// An element's operations, in history order.
    pub fn element_ops(&self, el: FeatureId) -> Vec<FeatureId> {
        self.doc.features.iter().filter(|g| !g.is_solid() && !g.is_path() && self.owner_of(g.id) == Some(el)).map(|g| g.id).collect()
    }

    /// Where an element's marker is: how many of its operations are built
    /// (those before the first one past the marker).
    pub fn marker_of(&self, el: FeatureId) -> usize {
        let ops = self.element_ops(el);
        ops.iter().position(|id| self.doc.feature(*id).map(|f| f.suspended).unwrap_or(false)).unwrap_or(ops.len())
    }

    /// Moves an element's marker: its first `n` operations built, the
    /// others left out until it moves past them again. Live while the
    /// strip is scrubbed (one undo step), else one undo step.
    pub fn set_marker(&mut self, el: FeatureId, n: usize) {
        let ops = self.element_ops(el);
        let apply = move |d: &mut Doc| {
            for (i, id) in ops.iter().enumerate() {
                if let Some(g) = d.feature_mut(*id) {
                    g.suspended = i >= n;
                }
            }
        };
        if self.is_live() {
            self.live(apply);
        } else {
            self.edit("History", apply);
        }
    }

    /// Turns an operation off or on by hand. The chain is kept: the
    /// operations after it are still built (one that names a face or an
    /// edge only it made fails, and the model says so).
    pub fn toggle_op(&mut self, id: FeatureId) {
        let Some(f) = self.doc.feature(id) else { return };
        let on = f.visible;
        let name = f.name.clone();
        self.edit(if on { "Turn off" } else { "Turn on" }, |d| {
            if let Some(f) = d.feature_mut(id) {
                f.visible = !on;
            }
        });
        self.toast(&format!("{name} {}", if on { "off" } else { "on" }), false);
    }

    /// Keeps each element's operations past its marker at the end of its
    /// history: an operation made while the marker is back goes in at the
    /// marker (as in Fusion's timeline), before those left out.
    pub(crate) fn keep_markers(&mut self) {
        if !self.doc.features.iter().any(|f| f.suspended) {
            return;
        }
        for _ in 0..self.doc.features.len() {
            let mut first_out: HashMap<FeatureId, usize> = HashMap::new();
            let mut fix = None;
            for (i, f) in self.doc.features.iter().enumerate() {
                if f.is_solid() || f.is_path() {
                    continue;
                }
                let Some(el) = self.owner_of(f.id) else { continue };
                if f.suspended {
                    first_out.entry(el).or_insert(i);
                } else if let Some(&j) = first_out.get(&el) {
                    fix = Some((i, j));
                    break;
                }
            }
            let Some((i, j)) = fix else { return };
            let f = self.doc.features.remove(i);
            self.doc.features.insert(j, f);
        }
    }

    /// The scene's history: the names of the steps done, oldest first,
    /// then those that can be done again; and how many are done.
    pub fn scene_history(&self) -> (Vec<String>, usize) {
        let mut v: Vec<String> = self.undo.iter().filter(|u| u.doc.is_some()).map(|u| u.label.clone()).collect();
        let done = v.len();
        v.extend(self.redo.iter().rev().filter(|r| r.doc.is_some()).map(|r| r.label.clone()));
        (v, done)
    }

    /// Goes back or forward in the scene's history to `n` steps done (the
    /// document's steps; selection steps are passed through).
    pub fn scene_go(&mut self, n: usize) {
        let done = |s: &Self| s.undo.iter().filter(|u| u.doc.is_some()).count();
        let before = done(self);
        while done(self) > n && !self.undo.is_empty() {
            self.undo();
        }
        while done(self) < n && self.redo.iter().any(|r| r.doc.is_some()) {
            self.redo();
        }
        if done(self) != before {
            let (names, at) = self.scene_history();
            let what = if at == 0 { "the start".to_string() } else { names[at - 1].clone() };
            self.toast(&format!("History: {what} ({at} of {})", names.len()), false);
        }
    }
}
