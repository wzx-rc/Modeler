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
use glam::{DMat4, DVec3};
use nurbs_kernel::{Doc, Feature, FeatureId, Object, Op, Sim};

/// Whole Objects dragged as one (each as it was, with every feature of
/// its): a move, turn or scale the same along every axis carries them all
/// and the Object's frame (`Feature::similar_by`: the kernel finds the same
/// history built and only puts it in its new place). Any other scale (a
/// side of the box dragged): shapes alone that take it exactly change their
/// sizes; else a scale step (`Feature.stretch`, "Scale") at the end of the
/// Object's history takes what is built before it, as a mesh's scale, and
/// what is made after it is made as it is (Romain 24 nov., 13 h 09: "Si je
/// rajoute un cylindre APRES il prend le scale value coef"). Scaled again
/// just after one, that step is the one scaled. `legacy` (the Scale tool's
/// Legacy option, his same message: "ça scale en essayant de maintenir les
/// formes… un fillet ne va pas se modifier"): every scale, the same along
/// every axis too, put into each feature (`Feature::affine_by`): shapes
/// stay shapes of their kind, operations keep their sizes, an extrusion
/// grows along its face (`grow`, read on the model when the drag began).
pub fn put_objects(doc: &mut Doc, objs: &[(Object, Vec<Feature>)], sim: Option<Sim>, aff: Option<DMat4>, legacy: Option<&[(FeatureId, DVec3)]>) {
    for (o0, fs) in objs {
        if let (Some(grow), Some(a)) = (legacy, aff.or(sim.map(|t| t.matrix()))) {
            // every feature as it was taken through the map, kept a shape
            for f0 in fs {
                if let Some(f) = doc.feature_mut(f0.id) {
                    let mut g = f0.affine_by(a);
                    if let (Some(p), Some((_, n))) = (g.push.as_mut(), grow.iter().find(|(id, _)| *id == f0.id)) {
                        p.depth *= grow_along(a, *n);
                    }
                    *f = g;
                }
            }
            // (a step this drag made before Legacy went on: taken away)
            drop_new_steps(doc, o0.id, fs);
            continue;
        }
        if let Some(t) = sim {
            for f0 in fs {
                if let Some(f) = doc.feature_mut(f0.id) {
                    *f = f0.similar_by(&t);
                }
            }
            if let Some(o) = doc.object_mut(o0.id) {
                o.frame = t.after(&o0.frame);
            }
            drop_new_steps(doc, o0.id, fs);
        } else if let Some(a) = aff {
            // shapes alone that take the scale exactly (a cube, a sketch, a
            // cylinder along it; Romain 24 nov., 12 h 24: a simple Cube
            // scaled broke every tool after it): their sizes change
            if fs.iter().all(|f| f.is_solid() && f.absorbs(a)) {
                for f0 in fs {
                    if let Some(f) = doc.feature_mut(f0.id) {
                        *f = f0.affine_by(a);
                    }
                }
                drop_new_steps(doc, o0.id, fs);
                continue;
            }
            for f0 in fs {
                if let Some(f) = doc.feature_mut(f0.id) {
                    *f = f0.clone();
                }
            }
            // the step this goes on with: the Object's last feature when the
            // drag began, else one this drag made, else a new one
            let last0 = fs.iter().filter(|f| doc.features.iter().any(|g| g.id == f.id)).max_by_key(|f| doc.features.iter().position(|g| g.id == f.id)).filter(|f| f.stretch.is_some());
            match last0 {
                Some(f0) => {
                    let m = a * f0.stretch.as_ref().unwrap().matrix();
                    if let Some(st) = doc.feature_mut(f0.id).and_then(|f| f.stretch.as_mut()) {
                        st.rows = nurbs_kernel::mat_to_rows(m);
                    }
                }
                None => match new_step(doc, o0.id, fs) {
                    Some(id) => {
                        if let Some(st) = doc.feature_mut(id).and_then(|f| f.stretch.as_mut()) {
                            st.rows = nurbs_kernel::mat_to_rows(a);
                        }
                    }
                    None => {
                        let at = a.transform_point3(DVec3::ZERO);
                        doc.add_stretch(o0.id, a, at);
                    }
                },
            }
        }
    }
}

/// How much an extrusion along `n` grows under `a`: the pushed vector's
/// part along the face's new normal.
pub fn grow_along(a: DMat4, n: DVec3) -> f64 {
    let n2 = a.inverse().transpose().transform_vector3(n).try_normalize();
    let k = n2.map_or(1.0, |n2| a.transform_vector3(n).dot(n2));
    if k.is_finite() && k > 1e-6 { k } else { 1.0 }
}

/// A scale step of this Object that was not there when the drag began.
fn new_step(doc: &Doc, o: FeatureId, fs: &[Feature]) -> Option<FeatureId> {
    doc.features.iter().find(|f| f.stretch.as_ref().is_some_and(|s| s.shapes.iter().any(|m| fs.iter().any(|g| g.id == *m))) && !fs.iter().any(|g| g.id == f.id)).map(|f| f.id).filter(|_| doc.object(o).is_some())
}

fn drop_new_steps(doc: &mut Doc, o: FeatureId, fs: &[Feature]) {
    while let Some(id) = new_step(doc, o, fs) {
        doc.features.retain(|f| f.id != id);
    }
}

/// A scale along a frame's axes about a point, as a matrix.
pub fn scale_about(p: DVec3, frame: glam::DQuat, k: DVec3) -> DMat4 {
    DMat4::from_translation(p) * DMat4::from_quat(frame) * DMat4::from_scale(k) * DMat4::from_quat(frame.inverse()) * DMat4::from_translation(-p)
}

impl AppState {
    /// The whole Objects among these shapes (every shape of theirs, not the
    /// Object entered), each as it is with every feature of its.
    pub fn whole_objects_of(&self, ids: &[FeatureId]) -> Vec<(Object, Vec<Feature>)> {
        self.doc
            .objects
            .iter()
            .filter(|o| self.open_object != Some(o.id) && !o.members.is_empty() && o.members.iter().all(|m| ids.contains(m)))
            .map(|o| (o.clone(), self.doc.object_features(o.id).into_iter().filter_map(|id| self.doc.feature(id).cloned()).collect()))
            .collect()
    }

    /// Where the eye sees a feature of a scaled Object: the scale steps
    /// made after it in its Object (`Doc::seen_map`), inverted: what the
    /// eye sees taken back to where the feature keeps its points and sizes
    /// (an operation made before a step). None when no step comes after it.
    pub fn unstretch(&self, id: FeatureId) -> Option<DMat4> {
        self.doc.seen_map(id).map(|m| m.inverse())
    }
    pub fn local_vec(&self, id: FeatureId, v: DVec3) -> DVec3 {
        self.unstretch(id).map_or(v, |m| m.transform_vector3(v))
    }
    pub fn local_pt(&self, id: FeatureId, p: DVec3) -> DVec3 {
        self.unstretch(id).map_or(p, |m| m.transform_point3(p))
    }
    /// A length seen along `dir` as it is kept.
    pub fn local_len(&self, id: FeatureId, dir: DVec3, len: f64) -> f64 {
        self.unstretch(id).and_then(|m| dir.try_normalize().map(|d| m.transform_vector3(d).length())).map_or(len, |k| len * k)
    }
    /// A kept point of an operation where the eye sees it.
    pub fn seen_pt(&self, id: FeatureId, p: DVec3) -> DVec3 {
        self.doc.seen_map(id).map_or(p, |m| m.transform_point3(p))
    }

    /// The scale steps made after a feature (a shape: where the eye sees it).
    pub fn stretch_of(&self, id: FeatureId) -> Option<DMat4> {
        self.doc.seen_map(id)
    }

    /// A shape's centre and corners where the eye sees them (through the
    /// scale steps after it).
    pub fn seen_centre(&self, f: &Feature) -> DVec3 {
        let c = f.centre();
        self.stretch_of(f.id).map_or(c, |s| s.transform_point3(c))
    }
    pub fn seen_corners(&self, f: &Feature) -> [DVec3; 8] {
        let c = f.corners();
        match self.stretch_of(f.id) {
            Some(s) => c.map(|p| s.transform_point3(p)),
            None => c,
        }
    }

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

impl AppState {
    /// The Objects Freeze Transform works on: those of what is picked (its
    /// shapes, faces, edges; the Object entered too), else the selected.
    pub fn freeze_targets(&self) -> Vec<FeatureId> {
        let mut out: Vec<FeatureId> = vec![];
        for el in self.picked_elements().into_iter().chain(self.selection.iter().copied()) {
            let el = self.element_of(el);
            if let Some(o) = self.doc.object_of(el) {
                if !out.contains(&o) {
                    out.push(o);
                }
            }
        }
        out
    }

    /// An Object's own transform as the eye sees it: its frame, then its
    /// last scale step when that is its last feature (the step a Scale
    /// drag changes: the drag preview moves the model on screen by it).
    pub fn object_seen(&self, o: &Object) -> DMat4 {
        let last = self.doc.last_stretch(o.id).and_then(|id| self.doc.feature(id)).and_then(|f| f.stretch.as_ref()).filter(|_| self.doc.features.iter().any(|f| f.stretch.is_some()));
        last.map_or(DMat4::IDENTITY, |st| st.matrix()) * o.frame.matrix()
    }

    /// An extrusion's face normal as the step keeps it (before the scale
    /// steps after it), read on the model where the eye sees it.
    pub fn push_kept_normal(&self, id: FeatureId) -> Option<DVec3> {
        let p = self.doc.feature(id)?.push.clone()?;
        let m = self.doc.seen_map(id);
        let seen = m.map_or(p.face.at, |m| m.transform_point3(p.face.at));
        let face = self.model_face_of(&nurbs_kernel::FaceRef { tag: p.face.tag, at: seen }).or_else(|| self.model_face_of(&nurbs_kernel::FaceRef { tag: nurbs_kernel::FaceTag { feature: id, sub: 1 }.pack(), at: seen }));
        let nw = face.and_then(|fi| self.normal_near(fi, seen))?;
        match m {
            Some(m) => m.transpose().transform_vector3(nw).try_normalize(),
            None => Some(nw),
        }
    }

    /// Freeze Transform (Maya's; Romain 24 nov., 12 h 24: "On dirait il
    /// manque un FreezeTransform"): each Object's scale steps put into its
    /// features (`Doc::freeze_object`), so it is made of shapes of their own
    /// kind again; its frame made the identity. An extrusion keeps how far
    /// it goes as seen (its depth grown by the scale across its face, read
    /// on the model); fillets, insets and walls keep their size (made round
    /// again). One undo step.
    pub fn freeze_transform(&mut self) {
        let objs: Vec<FeatureId> = self.freeze_targets().into_iter().filter(|o| self.doc.is_stretched(*o)).collect();
        if objs.is_empty() {
            self.toast("Freeze Transform: pick an Object with a Scale step first", false);
            return;
        }
        // each extrusion's face normal as it is kept
        let mut normals: std::collections::HashMap<FeatureId, DVec3> = std::collections::HashMap::new();
        for o in &objs {
            for id in self.doc.object_features(*o) {
                if let Some(n) = self.push_kept_normal(id) {
                    normals.insert(id, n);
                }
            }
        }
        let mut done: Vec<String> = vec![];
        let mut refused: Option<String> = None;
        let mut doc = self.doc.clone();
        for o in &objs {
            let name = doc.object(*o).map(|ob| ob.name.clone()).unwrap_or_default();
            match doc.freeze_object(*o, &|f: &nurbs_kernel::Feature, s: DMat4| normals.get(&f.id).map_or(1.0, |n| grow_along(s, *n))) {
                Ok(()) => done.push(name),
                Err(e) => refused = Some(format!("{name}: {e}")),
            }
        }
        if done.is_empty() {
            self.toast(&format!("Freeze Transform: {}", refused.unwrap_or_else(|| "nothing to freeze".into())), true);
            return;
        }
        self.edit("Freeze Transform", |d| *d = doc);
        match refused {
            Some(r) => self.toast(&format!("Frozen: {} · not {r}", done.join(", ")), true),
            None => self.toast(&format!("Frozen: {} (its Scale steps put into its shapes)", done.join(", ")), false),
        }
    }
}
