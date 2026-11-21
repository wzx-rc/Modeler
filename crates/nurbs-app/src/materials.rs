//! The six materials (Romain, 17 nov. 2026: six small balls, no names in the
//! way). A feature stores the index of its material. Each ball's colour,
//! specular and transparency can be changed (Romain, 20 nov., 15 h 42); the
//! document keeps what was changed (`Doc.looks`).

use nurbs_kernel::Doc;

pub struct Material {
    pub name: &'static str,
    /// sRGB 0..1.
    pub color: [f32; 3],
    pub metal: f32,
    pub rough: f32,
}

pub const MATERIALS: [Material; 6] = [
    Material { name: "Grey clay", color: [0.50, 0.50, 0.49], metal: 0.0, rough: 0.55 },
    Material { name: "Green", color: [0.34, 0.39, 0.24], metal: 0.0, rough: 0.45 },
    Material { name: "Orange", color: [0.86, 0.42, 0.14], metal: 0.0, rough: 0.4 },
    Material { name: "Iron", color: [0.16, 0.17, 0.19], metal: 0.7, rough: 0.35 },
    Material { name: "Steel", color: [0.56, 0.57, 0.6], metal: 1.0, rough: 0.3 },
    Material { name: "Rubber", color: [0.08, 0.08, 0.09], metal: 0.0, rough: 0.8 },
];

pub fn get(i: u32) -> &'static Material {
    &MATERIALS[(i as usize).min(MATERIALS.len() - 1)]
}

/// The specular a ball comes with (the highlights as they were).
pub const SPECULAR: f32 = 0.5;

/// A material as it is drawn: the ball's settings with the document's
/// changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// sRGB 0..1.
    pub color: [f32; 3],
    pub metal: f32,
    pub rough: f32,
    /// 0..1, 0.5 as the ball comes.
    pub specular: f32,
    /// 0 opaque .. 1 clear.
    pub transparency: f32,
}

impl Look {
    pub fn opacity(&self) -> f32 {
        1.0 - self.transparency
    }
}

impl Material {
    /// The ball as it comes.
    pub fn look(&self) -> Look {
        Look { color: self.color, metal: self.metal, rough: self.rough, specular: SPECULAR, transparency: 0.0 }
    }
}

/// Material `i` as the document has it.
pub fn look(doc: &Doc, i: u32) -> Look {
    let m = get(i);
    let mut l = m.look();
    if let Some(x) = doc.looks.iter().find(|x| x.material == i) {
        l.color = x.color;
        l.specular = x.specular;
        l.transparency = x.transparency;
    }
    l
}

/// Whether material `i` differs from how it comes.
pub fn changed(doc: &Doc, i: u32) -> bool {
    look(doc, i) != get(i).look()
}

/// Sets material `i`'s look in the document (the entry goes when it is as
/// the ball comes).
pub fn set_look(doc: &mut Doc, i: u32, color: [f32; 3], specular: f32, transparency: f32) {
    doc.looks.retain(|x| x.material != i);
    let base = get(i).look();
    let same = color.iter().zip(base.color.iter()).all(|(a, b)| (a - b).abs() < 1e-4) && (specular - base.specular).abs() < 1e-4 && transparency.abs() < 1e-4;
    if !same {
        doc.looks.push(nurbs_kernel::MaterialLook { material: i, color, specular, transparency });
        doc.looks.sort_by_key(|x| x.material);
    }
}

use crate::state::AppState;

impl AppState {
    /// Material `i` as the document has it.
    pub fn look(&self, i: u32) -> Look {
        look(&self.doc, i)
    }

    /// Changes a ball's look. The document changes but the kernel is not
    /// asked (it does not use it), so the part is not built again; the
    /// faces take it at the next frame. One undo step when the gesture
    /// ends (`end_look`).
    pub fn edit_look(&mut self, i: u32, label: &str, f: impl FnOnce(&mut Look)) {
        let old = self.look(i);
        let mut l = old;
        f(&mut l);
        l.specular = l.specular.clamp(0.0, 1.0);
        l.transparency = l.transparency.clamp(0.0, 1.0);
        if l == old {
            return;
        }
        if self.look_live.is_none() {
            self.look_live = Some((label.to_string(), self.doc.clone(), self.sel_state()));
        }
        set_look(&mut self.doc, i, l.color, l.specular, l.transparency);
        self.looks_rev += 1;
    }

    /// The look's gesture ended: one undo step if it changed anything.
    pub fn end_look(&mut self) {
        if let Some((label, before, sel)) = self.look_live.take() {
            if before != self.doc {
                self.push_undo(&label, before, sel);
                self.record("edit", &label);
            }
        }
    }

    /// Material `i` back as it comes (one undo step).
    pub fn reset_look(&mut self, i: u32) {
        self.end_look();
        let base = get(i).look();
        self.edit_look(i, "Material reset", |l| *l = base);
        self.end_look();
    }
}
