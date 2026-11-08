//! Reference scene: the sci-fi grenade used as the v1 acceptance test.
//! Units: mm, Y up, front = +Z.

use crate::scene::*;
use glam::{Quat, Vec2, Vec3};
use std::f32::consts::FRAC_PI_2;

pub const MAT_CLAY: u32 = 0;
pub const MAT_OLIVE: u32 = 1;
pub const MAT_ORANGE: u32 = 2;
pub const MAT_DARK: u32 = 3;
pub const MAT_STEEL: u32 = 4;
pub const MAT_RUBBER: u32 = 5;

fn cyl(r: f32, hh: f32, round: f32) -> Shape {
    let mut s = Shape::new(PrimKind::Cylinder);
    s.size = Vec3::new(r, hh, 0.0);
    s.round = round;
    s
}

fn bx(h: Vec3, round: f32) -> Shape {
    let mut s = Shape::new(PrimKind::Box);
    s.size = h;
    s.round = round;
    s
}

fn ell(r: Vec3) -> Shape {
    let mut s = Shape::new(PrimKind::Sphere);
    s.size = r;
    s
}

fn torus(major: f32, minor: f32) -> Shape {
    let mut s = Shape::new(PrimKind::Torus);
    s.size = Vec3::new(major, 0.0, minor);
    s
}

/// Y axis → +Z
fn to_z() -> Quat {
    Quat::from_rotation_x(FRAC_PI_2)
}
/// Y axis → +X
fn to_x() -> Quat {
    Quat::from_rotation_z(-FRAC_PI_2)
}

pub fn lever_profile() -> Profile {
    let pts: [(f32, f32, bool); 21] = [
        (16.0, 132.0, false),
        (24.0, 142.0, true),
        (38.0, 141.0, true),
        (46.0, 130.0, true),
        (49.0, 104.0, false),
        (49.0, 62.0, true),
        (47.0, 36.0, false),
        (44.0, 22.0, true),
        (40.0, 22.0, false),
        (40.0, 34.0, false),
        (37.0, 40.0, true),
        (40.0, 46.0, false),
        (37.0, 52.0, true),
        (40.0, 58.0, false),
        (37.0, 64.0, true),
        (40.0, 70.0, false),
        (41.0, 104.0, false),
        (40.0, 120.0, true),
        (33.0, 130.0, true),
        (24.0, 132.0, true),
        (19.0, 128.0, false),
    ];
    Profile {
        points: pts.iter().map(|(x, y, s)| ProfilePoint { pos: Vec2::new(*x, *y), smooth: *s }).collect(),
        mode: ProfileMode::Extrude,
    }
}

pub fn grenade() -> Scene {
    let mut s = Scene::new();
    let root = s.root;

    // --- body ---------------------------------------------------------------
    let body = s.add(root, NodeSpec::group("Corps", false));
    if let Some(n) = s.node_mut(body) {
        n.expanded = true;
    }
    let mut corps = cyl(27.0, 34.0, 10.0);
    corps.taper = 0.04;
    s.add(body, NodeSpec::shape("Fût", corps).at(Vec3::new(0.0, 58.0, 0.0)).mat(MAT_OLIVE));

    let lobes = s.add(
        body,
        NodeSpec::group("Lobes", false)
            .at(Vec3::new(0.0, 46.0, 0.0))
            .repeat(Repeat::Linear { count: 2, step: Vec3::new(0.0, 24.0, 0.0) }),
    );
    s.add(
        lobes,
        NodeSpec::shape("Lobe", ell(Vec3::new(14.0, 15.5, 12.0)))
            .at(Vec3::new(0.0, 0.0, 19.0))
            .blend(5.0)
            .repeat(Repeat::Radial { count: 4, axis: Axis::Y })
            .mat(MAT_OLIVE),
    );

    let bosses = s.add(
        body,
        NodeSpec::group("Bossages", false)
            .at(Vec3::new(0.0, 46.0, 0.0))
            .repeat(Repeat::Linear { count: 2, step: Vec3::new(0.0, 24.0, 0.0) }),
    );
    if let Some(n) = s.node_mut(bosses) {
        n.expanded = true;
    }
    let radial4 = Repeat::Radial { count: 4, axis: Axis::Y };
    s.add(
        bosses,
        NodeSpec::shape("Bossage", cyl(6.5, 3.0, 1.2))
            .at(Vec3::new(0.0, 0.0, 30.0))
            .rot(to_z())
            .blend(1.5)
            .repeat(radial4)
            .mat(MAT_OLIVE),
    );
    s.add(
        bosses,
        NodeSpec::shape("Logement vis", cyl(3.7, 2.5, 0.0))
            .at(Vec3::new(0.0, 0.0, 33.2))
            .rot(to_z())
            .mode(CombineMode::Carve)
            .blend(0.4)
            .repeat(radial4)
            .mat(MAT_DARK),
    );
    s.add(
        bosses,
        NodeSpec::shape("Vis", cyl(3.0, 1.0, 0.8))
            .at(Vec3::new(0.0, 0.0, 32.4))
            .rot(to_z())
            .repeat(radial4)
            .mat(MAT_STEEL),
    );
    s.add(
        bosses,
        NodeSpec::shape("Fente", bx(Vec3::new(2.6, 0.45, 1.0), 0.0))
            .at(Vec3::new(0.0, 0.0, 33.5))
            .rot(Quat::from_rotation_z(0.6))
            .mode(CombineMode::Carve)
            .repeat(radial4)
            .mat(MAT_RUBBER),
    );

    s.add(body, NodeSpec::shape("Bague basse", cyl(28.6, 2.6, 1.2)).at(Vec3::new(0.0, 27.0, 0.0)).mat(MAT_ORANGE));
    s.add(body, NodeSpec::shape("Base", cyl(23.0, 7.0, 5.0)).at(Vec3::new(0.0, 15.0, 0.0)).blend(2.0).mat(MAT_OLIVE));
    s.add(
        body,
        NodeSpec::shape("Pieds", ell(Vec3::new(6.0, 4.0, 6.0)))
            .at(Vec3::new(0.0, 8.5, 15.0))
            .blend(2.0)
            .repeat(Repeat::Radial { count: 6, axis: Axis::Y })
            .mat(MAT_OLIVE),
    );

    // --- neck & head ----------------------------------------------------------
    let head = s.add(root, NodeSpec::group("Tête", false));
    s.add(head, NodeSpec::shape("Noyau du col", cyl(14.0, 9.0, 0.0)).at(Vec3::new(0.0, 100.0, 0.0)).mat(MAT_DARK));
    s.add(
        head,
        NodeSpec::shape("Filetage", torus(14.4, 1.5))
            .at(Vec3::new(0.0, 93.5, 0.0))
            .repeat(Repeat::Linear { count: 5, step: Vec3::new(0.0, 3.3, 0.0) })
            .mat(MAT_STEEL),
    );
    s.add(head, NodeSpec::shape("Collier", cyl(19.0, 4.0, 1.5)).at(Vec3::new(0.0, 112.0, 0.0)).mat(MAT_ORANGE));
    s.add(head, NodeSpec::shape("Bloc", bx(Vec3::new(16.0, 8.0, 13.0), 3.5)).at(Vec3::new(0.0, 124.0, 0.0)).mat(MAT_DARK));
    s.add(
        head,
        NodeSpec::shape("Charnière", bx(Vec3::new(8.0, 7.0, 8.5), 3.0))
            .at(Vec3::new(13.0, 126.0, 0.0))
            .blend(1.0)
            .mat(MAT_ORANGE),
    );
    s.add(
        head,
        NodeSpec::shape("Trou d'axe", cyl(2.4, 10.0, 0.0))
            .at(Vec3::new(15.0, 126.0, 0.0))
            .rot(to_z())
            .mode(CombineMode::Carve)
            .blend(0.3)
            .mat(MAT_DARK),
    );
    s.add(
        head,
        NodeSpec::shape("Axe", cyl(1.7, 9.6, 0.4)).at(Vec3::new(15.0, 126.0, 0.0)).rot(to_z()).mat(MAT_STEEL),
    );
    s.add(
        head,
        NodeSpec::shape("Tambour", cyl(12.0, 19.0, 2.0)).at(Vec3::new(-6.0, 142.0, 0.0)).rot(to_x()).mat(MAT_DARK),
    );
    s.add(
        head,
        NodeSpec::shape("Rainure", bx(Vec3::new(0.1, 20.0, 20.0), 0.0))
            .at(Vec3::new(3.0, 142.0, 0.0))
            .mode(CombineMode::Groove)
            .groove(1.2, 0.9)
            .mat(MAT_DARK),
    );
    s.add(
        head,
        NodeSpec::shape("Capuchon", cyl(10.5, 3.0, 1.2)).at(Vec3::new(-25.5, 142.0, 0.0)).rot(to_x()).mat(MAT_ORANGE),
    );
    s.add(
        head,
        NodeSpec::shape("Bouchon", cyl(6.0, 3.6, 0.8)).at(Vec3::new(-27.5, 142.0, 0.0)).rot(to_x()).mat(MAT_DARK),
    );

    // --- lever ------------------------------------------------------------------
    let lever = s.add(root, NodeSpec::group("Levier", false));
    let mut prof = Shape::new(PrimKind::Profile);
    prof.size = Vec3::new(0.0, 0.0, 4.5);
    prof.round = 1.2;
    prof.profile = Some(lever_profile());
    s.add(lever, NodeSpec::shape("Lame", prof).at(Vec3::new(0.0, 0.0, -4.0)).mat(MAT_OLIVE));
    let mut pin = Shape::new(PrimKind::Capsule);
    pin.size = Vec3::new(1.4, 6.0, 0.0);
    s.add(lever, NodeSpec::shape("Goupille", pin).at(Vec3::new(55.0, 112.0, -4.0)).rot(to_x()).mat(MAT_STEEL));
    s.add(
        lever,
        NodeSpec::shape("Anneau", torus(6.5, 1.3)).at(Vec3::new(63.5, 112.0, -4.0)).rot(to_z()).mat(MAT_STEEL),
    );
    s
}

/// A small starter scene: one rounded block.
pub fn starter() -> Scene {
    let mut s = Scene::new();
    let root = s.root;
    s.add(root, NodeSpec::shape("Bloc", bx(Vec3::new(30.0, 20.0, 20.0), 3.0)).at(Vec3::new(0.0, 20.0, 0.0)));
    s
}
