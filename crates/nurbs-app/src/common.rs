//! The base the tabs share (Romain, 21 nov. 2026: "tout feature commune
//! (shader parameter, outliner…) tout un par un qui est commun doit marcher
//! sur une base commune").
//!
//! An **element** is a feature that stands on its own in the scene: a NURBS
//! shape or curve, a surface, a Sculpt or Poly mesh, an SDF object. Its
//! **items** are what it is made of, in order, each with a name, a kind, a
//! state (on / off) and a place in the element's history:
//! - a NURBS shape: its operations (fillets, insets, moves, cuts), the
//!   features that name its faces (`owner_of`);
//! - an SDF object: its shapes (Add / Carve / Intersect, each blended).
//!
//! The outliner, the Sub Outliner, the history strip and its marker, the
//! operation window's place, F (frame), the right-click menus and the
//! marking menu go through `Item`, so a feature added to one kind of element
//! works the same for the others.

use crate::state::AppState;
use crate::workspace::Workspace;
use forme_core::NodeId;
use glam::{DVec3, Vec3};
use nurbs_kernel::mesh::MeshKind;
use nurbs_kernel::{Feature, FeatureId, Prim};

/// One thing in the scene's lists: a feature (an element, or an operation
/// on one), or a shape inside an SDF object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Feature(FeatureId),
    Shape(FeatureId, NodeId),
}

impl Item {
    /// The feature it is, or is in.
    pub fn feature(self) -> FeatureId {
        match self {
            Item::Feature(id) | Item::Shape(id, _) => id,
        }
    }
}

/// The outliner's groups, by kind of element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Primitive,
    /// A Forms tool's surface pulled with brushes (a NURBS solid).
    Form,
    Shape,
    Curve,
    Surface,
    Sculpt,
    Poly,
    Sdf,
}

impl Kind {
    /// In the outliner's order (the operations whose element cannot be told
    /// go between Surfaces and Sculpt, as before).
    pub const ALL: [Kind; 8] = [Kind::Primitive, Kind::Form, Kind::Shape, Kind::Curve, Kind::Surface, Kind::Sculpt, Kind::Poly, Kind::Sdf];

    pub fn title(self) -> &'static str {
        match self {
            Kind::Primitive => "PRIMITIVES",
            Kind::Form => "FORMS",
            Kind::Shape => "SHAPES",
            Kind::Curve => "CURVES",
            Kind::Surface => "SURFACES",
            Kind::Sculpt => "SCULPT",
            Kind::Poly => "POLY",
            Kind::Sdf => "SDF",
        }
    }

    /// The tab its elements are worked in.
    pub fn workspace(self) -> Workspace {
        match self {
            Kind::Sculpt => Workspace::Sculpt,
            Kind::Poly => Workspace::Poly,
            Kind::Sdf => Workspace::Sdf,
            _ => Workspace::Forme,
        }
    }

    /// What its items are called.
    pub fn items_word(self) -> (&'static str, &'static str) {
        match self {
            Kind::Sdf => ("shape", "shapes"),
            _ => ("op", "ops"),
        }
    }
}

/// The group an element is listed in (None: an operation, or a curve
/// inside a 2D boolean, listed under something else).
pub fn kind_of(f: &Feature) -> Option<Kind> {
    if let Some(m) = &f.mesh {
        // (a linked surface is listed with the Poly meshes, as before)
        return Some(match m.kind {
            MeshKind::Sculpt => Kind::Sculpt,
            MeshKind::Poly => Kind::Poly,
            MeshKind::Sdf => Kind::Sdf,
        });
    }
    // a curve filled, turned or lofted is a surface (or a solid), not a curve
    if f.is_sheet() || f.fill || f.revolve.is_some() || f.loft.is_some() {
        return Some(Kind::Surface);
    }
    if f.is_path() {
        return Some(Kind::Curve);
    }
    if f.form.is_some() {
        return Some(Kind::Form);
    }
    if !f.is_solid() {
        return None;
    }
    Some(match f.prim {
        Prim::Profile { .. } => Kind::Shape,
        _ => Kind::Primitive,
    })
}

impl AppState {
    /// An element's items, in its history's order: a NURBS shape's
    /// operations, an SDF object's shapes.
    pub fn element_items(&self, el: FeatureId) -> Vec<Item> {
        match self.doc.feature(el) {
            Some(f) if crate::sdf::is_sdf(f) => crate::sdf::model_of(f).map(|m| m.shapes().into_iter().map(|n| Item::Shape(el, n)).collect()).unwrap_or_default(),
            Some(_) => self.element_ops(el).into_iter().map(Item::Feature).collect(),
            None => vec![],
        }
    }

    /// The element an item belongs to (an operation's shape, a shape's SDF
    /// object; an element: itself).
    pub fn item_element(&self, it: Item) -> Option<FeatureId> {
        match it {
            Item::Shape(id, _) => Some(id),
            Item::Feature(id) => {
                let f = self.doc.feature(id)?;
                if f.is_solid() || f.is_path() {
                    Some(id)
                } else {
                    self.owner_of(id)
                }
            }
        }
    }

    /// Its name.
    pub fn item_name(&self, it: Item) -> String {
        match it {
            Item::Feature(id) => self.doc.feature(id).map(|f| f.name.clone()).unwrap_or_default(),
            Item::Shape(id, n) => self.doc.feature(id).and_then(crate::sdf::scene_of).and_then(|s| s.node(n)).map(|x| x.name.clone()).unwrap_or_default(),
        }
    }

    /// What it is ("Fillet", "Sphere"…) and its main values.
    pub fn item_info(&self, it: Item) -> (String, String) {
        match it {
            Item::Feature(id) => match self.doc.feature(id) {
                Some(f) => (f.kind_label().to_string(), crate::ui::step_info(f)),
                None => (String::new(), String::new()),
            },
            Item::Shape(id, n) => {
                let Some(nd) = self.doc.feature(id).and_then(crate::sdf::scene_of).and_then(|s| s.node(n)) else { return (String::new(), String::new()) };
                let kind = nd.shape().map(|s| crate::sdf::prim_label(s.kind)).unwrap_or("Shape").to_string();
                let info = if nd.combine.blend > 0.0 { format!("{} · {}", nd.combine.mode.label(), crate::widgets::fmt_mm(nd.combine.blend)) } else { nd.combine.mode.label().to_string() };
                (kind, info)
            }
        }
    }

    /// Whether it is on (an operation or a shape turned off by hand is
    /// left out of the build, the ones after it kept).
    pub fn item_on(&self, it: Item) -> bool {
        match it {
            Item::Feature(id) => self.doc.feature(id).map(|f| f.visible).unwrap_or(false),
            Item::Shape(id, n) => self.doc.feature(id).and_then(crate::sdf::scene_of).and_then(|s| s.node(n)).map(|x| x.visible).unwrap_or(false),
        }
    }

    /// Turns it off or on (one undo step).
    pub fn toggle_item(&mut self, it: Item) {
        match it {
            Item::Feature(id) => self.toggle_op(id),
            Item::Shape(id, n) => {
                let on = self.item_on(it);
                let name = self.item_name(it);
                self.edit(if on { "Turn off" } else { "Turn on" }, |d| {
                    if let Some(x) = sdf_node_mut(d, id, n) {
                        x.visible = !on;
                    }
                });
                self.toast(&format!("{name} {}", if on { "off" } else { "on" }), false);
            }
        }
    }

    /// Whether it is the one picked (a feature selected, a shape picked).
    pub fn item_selected(&self, it: Item) -> bool {
        match it {
            Item::Feature(id) => self.is_selected(id),
            Item::Shape(id, n) => self.sdf_sel == Some((id, n)),
        }
    }

    /// Picks it: a feature selected (as a click on its row), a shape picked
    /// in its object (the SDF tab, where its gizmo works).
    pub fn select_item(&mut self, it: Item) {
        match it {
            Item::Feature(id) => {
                self.select_tool();
                self.select(Some(id), false);
            }
            Item::Shape(id, n) => {
                if self.workspace != Workspace::Sdf {
                    self.set_workspace(Workspace::Sdf);
                }
                self.sdf_sel = Some((id, n));
                self.selection = vec![id];
            }
        }
    }

    /// Deletes it (an SDF object's last shape: the object).
    pub fn delete_item(&mut self, it: Item) {
        match it {
            Item::Feature(id) => {
                self.select(Some(id), false);
                self.delete_selected();
            }
            Item::Shape(id, n) => {
                let left = self.element_items(id).len();
                if left <= 1 {
                    self.select(Some(id), false);
                    self.sdf_sel = None;
                    self.delete_selected();
                    return;
                }
                let name = self.item_name(it);
                self.edit("Delete shape", |d| {
                    if let Some(md) = sdf_model_mut(d, id) {
                        let at = md.shapes().iter().position(|x| *x == n);
                        md.scene.remove(n);
                        // the marker stays after the same shapes
                        if let (Some(m), Some(at)) = (md.marker, at) {
                            if at < m {
                                md.marker = Some(m - 1);
                            }
                        }
                    }
                });
                if self.sdf_sel == Some((id, n)) {
                    self.sdf_sel = None;
                }
                self.toast(&format!("{name} deleted"), false);
            }
        }
    }

    /// Where the element's history marker is: how many of its items are
    /// built (those before it).
    pub fn item_marker(&self, el: FeatureId) -> usize {
        match self.doc.feature(el) {
            Some(f) if crate::sdf::is_sdf(f) => {
                let n = self.element_items(el).len();
                crate::sdf::model_of(f).and_then(|m| m.marker).unwrap_or(n).min(n)
            }
            _ => self.marker_of(el),
        }
    }

    /// Moves the element's marker: its first `n` items built, the others
    /// kept past it (live while the strip is scrubbed, else one undo step).
    pub fn set_item_marker(&mut self, el: FeatureId, n: usize) {
        let sdf = self.doc.feature(el).map(crate::sdf::is_sdf).unwrap_or(false);
        if !sdf {
            self.set_marker(el, n);
            return;
        }
        let total = self.element_items(el).len();
        let m = (n < total).then_some(n);
        let apply = move |d: &mut nurbs_kernel::Doc| {
            if let Some(md) = sdf_model_mut(d, el) {
                md.marker = m;
            }
        };
        if self.is_live() {
            self.live(apply);
        } else {
            self.edit("History", apply);
        }
        // a shape past the marker is not picked
        if let Some((id, s)) = self.sdf_sel {
            if id == el && !self.element_items(el).iter().take(n).any(|x| *x == Item::Shape(el, s)) {
                self.sdf_sel = None;
            }
        }
    }

    /// Points of it in the world: what F frames, where its window goes.
    pub fn item_points(&self, it: Item) -> Vec<DVec3> {
        match it {
            Item::Shape(id, n) => {
                let Some(f) = self.doc.feature(id) else { return vec![] };
                let Some(scene) = crate::sdf::scene_of(f) else { return vec![] };
                crate::sdf::node_lines(f, scene, n).into_iter().flatten().map(|p| Vec3::from(p).as_dvec3()).collect()
            }
            Item::Feature(id) => {
                let Some(f) = self.doc.feature(id) else { return vec![] };
                if let Some(r) = &f.round {
                    return r.edges.iter().map(|e| e.at).collect();
                }
                if let Some(t) = &f.tweak {
                    return vec![t.at() + t.delta];
                }
                if f.is_mesh() {
                    return crate::meshes::world_bounds(f).map(|(lo, hi)| vec![lo.as_dvec3(), hi.as_dvec3()]).unwrap_or_default();
                }
                f.corners().to_vec()
            }
        }
    }

    /// The material an item shows (an SDF shape's own, else its object's).
    pub fn item_material(&self, it: Item) -> u32 {
        match it {
            Item::Feature(id) => self.material_of(self.element_of(id)),
            Item::Shape(id, n) => {
                let own = self.doc.feature(id).and_then(crate::sdf::scene_of).and_then(|s| s.node(n)).map(|x| x.material).unwrap_or(0);
                if own > 0 {
                    own - 1
                } else {
                    self.material_of(id)
                }
            }
        }
    }

    /// Gives an SDF shape its own material (one undo step; the object's
    /// other shapes keep theirs).
    pub fn set_shape_material(&mut self, id: FeatureId, n: NodeId, m: u32) {
        let name = crate::materials::get(m).name;
        let shape = self.item_name(Item::Shape(id, n));
        self.edit("Material", |d| {
            if let Some(x) = sdf_node_mut(d, id, n) {
                x.material = m + 1;
            }
        });
        self.toast(&format!("{name} on {shape}"), false);
    }
}

impl AppState {
    /// Renames an item (one undo step).
    pub fn rename_item(&mut self, it: Item, name: &str) {
        let name = name.trim().to_string();
        if name.is_empty() || name == self.item_name(it) {
            return;
        }
        self.edit("Rename", |d| match it {
            Item::Feature(id) => {
                if let Some(f) = d.feature_mut(id) {
                    f.name = name;
                }
            }
            Item::Shape(id, n) => {
                if let Some(x) = sdf_node_mut(d, id, n) {
                    x.name = name;
                }
            }
        });
    }

    /// Moves an item earlier (-1) or later (+1): a feature in the document's
    /// history, an SDF shape among its object's (the marker stays put).
    pub fn reorder_item(&mut self, it: Item, delta: i32) {
        match it {
            Item::Feature(id) => self.reorder(id, delta),
            Item::Shape(id, n) => self.edit("Reorder", |d| {
                let Some(md) = sdf_model_mut(d, id) else { return };
                let Some(parent) = md.scene.parent_of(n) else { return };
                let Some(i) = md.scene.node(parent).and_then(|p| p.children().iter().position(|c| *c == n)) else { return };
                let len = md.scene.node(parent).map(|p| p.children().len()).unwrap_or(0);
                let j = i as i32 + delta;
                if j < 0 || j as usize >= len {
                    return;
                }
                md.scene.reparent(n, parent, j as usize);
            }),
        }
    }

    /// The triangles an element shows (world, mm): a mesh's (Sculpt, Poly,
    /// SDF), or a NURBS element's faces in the kernel's mesh.
    pub fn element_triangles(&self, el: FeatureId) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let Some(f) = self.doc.feature(el) else { return (vec![], vec![]) };
        if let Some(m) = &f.mesh {
            let pts = crate::meshes::world_points(f);
            let mut tris = vec![];
            for q in &m.faces {
                for k in 1..q.len().saturating_sub(1) {
                    tris.push([q[0], q[k], q[k + 1]]);
                }
            }
            return (pts, tris);
        }
        let Some(m) = self.model.as_ref() else { return (vec![], vec![]) };
        let mut map = std::collections::HashMap::new();
        let mut pts = vec![];
        let mut tris = vec![];
        for (t, tri) in m.triangles.iter().enumerate() {
            let Some(tag) = m.faces.get(m.tri_face[t] as usize).and_then(|x| x.tag) else { continue };
            if self.element_of(tag.feature) != el {
                continue;
            }
            let mut out = [0u32; 3];
            for (k, v) in tri.iter().enumerate() {
                out[k] = *map.entry(*v).or_insert_with(|| {
                    pts.push(Vec3::from(m.positions[*v as usize]));
                    pts.len() as u32 - 1
                });
            }
            tris.push(out);
        }
        (pts, tris)
    }

    /// Exports elements as an .obj (one object each, millimetres, Y up) to
    /// Documents/Forme NURBS: the same for every kind (a NURBS shape's
    /// faces as the kernel meshes them, a clay, a Poly mesh, an SDF object).
    pub fn export_elements(&mut self, ids: &[FeatureId]) -> Option<std::path::PathBuf> {
        let mut s = String::from("# Forme: elements as shown (mm, Y up)\n");
        let mut base = 1u32;
        let mut n_tri = 0;
        for id in ids {
            let (pts, tris) = self.element_triangles(*id);
            if tris.is_empty() {
                continue;
            }
            let name = self.doc.feature(*id).map(|f| crate::files::sanitize(&f.name).replace(' ', "_")).unwrap_or_default();
            s.push_str(&format!("o {name}\n"));
            for p in &pts {
                s.push_str(&format!("v {:.5} {:.5} {:.5}\n", p.x, p.y, p.z));
            }
            for t in &tris {
                s.push_str(&format!("f {} {} {}\n", t[0] + base, t[1] + base, t[2] + base));
            }
            base += pts.len() as u32;
            n_tri += tris.len();
        }
        if n_tri == 0 {
            self.toast("Nothing to export: no triangles yet", true);
            return None;
        }
        let what = match ids {
            [one] => self.doc.feature(*one).map(|f| crate::files::sanitize(&f.name)).unwrap_or_default(),
            _ => format!("{} elements", ids.len()),
        };
        let path = crate::files::documents_dir().join(format!("{}-{}.obj", crate::files::sanitize(&self.doc_name), what));
        match std::fs::write(&path, s) {
            Ok(()) => {
                let text = format!("Saved {n_tri} triangles to {}", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                self.toast(&text, false);
                Some(path)
            }
            Err(e) => {
                self.toast(&format!("Export failed: {e}"), true);
                None
            }
        }
    }
}

/// An SDF object's model in a document, to change it.
pub fn sdf_model_mut(d: &mut nurbs_kernel::Doc, id: FeatureId) -> Option<&mut nurbs_kernel::mesh::SdfModel> {
    d.feature_mut(id)?.mesh.as_mut()?.make_mut().sdf.as_mut()
}

/// A shape of an SDF object in a document, to change it.
pub fn sdf_node_mut(d: &mut nurbs_kernel::Doc, id: FeatureId, n: NodeId) -> Option<&mut forme_core::Node> {
    sdf_model_mut(d, id)?.scene.node_mut(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forme_core::PrimKind as K;
    use nurbs_kernel::client::Kernel;
    use nurbs_kernel::Doc;

    /// An app with no kernel (the SDF tab never needs it), an SDF object of
    /// a cube, a sphere and a cylinder (Carve).
    fn three() -> (AppState, FeatureId) {
        let mut st = AppState::new(Doc::default(), "t", Kernel::start(None));
        st.set_workspace(Workspace::Sdf);
        st.sdf_add(K::Box);
        let id = st.sdf_target().expect("the new object");
        st.sdf_add(K::Sphere);
        st.sdf.mode = forme_core::CombineMode::Carve;
        st.sdf_add(K::Cylinder);
        st.sdf_tick();
        (st, id)
    }

    fn tris(st: &AppState, id: FeatureId) -> usize {
        st.doc.feature(id).and_then(|f| f.mesh.as_ref()).map(|m| m.faces.len()).unwrap_or(0)
    }

    #[test]
    fn sdf_objects_are_elements_with_their_shapes_as_items() {
        let (st, id) = three();
        let f = st.doc.feature(id).unwrap();
        assert_eq!(kind_of(f), Some(Kind::Sdf));
        let items = st.element_items(id);
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|it| matches!(it, Item::Shape(x, _) if *x == id)));
        assert_eq!(st.item_info(items[2]).0, "Cylinder");
        assert!(st.item_info(items[2]).1.starts_with("Carve"));
        assert_eq!(st.item_marker(id), 3, "all built");
        assert_eq!(st.item_element(items[1]), Some(id));
        assert!(tris(&st, id) > 100, "meshed: {}", tris(&st, id));
    }

    #[test]
    fn the_marker_leaves_shapes_out_and_new_ones_go_in_at_it() {
        let (mut st, id) = three();
        let full = st.doc.feature(id).map(|f| crate::meshes::size(f)).unwrap();
        // back to the cube alone: the sphere on top is left out
        st.set_item_marker(id, 1);
        st.sdf_tick();
        assert_eq!(st.item_marker(id), 1);
        let cube = st.doc.feature(id).map(|f| crate::meshes::size(f)).unwrap();
        assert!(cube.y < full.y - 5.0, "the object lower without its sphere: {cube:?} vs {full:?}");
        // a shape made meanwhile goes in at the marker, before those left out
        st.sdf.mode = forme_core::CombineMode::Add;
        st.sdf_sel = None;
        st.sdf_add(K::Torus);
        let items = st.element_items(id);
        assert_eq!(items.len(), 4);
        assert_eq!(st.item_marker(id), 2);
        assert_eq!(st.item_info(items[1]).0, "Torus");
        // all back
        st.set_item_marker(id, 4);
        assert_eq!(crate::sdf::model_of(st.doc.feature(id).unwrap()).unwrap().marker, None);
        // undo: the marker as it was
        st.undo();
        assert_eq!(st.item_marker(id), 2);
    }

    #[test]
    fn turned_off_deleted_and_copied_shapes_keep_the_marker() {
        let (mut st, id) = three();
        let items = st.element_items(id);
        // off: left out, kept
        st.toggle_item(items[2]);
        assert!(!st.item_on(items[2]));
        assert_eq!(st.element_items(id).len(), 3);
        st.toggle_item(items[2]);
        assert!(st.item_on(items[2]));
        // the marker after the sphere; the cube deleted: it stays after it
        st.set_item_marker(id, 2);
        st.delete_item(items[0]);
        assert_eq!(st.element_items(id).len(), 2);
        assert_eq!(st.item_marker(id), 1);
        // the sphere copied: the copy is built too
        st.select_item(Item::Shape(id, match st.element_items(id)[0] {
            Item::Shape(_, n) => n,
            _ => unreachable!(),
        }));
        assert!(st.sdf_duplicate());
        assert_eq!(st.element_items(id).len(), 3);
        assert_eq!(st.item_marker(id), 2);
    }

    #[test]
    fn a_shape_wears_its_own_material_on_its_part_of_the_surface() {
        let (mut st, id) = three();
        let items = st.element_items(id);
        let Item::Shape(_, sphere) = items[1] else { unreachable!() };
        st.set_shape_material(id, sphere, 2);
        st.sdf_tick();
        assert_eq!(st.item_material(items[1]), 2);
        assert_eq!(st.item_material(items[0]), 0, "the cube keeps its object's");
        let mats = st.sdf_mats.get(&id).cloned().unwrap_or_default();
        assert_eq!(mats.len(), tris(&st, id));
        let on_sphere = mats.iter().filter(|m| **m == 3).count();
        assert!(on_sphere > 10 && on_sphere < mats.len(), "the sphere's triangles: {on_sphere} of {}", mats.len());
        // the whole object given one: the shapes' own go (as Maya's faces)
        st.selection = vec![id];
        st.sdf_sel = None;
        st.set_workspace(Workspace::Forme);
        st.apply_material(4);
        assert_eq!(st.item_material(items[1]), 4);
        assert_eq!(st.doc.feature(id).unwrap().material, 4);
    }

    #[test]
    fn materials_settings_are_the_documents_and_undo() {
        let (mut st, _) = three();
        // one system for every tab: the balls' looks (nurbs-zero's)
        st.edit_look(1, "Material color", |l| {
            l.color = [0.8, 0.2, 0.2];
            l.transparency = 0.3;
        });
        st.end_look();
        assert_eq!(st.look(1).color, [0.8, 0.2, 0.2]);
        // saved with the document
        let json = serde_json::to_string(&st.doc).unwrap();
        let back: Doc = serde_json::from_str(&json).unwrap();
        assert_eq!(crate::materials::look(&back, 1).transparency, 0.3);
        st.undo();
        assert!(!crate::materials::changed(&st.doc, 1));
    }

    #[test]
    fn nurbs_operations_are_items_too() {
        let mut st = AppState::new(Doc::default(), "t", Kernel::start(None));
        let mut block = 0;
        st.edit("Cube", |d| block = d.add(nurbs_kernel::Op::Add, Prim::Box { x: 20.0, y: 10.0, z: 20.0 }, DVec3::ZERO, glam::DQuat::IDENTITY));
        let f = st.doc.feature(block).unwrap();
        assert_eq!(kind_of(f), Some(Kind::Primitive));
        assert!(st.element_items(block).is_empty());
        assert_eq!(st.item_marker(block), 0);
        assert_eq!(st.item_element(Item::Feature(block)), Some(block));
    }
}