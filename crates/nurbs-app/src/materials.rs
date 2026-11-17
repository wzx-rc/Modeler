//! The six materials (Romain, 17 nov. 2026: six small balls, no names in the
//! way). A feature stores the index of its material.

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
