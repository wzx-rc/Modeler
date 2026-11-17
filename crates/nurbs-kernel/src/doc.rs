//! The document: features applied in order. Add = union with what is
//! there, Carve = subtraction. A feature is a primitive standing on its base:
//! `origin` is the centre of its base, it rises along its local +Y (the
//! rotation `rot` turns local into world). Millimetres, Y up.

use glam::{DMat4, DQuat, DVec3};
use serde::{Deserialize, Serialize};

pub type FeatureId = u32;

/// Format version written in the file.
pub const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Add,
    Carve,
}

impl Op {
    pub fn label(self) -> &'static str {
        match self {
            Op::Add => "Add",
            Op::Carve => "Carve",
        }
    }
    pub fn other(self) -> Op {
        match self {
            Op::Add => Op::Carve,
            Op::Carve => Op::Add,
        }
    }
}

/// The primitives (sizes in mm, full sizes).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Prim {
    /// Width (local X), height (local Y), depth (local Z).
    Box { x: f64, y: f64, z: f64 },
    Cylinder { radius: f64, height: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrimKind {
    Box,
    Cylinder,
}

impl PrimKind {
    pub fn label(self) -> &'static str {
        match self {
            PrimKind::Box => "Cube",
            PrimKind::Cylinder => "Cylinder",
        }
    }
    /// Default primitive: 20 mm.
    pub fn default_prim(self) -> Prim {
        match self {
            PrimKind::Box => Prim::Box { x: 20.0, y: 20.0, z: 20.0 },
            PrimKind::Cylinder => Prim::Cylinder { radius: 10.0, height: 20.0 },
        }
    }
}

impl Prim {
    pub fn kind(&self) -> PrimKind {
        match self {
            Prim::Box { .. } => PrimKind::Box,
            Prim::Cylinder { .. } => PrimKind::Cylinder,
        }
    }
    pub fn height(&self) -> f64 {
        match *self {
            Prim::Box { y, .. } => y,
            Prim::Cylinder { height, .. } => height,
        }
    }
    pub fn set_height(&mut self, h: f64) {
        match self {
            Prim::Box { y, .. } => *y = h,
            Prim::Cylinder { height, .. } => *height = h,
        }
    }
    /// Half extents in the local frame, around the base centre: x, height, z.
    pub fn half_xz(&self) -> (f64, f64) {
        match *self {
            Prim::Box { x, z, .. } => (x * 0.5, z * 0.5),
            Prim::Cylinder { radius, .. } => (radius, radius),
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: FeatureId,
    pub name: String,
    pub op: Op,
    pub prim: Prim,
    /// Centre of the base (world).
    pub origin: DVec3,
    /// Local to world rotation (the primitive rises along local +Y).
    pub rot: DQuat,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub material: u32,
}

impl Feature {
    /// Local (base centre at 0, height along +Y) to world.
    pub fn matrix(&self) -> DMat4 {
        DMat4::from_rotation_translation(self.rot, self.origin)
    }
    pub fn axis(&self) -> DVec3 {
        self.rot * DVec3::Y
    }
    /// Centre of the volume (world).
    pub fn centre(&self) -> DVec3 {
        self.origin + self.axis() * self.prim.height() * 0.5
    }
    /// The 8 corners of its box (world).
    pub fn corners(&self) -> [DVec3; 8] {
        let (hx, hz) = self.prim.half_xz();
        let h = self.prim.height();
        let m = self.matrix();
        let mut out = [DVec3::ZERO; 8];
        for (i, c) in out.iter_mut().enumerate() {
            let l = DVec3::new(if i & 1 == 0 { -hx } else { hx }, if i & 2 == 0 { 0.0 } else { h }, if i & 4 == 0 { -hz } else { hz });
            *c = m.transform_point3(l);
        }
        out
    }
    /// World bounds.
    pub fn bounds(&self) -> (DVec3, DVec3) {
        let c = self.corners();
        let lo = c.iter().fold(DVec3::splat(f64::MAX), |a, b| a.min(*b));
        let hi = c.iter().fold(DVec3::splat(f64::MIN), |a, b| a.max(*b));
        (lo, hi)
    }
    /// Its outline as polylines (world): a box's 12 edges, a cylinder's two
    /// rims and four lines between them. Drawn over the part for the
    /// selected cutter and while it is being created.
    pub fn outline(&self) -> Vec<Vec<DVec3>> {
        let m = self.matrix();
        let h = self.prim.height();
        match self.prim {
            Prim::Box { .. } => {
                let c = self.corners();
                let pairs = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
                pairs.iter().map(|(a, b)| vec![c[*a], c[*b]]).collect()
            }
            Prim::Cylinder { radius, .. } => {
                let n = 64;
                let ring = |y: f64| -> Vec<DVec3> {
                    (0..=n)
                        .map(|i| {
                            let a = std::f64::consts::TAU * i as f64 / n as f64;
                            m.transform_point3(DVec3::new(radius * a.cos(), y, radius * a.sin()))
                        })
                        .collect()
                };
                let mut out = vec![ring(0.0), ring(h)];
                for k in 0..4 {
                    let a = std::f64::consts::FRAC_PI_2 * k as f64;
                    let p = DVec3::new(radius * a.cos(), 0.0, radius * a.sin());
                    out.push(vec![m.transform_point3(p), m.transform_point3(p + DVec3::Y * h)]);
                }
                out
            }
        }
    }
    /// Content hash (the kernel's caches, and the name of a step to skip).
    pub fn hash(&self) -> u64 {
        let json = serde_json::to_vec(self).unwrap_or_default();
        fnv(&json)
    }
}

/// FNV-1a over bytes.
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Doc {
    #[serde(default)]
    pub version: u32,
    pub features: Vec<Feature>,
    pub next_id: FeatureId,
}

impl Default for Doc {
    fn default() -> Self {
        Doc { version: VERSION, features: vec![], next_id: 1 }
    }
}

impl Doc {
    pub fn feature(&self, id: FeatureId) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id)
    }
    pub fn feature_mut(&mut self, id: FeatureId) -> Option<&mut Feature> {
        self.features.iter_mut().find(|f| f.id == id)
    }
    pub fn index_of(&self, id: FeatureId) -> Option<usize> {
        self.features.iter().position(|f| f.id == id)
    }
    /// Adds a feature at the end, with a fresh id and a name like "Cube 2".
    pub fn add(&mut self, op: Op, prim: Prim, origin: DVec3, rot: DQuat) -> FeatureId {
        let id = self.next_id;
        self.next_id += 1;
        let name = self.fresh_name(prim.kind().label());
        self.features.push(Feature { id, name, op, prim, origin, rot, visible: true, material: 0 });
        id
    }
    /// "Cube", then "Cube 2", "Cube 3"…
    pub fn fresh_name(&self, base: &str) -> String {
        if !self.features.iter().any(|f| f.name == base) {
            return base.to_string();
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| !self.features.iter().any(|f| &f.name == n)).unwrap()
    }
    pub fn remove(&mut self, ids: &[FeatureId]) {
        self.features.retain(|f| !ids.contains(&f.id));
    }
    /// Copies of the features (after the last one), with fresh ids; returns
    /// the new ids.
    pub fn duplicate(&mut self, ids: &[FeatureId], offset: DVec3) -> Vec<FeatureId> {
        let src: Vec<Feature> = self.features.iter().filter(|f| ids.contains(&f.id)).cloned().collect();
        let mut out = vec![];
        for mut f in src {
            f.id = self.next_id;
            self.next_id += 1;
            f.name = self.fresh_name(f.prim.kind().label());
            f.origin += offset;
            out.push(f.id);
            self.features.push(f);
        }
        out
    }
    /// Bounds of the visible features (None when empty).
    pub fn bounds(&self) -> Option<(DVec3, DVec3)> {
        self.features.iter().filter(|f| f.visible).map(|f| f.bounds()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
    pub fn from_json(s: &str) -> Result<Doc, String> {
        let d: Doc = serde_json::from_str(s).map_err(|e| e.to_string())?;
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let mut d = Doc::default();
        let a = d.add(Op::Add, PrimKind::Box.default_prim(), DVec3::ZERO, DQuat::IDENTITY);
        let b = d.add(Op::Carve, Prim::Cylinder { radius: 4.0, height: 30.0 }, DVec3::new(0.0, -5.0, 0.0), DQuat::IDENTITY);
        assert_eq!((a, b), (1, 2));
        assert_eq!(d.features[1].name, "Cylinder");
        assert_eq!(d.fresh_name("Cube"), "Cube 2");
        let back = Doc::from_json(&d.to_json()).unwrap();
        assert_eq!(back, d);
        let (lo, hi) = d.feature(1).unwrap().bounds();
        assert!((lo - DVec3::new(-10.0, 0.0, -10.0)).length() < 1e-9 && (hi - DVec3::new(10.0, 20.0, 10.0)).length() < 1e-9);
    }
}
