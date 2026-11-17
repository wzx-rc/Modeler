//! The document: features applied in order. Add = union with what is
//! there, Carve = subtraction. A feature is a primitive standing on its base:
//! `origin` is the centre of its base, it rises along its local +Y (the
//! rotation `rot` turns local into world). Millimetres, Y up.
//!
//! A sketched feature (`Prim::Profile`) is a closed outline of corners and
//! curve points (`Profile`) drawn on a plane, pulled to a height.
//!
//! A feature can also be a fillet or a chamfer (`round`) of edges of what is
//! built before it. Its edges are named by the tags of the two faces each
//! one joins (`EdgeRef`), so they are found again when a shape moves.

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
    /// A sketched outline (`Feature::profile`) pulled to a height: the
    /// outline's box width (local X), the height (local Y), its box depth
    /// (local Z). Resizing it stretches the outline.
    Profile { x: f64, y: f64, z: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrimKind {
    Box,
    Cylinder,
    /// The Sketch tool's outlines.
    Profile,
}

impl PrimKind {
    pub fn label(self) -> &'static str {
        match self {
            PrimKind::Box => "Cube",
            PrimKind::Cylinder => "Cylinder",
            PrimKind::Profile => "Sketch",
        }
    }
    /// Default primitive: 20 mm.
    pub fn default_prim(self) -> Prim {
        match self {
            PrimKind::Box => Prim::Box { x: 20.0, y: 20.0, z: 20.0 },
            PrimKind::Cylinder => Prim::Cylinder { radius: 10.0, height: 20.0 },
            PrimKind::Profile => Prim::Profile { x: 20.0, y: 20.0, z: 20.0 },
        }
    }
}

impl Prim {
    pub fn kind(&self) -> PrimKind {
        match self {
            Prim::Box { .. } => PrimKind::Box,
            Prim::Cylinder { .. } => PrimKind::Cylinder,
            Prim::Profile { .. } => PrimKind::Profile,
        }
    }
    pub fn height(&self) -> f64 {
        match *self {
            Prim::Box { y, .. } | Prim::Profile { y, .. } => y,
            Prim::Cylinder { height, .. } => height,
        }
    }
    pub fn set_height(&mut self, h: f64) {
        match self {
            Prim::Box { y, .. } | Prim::Profile { y, .. } => *y = h,
            Prim::Cylinder { height, .. } => *height = h,
        }
    }
    /// Half extents in the local frame, around the base centre: x, height, z.
    pub fn half_xz(&self) -> (f64, f64) {
        match *self {
            Prim::Box { x, z, .. } | Prim::Profile { x, z, .. } => (x * 0.5, z * 0.5),
            Prim::Cylinder { radius, .. } => (radius, radius),
        }
    }
}

fn yes() -> bool {
    true
}

/// A sketched outline: its points in the unit square [-0.5, 0.5]² of the
/// feature's local X and Z (scaled by the profile's box), each a corner or
/// a curve point. A curve point rounds the outline into a quadratic curve
/// from the middle of its incoming side to the middle of its outgoing one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub points: Vec<[f64; 2]>,
    pub smooth: Vec<bool>,
}

/// One piece of an outline: a curve around a curve point, or a straight
/// side between two pieces.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    /// 2 i for the curve around point i, 2 i + 1 for the side after it.
    pub id: usize,
    pub pts: Vec<[f64; 2]>,
}

impl Profile {
    /// The points scaled to a box of `w` × `d`.
    pub fn scaled(&self, w: f64, d: f64) -> Vec<[f64; 2]> {
        self.points.iter().map(|p| [p[0] * w, p[1] * d]).collect()
    }
    /// Its pieces, in a box of `w` × `d`, curves sampled (as the kernel
    /// builds them: a curve between the middles of the two sides).
    pub fn pieces(&self, w: f64, d: f64) -> Vec<Piece> {
        pieces(&self.scaled(w, d), &self.smooth)
    }
    /// The closed outline as one polyline (last point = first).
    pub fn polyline(&self, w: f64, d: f64) -> Vec<[f64; 2]> {
        let mut out: Vec<[f64; 2]> = vec![];
        for piece in self.pieces(w, d) {
            for p in piece.pts {
                if out.last().map(|q| (q[0] - p[0]).abs() + (q[1] - p[1]).abs() > 1e-9).unwrap_or(true) {
                    out.push(p);
                }
            }
        }
        if let Some(f) = out.first().copied() {
            out.push(f);
        }
        out
    }
    /// Mirrored across local X (the feature turned over to grow downwards).
    pub fn flipped(&self) -> Profile {
        Profile { points: self.points.iter().map(|p| [p[0], -p[1]]).collect(), smooth: self.smooth.clone() }
    }
}

/// The pieces of a closed outline of points (corners or curve points).
pub fn pieces(p: &[[f64; 2]], smooth: &[bool]) -> Vec<Piece> {
    let n = p.len();
    let mut out = vec![];
    if n < 2 {
        return out;
    }
    let mid = |a: [f64; 2], b: [f64; 2]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    let far = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() + (a[1] - b[1]).abs() > 1e-9;
    let round = |i: usize| smooth.get(i).copied().unwrap_or(false) && far(p[(i + n - 1) % n], p[i]) && far(p[i], p[(i + 1) % n]);
    // where each point's piece starts and ends (the point itself at a corner)
    let ends = |i: usize| if round(i) { (mid(p[(i + n - 1) % n], p[i]), mid(p[i], p[(i + 1) % n])) } else { (p[i], p[i]) };
    for i in 0..n {
        let (a, b) = ends(i);
        if round(i) {
            let k = 16;
            let pts = (0..=k)
                .map(|j| {
                    let t = j as f64 / k as f64;
                    let (u, v, w) = ((1.0 - t) * (1.0 - t), 2.0 * t * (1.0 - t), t * t);
                    [u * a[0] + v * p[i][0] + w * b[0], u * a[1] + v * p[i][1] + w * b[1]]
                })
                .collect();
            out.push(Piece { id: 2 * i, pts });
        }
        let (next, _) = ends((i + 1) % n);
        if far(b, next) {
            out.push(Piece { id: 2 * i + 1, pts: vec![b, next] });
        }
    }
    out
}

/// An edge of the part, found again at every build: the packed tags
/// (`model::FaceTag::pack`) of the two faces it joins, in any order, and a
/// point on it to choose between edges joining the same two faces.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeRef {
    pub faces: [i64; 2],
    pub at: DVec3,
}

impl EdgeRef {
    /// The two tags in increasing order.
    pub fn key(&self) -> [i64; 2] {
        let [a, b] = self.faces;
        if a <= b { [a, b] } else { [b, a] }
    }
}

/// A fillet (or a chamfer) of edges of what is built before it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Round {
    /// Fillet radius, or chamfer distance (mm).
    pub size: f64,
    #[serde(default)]
    pub chamfer: bool,
    pub edges: Vec<EdgeRef>,
    /// Blend corners: made in one operation with the fillets just before it
    /// that it touches, so they meet in a rounded corner instead of each
    /// ending on its own (fillets only).
    #[serde(default)]
    pub blend: bool,
}

impl Round {
    pub fn label(&self) -> &'static str {
        if self.chamfer { "Chamfer" } else { "Fillet" }
    }
}

/// A face of the part, found again at every build: its packed tag and a
/// point on it (to choose between faces with the same tag).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FaceRef {
    pub tag: i64,
    pub at: DVec3,
}

/// A flat face pushed or pulled (the Inset and Extrude tools): its
/// outline moved in by `inset` (0: the whole face), then moved along its
/// normal by `depth`: out (> 0, material added), in (< 0, carved), or not
/// at all (0: the inset is only drawn on the face).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Push {
    pub face: FaceRef,
    #[serde(default)]
    pub inset: f64,
    #[serde(default)]
    pub depth: f64,
}

impl Push {
    pub fn label(&self) -> &'static str {
        if self.inset > 0.0 { "Inset" } else { "Extrude" }
    }
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
    /// A fillet or chamfer step: `op`, `prim` and `rot` are not used (a box
    /// of size 0), `origin` is the middle of its edges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<Round>,
    /// A sketched feature's outline (`Prim::Profile`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
    /// A face pushed or pulled: like `round`, a step on what is built
    /// before it (`origin` is the face point).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<Push>,
}

impl Feature {
    /// A primitive (Add / Carve), not a step on the part (fillet,
    /// chamfer, inset, extrude).
    pub fn is_solid(&self) -> bool {
        self.round.is_none() && self.push.is_none()
    }
    /// What it is: "Cube", "Cylinder", "Fillet", "Chamfer", "Inset", "Extrude".
    pub fn kind_label(&self) -> &'static str {
        match (&self.round, &self.push) {
            (Some(r), _) => r.label(),
            (_, Some(p)) => p.label(),
            _ => self.prim.kind().label(),
        }
    }
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
            Prim::Profile { x, z, .. } => {
                let Some(pr) = &self.profile else { return vec![] };
                let line = pr.polyline(x, z);
                let ring = |y: f64| -> Vec<DVec3> { line.iter().map(|q| m.transform_point3(DVec3::new(q[0], y, q[1]))).collect() };
                let mut out = vec![ring(0.0), ring(h)];
                // the corners' uprights
                for (i, q) in pr.scaled(x, z).iter().enumerate() {
                    if !pr.smooth.get(i).copied().unwrap_or(false) {
                        let b = m.transform_point3(DVec3::new(q[0], 0.0, q[1]));
                        out.push(vec![b, b + self.axis() * h]);
                    }
                }
                out
            }
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
        self.features.push(Feature { id, name, op, prim, origin, rot, visible: true, material: 0, round: None, profile: None, push: None });
        id
    }
    /// Adds a fillet (or chamfer) of these edges at the end.
    pub fn add_round(&mut self, size: f64, chamfer: bool, edges: Vec<EdgeRef>) -> FeatureId {
        let id = self.next_id;
        self.next_id += 1;
        let round = Round { size, chamfer, edges, blend: !chamfer };
        let name = self.fresh_name(round.label());
        let origin = round.edges.iter().map(|e| e.at).sum::<DVec3>() / round.edges.len().max(1) as f64;
        self.features.push(Feature { id, name, op: Op::Add, prim: Prim::Box { x: 0.0, y: 0.0, z: 0.0 }, origin, rot: DQuat::IDENTITY, visible: true, material: 0, round: Some(round), profile: None, push: None });
        id
    }
    /// Adds an inset or extrusion of a face at the end.
    pub fn add_push(&mut self, push: Push) -> FeatureId {
        let id = self.next_id;
        self.next_id += 1;
        let name = self.fresh_name(push.label());
        let origin = push.face.at;
        self.features.push(Feature { id, name, op: Op::Add, prim: Prim::Box { x: 0.0, y: 0.0, z: 0.0 }, origin, rot: DQuat::IDENTITY, visible: true, material: 0, round: None, profile: None, push: Some(push) });
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
    /// Copies of the shapes (after the last feature), with fresh ids; returns
    /// the new ids. Fillets and chamfers are not copied (their edges belong
    /// to the original).
    pub fn duplicate(&mut self, ids: &[FeatureId], offset: DVec3) -> Vec<FeatureId> {
        let src: Vec<Feature> = self.features.iter().filter(|f| ids.contains(&f.id) && f.is_solid()).cloned().collect();
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
        let r = d.add_round(2.0, false, vec![EdgeRef { faces: [65538, 65536], at: DVec3::new(10.0, 20.0, 0.0) }]);
        assert_eq!(d.feature(r).unwrap().name, "Fillet");
        assert_eq!(d.feature(r).unwrap().round.as_ref().unwrap().edges[0].key(), [65536, 65538]);
        assert!(d.feature(1).unwrap().round.is_none() && !d.to_json().contains("\"round\": null"));
        let back = Doc::from_json(&d.to_json()).unwrap();
        assert_eq!(back, d);
        let (lo, hi) = d.feature(1).unwrap().bounds();
        assert!((lo - DVec3::new(-10.0, 0.0, -10.0)).length() < 1e-9 && (hi - DVec3::new(10.0, 20.0, 10.0)).length() < 1e-9);
    }
}
