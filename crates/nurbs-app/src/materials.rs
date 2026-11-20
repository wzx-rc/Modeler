//! The six materials (Romain, 17 nov. 2026: six small balls, no names in the
//! way). A feature stores the index of its material; an SDF shape its index
//! + 1 (0: its object's).
//!
//! Their shader parameters (colour, metal, roughness) are the document's
//! (`Doc.materials`, set in the materials island's editor, the same for every
//! tab); a material the document does not set keeps its default here.

use nurbs_kernel::{Doc, MatDef};

/// How many materials there are (the balls).
pub const COUNT: usize = 6;

#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    /// sRGB 0..1.
    pub color: [f32; 3],
    pub metal: f32,
    pub rough: f32,
}

struct Default_ {
    name: &'static str,
    color: [f32; 3],
    metal: f32,
    rough: f32,
}

const DEFAULTS: [Default_; COUNT] = [
    Default_ { name: "Grey clay", color: [0.50, 0.50, 0.49], metal: 0.0, rough: 0.55 },
    Default_ { name: "Green", color: [0.34, 0.39, 0.24], metal: 0.0, rough: 0.45 },
    Default_ { name: "Orange", color: [0.86, 0.42, 0.14], metal: 0.0, rough: 0.4 },
    Default_ { name: "Iron", color: [0.16, 0.17, 0.19], metal: 0.7, rough: 0.35 },
    Default_ { name: "Steel", color: [0.56, 0.57, 0.6], metal: 1.0, rough: 0.3 },
    Default_ { name: "Rubber", color: [0.08, 0.08, 0.09], metal: 0.0, rough: 0.8 },
];

/// Material `i` as shipped.
pub fn default_of(i: u32) -> Material {
    let d = &DEFAULTS[(i as usize).min(COUNT - 1)];
    Material { name: d.name.to_string(), color: d.color, metal: d.metal, rough: d.rough }
}

/// Material `i` as the document sets it.
pub fn get(doc: &Doc, i: u32) -> Material {
    let i = (i as usize).min(COUNT - 1);
    match doc.materials.get(i) {
        Some(m) => Material { name: m.name.clone(), color: m.color, metal: m.metal, rough: m.rough },
        None => default_of(i as u32),
    }
}

/// All of them, as the document sets them.
pub fn all(doc: &Doc) -> Vec<Material> {
    (0..COUNT as u32).map(|i| get(doc, i)).collect()
}

/// Whether material `i` differs from its default.
pub fn edited(doc: &Doc, i: u32) -> bool {
    get(doc, i) != default_of(i)
}

/// Sets material `i` in a document (the others before it written as they
/// are, so the list holds by index).
pub fn set(doc: &mut Doc, i: u32, m: &Material) {
    let i = (i as usize).min(COUNT - 1);
    while doc.materials.len() <= i {
        let k = doc.materials.len() as u32;
        let d = default_of(k);
        doc.materials.push(MatDef { name: d.name, color: d.color, metal: d.metal, rough: d.rough });
    }
    doc.materials[i] = MatDef { name: m.name.clone(), color: m.color, metal: m.metal, rough: m.rough };
    // nothing left but defaults: the list emptied (the file stays as before)
    if (0..doc.materials.len() as u32).all(|k| !edited(doc, k)) {
        doc.materials.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_edits() {
        let mut d = Doc::default();
        assert_eq!(get(&d, 2).name, "Orange");
        assert_eq!(get(&d, 99).name, "Rubber", "past the end: the last one");
        let mut m = get(&d, 3);
        m.color = [0.9, 0.1, 0.1];
        set(&mut d, 3, &m);
        assert_eq!(d.materials.len(), 4, "the ones before it written as they are");
        assert_eq!(get(&d, 3).color, [0.9, 0.1, 0.1]);
        assert!(edited(&d, 3) && !edited(&d, 1));
        // back to the default: the list emptied
        set(&mut d, 3, &default_of(3));
        assert!(d.materials.is_empty());
    }
}
