//! Kit of ready-made hard-surface details placed like stamps on a surface.
//! Each piece is a small group whose local Y axis is the surface normal.

use forme_core::demo::{MAT_DARK, MAT_RUBBER, MAT_STEEL};
use forme_core::*;
use glam::{Quat, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KitPiece {
    Screw,
    Rivet,
    Nut,
    Vent,
}

impl KitPiece {
    pub const ALL: [KitPiece; 4] = [KitPiece::Screw, KitPiece::Rivet, KitPiece::Nut, KitPiece::Vent];
    pub fn label(self) -> &'static str {
        match self {
            KitPiece::Screw => "Vis à fente",
            KitPiece::Rivet => "Rivet",
            KitPiece::Nut => "Écrou hexagonal",
            KitPiece::Vent => "Grille d'aération",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            KitPiece::Screw => "Vis",
            KitPiece::Rivet => "Rivet",
            KitPiece::Nut => "Écrou",
            KitPiece::Vent => "Grille",
        }
    }
}

fn cyl(r: f32, hh: f32, round: f32) -> Shape {
    let mut s = Shape::new(PrimKind::Cylinder);
    s.size = Vec3::new(r, hh, 0.0);
    s.round = round;
    s
}

/// Inserts a piece at `pos` with local Y along the surface normal. Returns the group id.
pub fn insert(scene: &mut Scene, parent: NodeId, piece: KitPiece, pos: Vec3, rot: Quat, scale: f32) -> NodeId {
    let mut spec = NodeSpec::group(piece.short(), false).at(pos).rot(rot);
    spec.transform.scale = scale;
    let g = scene.add(parent, spec);
    match piece {
        KitPiece::Screw => {
            scene.add(
                g,
                NodeSpec::shape("Logement", cyl(3.4, 1.2, 0.0)).mode(CombineMode::Carve).blend(0.3).mat(MAT_DARK),
            );
            scene.add(g, NodeSpec::shape("Tête", cyl(2.9, 0.9, 0.7)).at(Vec3::new(0.0, -0.3, 0.0)).mat(MAT_STEEL));
            let mut slot = Shape::new(PrimKind::Box);
            slot.size = Vec3::new(2.4, 1.0, 0.4);
            scene.add(
                g,
                NodeSpec::shape("Fente", slot)
                    .at(Vec3::new(0.0, 0.7, 0.0))
                    .rot(Quat::from_rotation_y(0.6))
                    .mode(CombineMode::Carve)
                    .mat(MAT_RUBBER),
            );
        }
        KitPiece::Rivet => {
            let mut s = Shape::new(PrimKind::Sphere);
            s.size = Vec3::new(2.4, 1.3, 2.4);
            scene.add(g, NodeSpec::shape("Tête", s).blend(0.6).mat(MAT_STEEL));
        }
        KitPiece::Nut => {
            let mut s = Shape::new(PrimKind::Prism);
            s.size = Vec3::new(4.0, 1.6, 0.0);
            s.round = 0.5;
            s.sides = 6;
            scene.add(g, NodeSpec::shape("Écrou", s).at(Vec3::new(0.0, 1.2, 0.0)).mat(MAT_STEEL));
            scene.add(
                g,
                NodeSpec::shape("Perçage", cyl(1.9, 3.0, 0.0)).at(Vec3::new(0.0, 1.2, 0.0)).mode(CombineMode::Carve).mat(MAT_DARK),
            );
        }
        KitPiece::Vent => {
            let mut s = Shape::new(PrimKind::Box);
            s.size = Vec3::new(7.0, 1.5, 0.9);
            s.round = 0.4;
            scene.add(
                g,
                NodeSpec::shape("Fentes", s)
                    .at(Vec3::new(0.0, 0.0, -6.4))
                    .mode(CombineMode::Carve)
                    .chamfer_blend(0.4)
                    .repeat(Repeat::Linear { count: 5, step: Vec3::new(0.0, 0.0, 3.2) })
                    .mat(MAT_DARK),
            );
        }
    }
    g
}
