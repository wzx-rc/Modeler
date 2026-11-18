//! Round pieces (buttons, rings, barrel ends): turned parts, an axis and a
//! profile. Coaxial cylinders (and the planes square to their axis) give
//! the axis; every vertex of the piece is put in the (radius, height) plane
//! of that axis, where all the angles fall on the same profile line, so the
//! noise of each angle averages out; each vertex goes onto that common
//! profile, at its own angle. The piece comes out truly round.
//!
//! A vertex far from the common profile is a real detail (a notch, a key),
//! not noise: it is left alone. A piece whose profile is thick (not round
//! after all) is skipped.

use crate::mesh::{Grid, Mesh, Topo};
use crate::prim::Prim;
use crate::segment::Segmentation;
use crate::NONE;
use glam::{DVec2, DVec3};
use rayon::prelude::*;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RevolveParams {
    pub on: bool,
    /// Two cylinders share an axis within this angle (degrees) and distance (mm).
    pub axis_angle: f64,
    pub axis_dist: f64,
    /// Smallest cylinder that starts a piece (mm²), largest radius (mm).
    pub min_area: f64,
    pub max_radius: f64,
    /// How far around the cylinders the piece reaches (mm), and over which
    /// the move fades out.
    pub margin: f64,
    /// Neighbourhood of the profile fit, in the (radius, height) plane (mm).
    pub radius: f64,
    /// A vertex farther than this from the profile is a detail (mm).
    pub outlier: f64,
    /// Thickest profile still taken as round (median distance, mm).
    pub max_thick: f64,
    /// Share of the turn the piece must cover (degrees).
    pub min_cover: f64,
    pub max_move: f64,
}

impl Default for RevolveParams {
    fn default() -> Self {
        RevolveParams { on: true, axis_angle: 2.5, axis_dist: 0.25, min_area: 1.5, max_radius: 20.0, margin: 0.8, radius: 0.22, outlier: 0.12, max_thick: 0.05, min_cover: 260.0, max_move: 0.4 }
    }
}

/// A round piece asked for by hand or by the AI pass: an axis through `at`,
/// up to `radius` from it and `depth` along it on both sides.
#[derive(Clone, Copy, Debug)]
pub struct Hint {
    pub at: DVec3,
    pub axis: DVec3,
    pub radius: f64,
    pub depth: f64,
}

#[derive(Clone, Debug)]
pub struct Piece {
    pub p: DVec3,
    pub a: DVec3,
    pub r_max: f64,
    pub h: (f64, f64),
    /// Vertices in the piece, moved onto the profile, left as details.
    pub verts: usize,
    pub moved: usize,
    pub details: usize,
    /// Median distance of the vertices to the profile (mm).
    pub thick: f64,
    pub hint: bool,
    /// Why it was skipped (None: done).
    pub skipped: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct RevolveReport {
    pub pieces: Vec<Piece>,
    pub moved: usize,
}

impl RevolveReport {
    pub fn done(&self) -> usize {
        self.pieces.iter().filter(|p| p.skipped.is_none()).count()
    }
}

struct Axis {
    p: DVec3,
    a: DVec3,
    members: Vec<u32>,
    r_max: f64,
    h: (f64, f64),
    hint: bool,
}

fn line_dist(x: DVec3, p: DVec3, a: DVec3) -> f64 {
    let v = x - p;
    (v - a * a.dot(v)).length()
}

/// The axes of coaxial cylinder groups, the largest first.
fn axes(m: &Mesh, seg: &Segmentation, p: &RevolveParams) -> Vec<Axis> {
    let cos = p.axis_angle.to_radians().cos();
    let mut cyl: Vec<(u32, DVec3, DVec3, f64, f64)> = seg
        .regions
        .iter()
        .enumerate()
        .filter_map(|(i, r)| match r.prim {
            Prim::Cylinder { p: c, a, r: rad } if r.area >= p.min_area && rad <= p.max_radius && !r.faces.is_empty() => Some((i as u32, c, a, rad, r.area)),
            _ => None,
        })
        .collect();
    cyl.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap());
    let mut groups: Vec<(DVec3, DVec3, Vec<(u32, f64, f64)>)> = vec![];
    for (i, c, a, r, area) in cyl {
        let g = groups.iter_mut().find(|g| g.1.dot(a).abs() > cos && line_dist(c, g.0, g.1) < p.axis_dist);
        match g {
            Some(g) => g.2.push((i, r, area)),
            None => groups.push((c, a, vec![(i, r, area)])),
        }
    }
    let mut out = vec![];
    for (c, a, mem) in groups {
        let r_max = mem.iter().map(|m| m.1).fold(0.0, f64::max) + p.margin;
        // the height range: the members' faces along the axis
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for (i, _, _) in &mem {
            for &f in &seg.regions[*i as usize].faces {
                let h = a.dot(m.tri_centre(f as usize) - c);
                lo = lo.min(h);
                hi = hi.max(h);
            }
        }
        // planes square to the axis close the piece (a button's top): the
        // range reaches a little past the ends
        let reach = p.margin.max(0.25 * r_max);
        out.push(Axis { p: c, a, members: mem.iter().map(|m| m.0).collect(), r_max, h: (lo - reach, hi + reach), hint: false });
    }
    out
}

/// A line or a circle through weighted 2D points; the projection of `q`.
fn profile_project(pts: &[(DVec2, f64)], q: DVec2) -> Option<(DVec2, f64)> {
    let wsum: f64 = pts.iter().map(|x| x.1).sum();
    if wsum <= 0.0 {
        return None;
    }
    let c = pts.iter().map(|x| x.0 * x.1).sum::<DVec2>() / wsum;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for (x, w) in pts {
        let d = *x - c;
        sxx += w * d.x * d.x;
        sxy += w * d.x * d.y;
        syy += w * d.y * d.y;
    }
    // the line's direction: the larger eigenvector of the 2×2 covariance
    let tr = sxx + syy;
    let det = sxx * syy - sxy * sxy;
    let l1 = tr * 0.5 + ((tr * tr * 0.25 - det).max(0.0)).sqrt();
    let dir = if sxy.abs() > 1e-15 { DVec2::new(l1 - syy, sxy).normalize() } else if sxx >= syy { DVec2::X } else { DVec2::Y };
    let nrm = DVec2::new(-dir.y, dir.x);
    let line_rms = (pts.iter().map(|(x, w)| w * nrm.dot(*x - c).powi(2)).sum::<f64>() / wsum).sqrt();
    // the points must spread along the profile well beyond its thickness,
    // else the direction is a guess
    let along = (pts.iter().map(|(x, w)| w * dir.dot(*x - c).powi(2)).sum::<f64>() / wsum).sqrt();
    if along < line_rms * 2.5 {
        return None;
    }
    let on_line = c + dir * dir.dot(q - c);
    // a circle (Kasa) when the line is poor: a fillet in the profile
    let mut a = [[0.0f64; 3]; 3];
    let mut b = [0.0f64; 3];
    for (x, w) in pts {
        let d = *x - c;
        let row = [d.x, d.y, 1.0];
        let rhs = -(d.x * d.x + d.y * d.y);
        for i in 0..3 {
            for j in 0..3 {
                a[i][j] += w * row[i] * row[j];
            }
            b[i] += w * row[i] * rhs;
        }
    }
    if let Some(s) = solve3(a, b) {
        let cc = DVec2::new(-s[0] * 0.5, -s[1] * 0.5);
        let r2 = cc.length_squared() - s[2];
        if r2 > 0.0 {
            let r = r2.sqrt();
            let circ_rms = (pts.iter().map(|(x, w)| w * ((*x - c - cc).length() - r).powi(2)).sum::<f64>() / wsum).sqrt();
            if r > 0.05 && r < 50.0 && circ_rms < line_rms * 0.6 {
                let v = q - c - cc;
                if v.length() > 1e-9 {
                    return Some((c + cc + v.normalize() * r, circ_rms));
                }
            }
        }
    }
    Some((on_line, line_rms))
}

fn solve3(a: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: &[[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = det(&a);
    if d.abs() < 1e-18 {
        return None;
    }
    let mut out = [0.0; 3];
    for k in 0..3 {
        let mut m = a;
        for i in 0..3 {
            m[i][k] = b[i];
        }
        out[k] = det(&m) / d;
    }
    Some(out)
}

/// Puts the round pieces' vertices on their common profile. `hints` add
/// pieces; `off` drops the pieces whose axis passes near one of its points.
pub fn revolve(m: &mut Mesh, topo: &Topo, seg: &Segmentation, hints: &[Hint], off: &[DVec3], p: &RevolveParams) -> RevolveReport {
    let mut rep = RevolveReport::default();
    if !p.on {
        return rep;
    }
    let mut list = axes(m, seg, p);
    // the pieces dropped by hand / by the AI pass (not the ones asked for)
    list.retain(|ax| !off.iter().any(|o| line_dist(*o, ax.p, ax.a) < ax.r_max && (ax.h.0..=ax.h.1).contains(&ax.a.dot(*o - ax.p))));
    // a piece asked for replaces the automatic ones on its spot
    list.retain(|ax| !hints.iter().any(|h| (ax.p + ax.a * ax.a.dot(h.at - ax.p) - h.at).length() < h.radius * 0.5 && ax.a.dot(h.axis.normalize_or_zero()).abs() < 0.9));
    for h in hints {
        let a = h.axis.normalize_or_zero();
        if a == DVec3::ZERO {
            continue;
        }
        list.insert(0, Axis { p: h.at, a, members: vec![], r_max: h.radius + p.margin, h: (-h.depth, h.depth), hint: true });
    }
    let grid = Grid::new(&m.pos, 1.0);
    // a vertex's regions (its faces' labels)
    let mut done = vec![false; m.pos.len()];
    let cos3 = 3f64.to_radians().cos();
    for ax in list {
        let (c, a) = (ax.p, ax.a);
        let mid = c + a * ((ax.h.0 + ax.h.1) * 0.5);
        let reach = (ax.r_max * ax.r_max + ((ax.h.1 - ax.h.0) * 0.5).powi(2)).sqrt();
        // which regions belong: the group's cylinders, planes square to the
        // axis, spheres centred on it, free faces
        let mut ok_region: HashMap<u32, bool> = HashMap::new();
        let mut belongs = |l: u32| -> bool {
            if l == NONE || ax.hint {
                return true;
            }
            *ok_region.entry(l).or_insert_with(|| {
                if ax.members.contains(&l) {
                    return true;
                }
                match seg.regions[l as usize].prim {
                    Prim::Plane { n, .. } => n.dot(a).abs() > cos3,
                    Prim::Sphere { c: sc, .. } => line_dist(sc, c, a) < p.axis_dist,
                    Prim::Cylinder { p: q, a: b, .. } => b.dot(a).abs() > p.axis_angle.to_radians().cos() && line_dist(q, c, a) < p.axis_dist,
                }
            })
        };
        let mut verts: Vec<usize> = vec![];
        grid.near(mid, reach, |v| {
            let v = v as usize;
            if done[v] {
                return;
            }
            let x = m.pos[v] - c;
            let h = a.dot(x);
            let r = (x - a * h).length();
            if r > ax.r_max || h < ax.h.0 || h > ax.h.1 {
                return;
            }
            let ok = topo.tris_of(v).iter().all(|&t| belongs(seg.label[t as usize]));
            if ok {
                verts.push(v);
            }
        });
        let mut piece = Piece { p: c, a, r_max: ax.r_max, h: ax.h, verts: verts.len(), moved: 0, details: 0, thick: 0.0, hint: ax.hint, skipped: None };
        if verts.len() < 60 {
            piece.skipped = Some("too few vertices".into());
            rep.pieces.push(piece);
            continue;
        }
        let u = a.any_orthonormal_vector();
        let w = a.cross(u);
        // (radius, height, angle bin) of each vertex
        let rh: Vec<(DVec2, u8)> = verts
            .iter()
            .map(|&v| {
                let x = m.pos[v] - c;
                let h = a.dot(x);
                let rad = x - a * h;
                let ang = rad.dot(w).atan2(rad.dot(u));
                let bin = (((ang + std::f64::consts::PI) / std::f64::consts::TAU * 36.0) as i64).clamp(0, 35) as u8;
                (DVec2::new(rad.length(), h), bin)
            })
            .collect();
        // the turn must be (mostly) there
        let mut bins = [false; 36];
        for (q, b) in &rh {
            if q.x > ax.r_max * 0.3 {
                bins[*b as usize] = true;
            }
        }
        let cover = bins.iter().filter(|b| **b).count() as f64 * 10.0;
        if cover < p.min_cover {
            piece.skipped = Some(format!("covers {cover:.0}° of the turn"));
            rep.pieces.push(piece);
            continue;
        }
        // the profile: each vertex's neighbours in the (r, h) plane, from
        // every angle
        // the neighbourhood spans a few triangles at least
        let edge = {
            let mut e: Vec<f64> = verts.iter().take(400).filter_map(|&v| topo.verts_of(v).first().map(|&u| (m.pos[v] - m.pos[u as usize]).length())).collect();
            e.sort_by(|a, b| a.partial_cmp(b).unwrap());
            e.get(e.len() / 2).copied().unwrap_or(p.radius)
        };
        let cell = p.radius.max(edge * 2.5);
        let mut hash: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
        for (i, (q, _)) in rh.iter().enumerate() {
            hash.entry(((q.x / cell).floor() as i64, (q.y / cell).floor() as i64)).or_default().push(i as u32);
        }
        let sig = cell * 0.5;
        let fits: Vec<Option<(DVec2, f64)>> = rh
            .par_iter()
            .map(|(q, _)| {
                let (ci, cj) = ((q.x / cell).floor() as i64, (q.y / cell).floor() as i64);
                let mut pts = vec![];
                let mut nb = [false; 36];
                for di in -1..=1 {
                    for dj in -1..=1 {
                        let Some(list) = hash.get(&(ci + di, cj + dj)) else { continue };
                        for &k in list {
                            let (o, b) = rh[k as usize];
                            let d2 = (o - *q).length_squared();
                            if d2 <= cell * cell {
                                pts.push((o, (-d2 / (sig * sig)).exp()));
                                nb[b as usize] = true;
                            }
                        }
                    }
                }
                // a consensus of many angles, or nothing
                if pts.len() < 12 || nb.iter().filter(|b| **b).count() < 8 {
                    return None;
                }
                profile_project(&pts, *q)
            })
            .collect();
        let mut dists: Vec<f64> = rh.iter().zip(&fits).filter_map(|((q, _), f)| f.map(|(g, _)| (g - *q).length())).collect();
        if dists.len() < verts.len() / 3 {
            piece.skipped = Some("profile not found".into());
            rep.pieces.push(piece);
            continue;
        }
        dists.sort_by(|a, b| a.partial_cmp(b).unwrap());
        piece.thick = dists[dists.len() / 2];
        if piece.thick > p.max_thick {
            piece.skipped = Some(format!("not round ({:.3} mm thick)", piece.thick));
            rep.pieces.push(piece);
            continue;
        }
        for (i, &v) in verts.iter().enumerate() {
            let Some((g, _)) = fits[i] else { continue };
            let q = rh[i].0;
            let mv = (g - q).length();
            if mv > p.outlier || mv > p.max_move {
                piece.details += 1;
                continue;
            }
            // fades out at the piece's rim and ends
            let fr = ((ax.r_max - q.x) / p.margin).clamp(0.0, 1.0);
            let fh = ((q.y - ax.h.0).min(ax.h.1 - q.y) / p.margin).clamp(0.0, 1.0);
            let f = smooth(fr.min(fh));
            if f <= 0.0 || q.x < 1e-6 {
                continue;
            }
            let x = m.pos[v] - c;
            let radial = (x - a * a.dot(x)) / q.x;
            let target = c + a * g.y + radial * g.x;
            let cur = m.pos[v];
            m.pos[v] = cur + (target - cur) * f;
            done[v] = true;
            piece.moved += 1;
        }
        rep.moved += piece.moved;
        rep.pieces.push(piece);
    }
    rep
}

fn smooth(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::Region;

    /// A noisy cylinder (a button's side) comes out round.
    #[test]
    fn noisy_cylinder_rounds() {
        let (na, nh) = (96usize, 50usize);
        let mut pos = vec![];
        let mut seed = 7u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as f64 / (1u64 << 31) as f64) - 0.5
        };
        for j in 0..nh {
            for i in 0..na {
                let t = i as f64 / na as f64 * std::f64::consts::TAU;
                let r = 5.0 + rnd() * 0.06;
                pos.push(DVec3::new(r * t.cos(), j as f64 * 0.1, r * t.sin()));
            }
        }
        let mut tris = vec![];
        for j in 0..nh - 1 {
            for i in 0..na {
                let a = (j * na + i) as u32;
                let b = (j * na + (i + 1) % na) as u32;
                tris.push([a, a + na as u32, b]);
                tris.push([b, a + na as u32, b + na as u32]);
            }
        }
        let mut m = Mesh { pos, tris, uv: vec![], tri_uv: vec![], unit_mm: 1.0 };
        let topo = Topo::new(&m);
        let faces: Vec<u32> = (0..m.tris.len() as u32).collect();
        let seg = Segmentation { label: vec![0; m.tris.len()], regions: vec![Region { prim: Prim::Cylinder { p: DVec3::ZERO, a: DVec3::Y, r: 5.0 }, faces, area: 100.0, rms: 0.02, locked: false }] };
        let spread = |m: &Mesh| {
            let rs: Vec<f64> = m.pos.iter().filter(|x| x.y > 1.5 && x.y < 3.5).map(|x| (x.x * x.x + x.z * x.z).sqrt()).collect();
            let mean = rs.iter().sum::<f64>() / rs.len() as f64;
            (rs.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / rs.len() as f64).sqrt()
        };
        let before = spread(&m);
        let p = RevolveParams { margin: 0.5, ..Default::default() };
        let rep = revolve(&mut m, &topo, &seg, &[], &[], &p);
        let after = spread(&m);
        assert_eq!(rep.done(), 1, "{:?}", rep.pieces);
        assert!(after < before * 0.4, "{before} -> {after}");
    }
}
