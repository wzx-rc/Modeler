//! Objects (Romain, 24 nov. 2026, 6 h 01: "Il faut trouver un moyen de
//! nommer un UBER Object afin de hiérarchiser… je voudrais que le Cube
//! soustraie le gros pavé vert mais pas le cylindre"). An Object is shapes
//! whose booleans act on each other (Add, Sub, Intersect in their order)
//! with their operations: one result, one name, one material. A boolean
//! acts only within its Object (the kernel builds each on its own,
//! `build::build_objects`); Objects side by side stay separate solids.
//! Groups are folders of Objects (his choice, 6 h 04).
//!
//! In the document: `Doc.objects` (`Object { id, name, members }`); every
//! shape is put in one by `Doc::settle_objects` at each change: a new shape
//! goes in the Object it touches (the one last worked on first), else a
//! new Object; an older document is split as its parts are built apart.
//! In the view a click takes the whole Object (as a closed group), a
//! double-click enters it (`open_object`) to pick its shapes one by one.
//! The outliner lists the Objects with their shapes under them; the Sub
//! Outliner shows the selected Object's composition.

use crate::state::{AppState, SelMode};
use nurbs_kernel::{FeatureId, Object, Op};

impl AppState {
    /// These shapes, each with the other shapes of its Object (not the
    /// Object entered).
    pub fn object_ids(&self, ids: &[FeatureId]) -> Vec<FeatureId> {
        let mut out: Vec<FeatureId> = vec![];
        for id in ids {
            let more = match self.doc.object_of(*id) {
                Some(o) if self.open_object != Some(o) => self.doc.object(o).map(|ob| ob.members.clone()).unwrap_or_default(),
                _ => vec![*id],
            };
            for m in more {
                if !out.contains(&m) {
                    out.push(m);
                }
            }
        }
        out
    }

    /// The Object selected: all its shapes, and only them (Object mode).
    pub fn selected_object(&self) -> Option<&Object> {
        if self.sel_mode != SelMode::Object || self.selection.is_empty() {
            return None;
        }
        let o = self.doc.object_of(self.selection[0])?;
        let ob = self.doc.object(o)?;
        (ob.members.len() == self.selection.len() && ob.members.iter().all(|m| self.selection.contains(m))).then_some(ob)
    }

    /// The Object the outliner's Sub Outliner shows: the selected one, else
    /// that of the element worked on.
    pub fn outline_object(&self) -> Option<FeatureId> {
        if let Some(o) = self.selected_object() {
            return Some(o.id);
        }
        let el = self.selection.first().map(|id| self.element_of(*id)).or(self.outline_element())?;
        self.doc.object_of(el)
    }

    /// Selects an Object whole (its row, its shapes all).
    pub fn select_object(&mut self, id: FeatureId, add: bool) {
        let Some(ob) = self.doc.object(id) else { return };
        let ids = ob.members.clone();
        self.set_sel_mode(SelMode::Object);
        if self.open_object == Some(id) {
            self.open_object = None;
        }
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
        } else {
            self.selection = ids;
        }
        self.group_sel = None;
        self.last_object = Some(id);
        self.edge_sel.clear();
        self.face_sel.clear();
        self.vert_sel.clear();
        self.renaming = false;
    }

    /// A double-click on a shape (the view), or its row: its Object entered,
    /// that shape alone picked.
    pub fn enter_object(&mut self, shape: FeatureId) {
        let el = self.element_of(shape);
        let Some(o) = self.doc.object_of(el) else {
            self.select(Some(el), false);
            return;
        };
        self.set_sel_mode(SelMode::Object);
        let new = self.open_object != Some(o);
        self.open_object = Some(o);
        self.last_object = Some(o);
        self.selection = vec![el];
        self.group_sel = None;
        if new {
            let name = self.doc.object(o).map(|ob| ob.name.clone()).unwrap_or_default();
            self.toast(&format!("In {name}: its shapes one by one · Esc: the whole Object"), false);
        }
    }

    /// Leaves the Object entered, its shapes picked whole again.
    pub fn leave_object(&mut self) -> bool {
        let Some(o) = self.open_object.take() else { return false };
        if self.selection.iter().any(|id| self.doc.object_of(self.element_of(*id)) == Some(o)) {
            self.select_object(o, false);
        }
        true
    }

    /// These shapes put in Object `to`, or in a new Object of their own
    /// (None). One undo step.
    pub fn shapes_to_object(&mut self, shapes: &[FeatureId], to: Option<FeatureId>) {
        let ids: Vec<FeatureId> = shapes.iter().map(|id| self.element_of(*id)).collect();
        let mut made = None;
        self.edit(if to.is_some() { "Move to Object" } else { "New Object" }, |d| {
            made = d.move_to_object(&ids, to);
        });
        if let Some(o) = made {
            let name = self.doc.object(o).map(|ob| ob.name.clone()).unwrap_or_default();
            self.last_object = Some(o);
            if self.open_object.is_some_and(|x| self.doc.object(x).is_none()) {
                self.open_object = None;
            }
            self.toast(&format!("In {name} now: its booleans act only there"), false);
        }
    }

    pub fn rename_object(&mut self, id: FeatureId, name: &str) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        self.edit_quiet("Rename", |d| {
            if let Some(o) = d.object_mut(id) {
                o.name = name.clone();
            }
        });
    }

    /// Shows or hides all of an Object's shapes.
    pub fn toggle_object_visible(&mut self, id: FeatureId) {
        let Some(ob) = self.doc.object(id) else { return };
        let ms = ob.members.clone();
        let shown = ms.iter().any(|m| self.doc.feature(*m).is_some_and(|f| f.visible));
        self.edit(if shown { "Hide" } else { "Show" }, |d| {
            for m in &ms {
                if let Some(f) = d.feature_mut(*m) {
                    f.visible = !shown;
                }
            }
        });
    }

    /// Add, Sub, Intersect, then Add again (the glyph in a shape's row).
    pub fn cycle_op(&mut self, id: FeatureId) {
        let Some(op) = self.doc.feature(id).map(|f| f.op) else { return };
        let next = match op {
            Op::Add => Op::Carve,
            Op::Carve => Op::Intersect,
            Op::Intersect => Op::Add,
        };
        self.set_ops(&[id], next);
    }
}
