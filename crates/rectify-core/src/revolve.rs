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
    /// The axis found again from the normals of the piece.
    pub refine: bool,
    /// A region belongs to a piece when its faces' normals meet the axis
    /// within this (sine of the angle).
    pub turn_tol: f64,
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
    /// A piece's radius grows by this (twice at most) while its profile holds.
    pub grow: f64,
    pub max_move: f64,
}

impl Default for RevolveParams {
    fn default() -> Self {
        RevolveParams { on: true, refine: true, turn_tol: 0.15, axis_angle: 2.5, axis_dist: 0.25, min_area: 1.5, max_radius: 20.0, margin: 0.8, radius: 0.22, outlier: 0.12, max_thick: 0.05, min_cover: 260.0, grow: 1.35, max_move: 0.4 }
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
    /// How far from round it was (oval or off-centre, mm).
    pub oval: f64,
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

/// The vertices of a piece around the axis (c, a): within its radius and
/// height, all their faces on regions that belong to a turned piece (the
/// group's cylinders, planes square to the axis, spheres centred on it,
/// coaxial cylinders, free faces), not already in a piece.
#[allow(clippy::too_many_arguments)]
fn gather(base: &Mesh, tn: &[DVec3], topo: &Topo, seg: &Segmentation, grid: &Grid, done: &[bool], ax: &Axis, c: DVec3, a: DVec3, p: &RevolveParams) -> Vec<usize> {
    let mid = c + a * ((ax.h.0 + ax.h.1) * 0.5);
    let reach = (ax.r_max * ax.r_max + ((ax.h.1 - ax.h.0) * 0.5).powi(2)).sqrt();
    // a region belongs when its faces could be turned about the axis: each
    // face's normal line meets the axis ((x − c) · (n × a) = 0), whatever
    // shape it was fitted with (a bore cut into flats belongs too)
    let mut ok_region: HashMap<u32, bool> = HashMap::new();
    let mut belongs = |l: u32| -> bool {
        if l == NONE || ax.hint {
            return true;
        }
        *ok_region.entry(l).or_insert_with(|| {
            if ax.members.contains(&l) {
                return true;
            }
            let faces = &seg.regions[l as usize].faces;
            let step = (faces.len() / 60).max(1);
            let (mut n_ok, mut n_all) = (0usize, 0usize);
            for &f in faces.iter().step_by(step) {
                let d = base.tri_centre(f as usize) - c;
                let rp = (d - a * a.dot(d)).length();
                if rp < 1e-6 {
                    continue;
                }
                n_all += 1;
                if (d.dot(tn[f as usize].cross(a)) / rp).abs() < p.turn_tol {
                    n_ok += 1;
                }
            }
            n_all > 0 && n_ok as f64 >= 0.8 * n_all as f64
        })
    };
    let mut verts: Vec<usize> = vec![];
    grid.near(mid, reach, |v| {
        let v = v as usize;
        if done[v] {
            return;
        }
        let x = base.pos[v] - c;
        let h = a.dot(x);
        let r = (x - a * h).length();
        if r > ax.r_max || h < ax.h.0 || h > ax.h.1 {
            return;
        }
        if topo.tris_of(v).iter().all(|&t| belongs(seg.label[t as usize])) {
            verts.push(v);
        }
    });
    verts
}

struct Fit {
    /// (radius, height) and angle bin of each vertex, as on the AI mesh.
    rh: Vec<(DVec2, u8)>,
    /// The radius taken out of each vertex to make the turn round (an
    /// oval or off-centre piece), mm.
    shift: Vec<f64>,
    /// Each vertex's point on the common profile.
    fits: Vec<Option<(DVec2, f64)>>,
    /// Median distance to the profile (mm).
    thick: f64,
    /// How far from round the piece was (largest radius taken out, mm).
    oval: f64,
}

/// Each point's projection on the common profile (its point, the local
/// fit's spread, the profile's normal there), from the points around it in
/// the (r, h) plane within `cell`, when many angles agree.
fn profile_fits(rh: &[(DVec2, u8)], cell: f64) -> Vec<Option<(DVec2, f64, DVec2)>> {
    let mut hash: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
    for (i, (q, _)) in rh.iter().enumerate() {
        hash.entry(((q.x / cell).floor() as i64, (q.y / cell).floor() as i64)).or_default().push(i as u32);
    }
    let sig = cell * 0.5;
    rh.par_iter()
        .map(|(q, _)| {
            let (ci, cj) = ((q.x / cell).floor() as i64, (q.y / cell).floor() as i64);
            let mut pts = vec![];
            let mut nb = [false; 36];
            // a dense piece: one point in n is plenty for the local fit,
            // picked by a hash (the mesh's order follows the angle, a plain
            // stride would keep a few angles only)
            let total: usize = (-1..=1).flat_map(|di| (-1..=1).map(move |dj| (di, dj))).filter_map(|(di, dj)| hash.get(&(ci + di, cj + dj)).map(|l| l.len())).sum();
            let step = total.div_ceil(400).max(1) as u64;
            for di in -1..=1 {
                for dj in -1..=1 {
                    let Some(list) = hash.get(&(ci + di, cj + dj)) else { continue };
                    for &k in list.iter().filter(|&&k| step == 1 || ((k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40) % step == 0) {
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
        .collect()
}

/// The median distance to the profile, or why there is none.
fn judge(rh: &[(DVec2, u8)], fits: &[Option<(DVec2, f64, DVec2)>], p: &RevolveParams) -> Result<f64, String> {
    let mut dists: Vec<f64> = rh.iter().zip(fits).filter_map(|((q, _), f)| f.map(|(g, _, _)| (g - *q).length())).collect();
    if dists.len() < rh.len() / 3 {
        return Err("profile not found".into());
    }
    dists.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let thick = dists[dists.len() / 2];
    if thick > p.max_thick {
        return Err(format!("not round ({thick:.3} mm thick)"));
    }
    Ok(thick)
}

/// The radius by angle that makes a turn oval or off-centre (harmonics 1
/// and 2), from each point's distance to a coarse profile along its
/// normal: Δr(θ) = Σ cₖ cos kθ + sₖ sin kθ. None when it cannot be told.
fn oval_of(rh: &[(DVec2, u8)], ang: &[f64], coarse: &[Option<(DVec2, f64, DVec2)>]) -> Option<[f64; 4]> {
    let mut a = [[0.0f64; 4]; 4];
    let mut b = [0.0f64; 4];
    let mut n = 0usize;
    for (i, f) in coarse.iter().enumerate() {
        let Some((g, _, nrm)) = f else { continue };
        let q = rh[i].0;
        let t = ang[i];
        let row = [nrm.x * t.cos(), nrm.x * t.sin(), nrm.x * (2.0 * t).cos(), nrm.x * (2.0 * t).sin()];
        let y = (q - *g).dot(*nrm);
        for j in 0..4 {
            b[j] += row[j] * y;
            for k in 0..4 {
                a[j][k] += row[j] * row[k];
            }
        }
        n += 1;
    }
    if n < 60 {
        return None;
    }
    solve4(a, b)
}

fn solve4(mut a: [[f64; 4]; 4], mut b: [f64; 4]) -> Option<[f64; 4]> {
    for i in 0..4 {
        let piv = (i..4).max_by(|&x, &y| a[x][i].abs().partial_cmp(&a[y][i].abs()).unwrap())?;
        if a[piv][i].abs() < 1e-12 {
            return None;
        }
        a.swap(i, piv);
        b.swap(i, piv);
        for k in 0..4 {
            if k != i {
                let f = a[k][i] / a[i][i];
                for j in i..4 {
                    a[k][j] -= f * a[i][j];
                }
                b[k] -= f * b[i];
            }
        }
    }
    Some([b[0] / a[0][0], b[1] / a[1][1], b[2] / a[2][2], b[3] / a[3][3]])
}

/// The common profile of a piece around (c, a), or why there is none.
fn fit(base: &Mesh, topo: &Topo, verts: &[usize], c: DVec3, a: DVec3, r_max: f64, p: &RevolveParams) -> Result<Fit, String> {
    if verts.len() < 60 {
        return Err("too few vertices".into());
    }
    let u = a.any_orthonormal_vector();
    let w = a.cross(u);
    let mut ang = Vec::with_capacity(verts.len());
    let rh: Vec<(DVec2, u8)> = verts
        .iter()
        .map(|&v| {
            let x = base.pos[v] - c;
            let h = a.dot(x);
            let rad = x - a * h;
            let t = rad.dot(w).atan2(rad.dot(u));
            ang.push(t);
            let bin = (((t + std::f64::consts::PI) / std::f64::consts::TAU * 36.0) as i64).clamp(0, 35) as u8;
            (DVec2::new(rad.length(), h), bin)
        })
        .collect();
    // the turn must be (mostly) there
    let mut bins = [false; 36];
    for (q, b) in &rh {
        if q.x > r_max * 0.3 {
            bins[*b as usize] = true;
        }
    }
    let cover = bins.iter().filter(|b| **b).count() as f64 * 10.0;
    if cover < p.min_cover {
        return Err(format!("covers {cover:.0}° of the turn"));
    }
    // the neighbourhood in the (r, h) plane spans a few triangles at least
    let edge = {
        let mut e: Vec<f64> = verts.iter().take(400).filter_map(|&v| topo.verts_of(v).first().map(|&u| (base.pos[v] - base.pos[u as usize]).length())).collect();
        e.sort_by(|a, b| a.partial_cmp(b).unwrap());
        e.get(e.len() / 2).copied().unwrap_or(p.radius)
    };
    let cell = p.radius.max(edge * 2.5);
    let plain = profile_fits(&rh, cell);
    let plain_judge = judge(&rh, &plain, p);
    let strip = |f: Vec<Option<(DVec2, f64, DVec2)>>| -> Vec<Option<(DVec2, f64)>> { f.into_iter().map(|x| x.map(|(g, e, _)| (g, e))).collect() };
    if let Ok(t) = plain_judge {
        if t <= p.max_thick * 0.5 {
            return Ok(Fit { shift: vec![0.0; rh.len()], rh, fits: strip(plain), thick: t, oval: 0.0 });
        }
    }
    // an oval or off-centre turn (common on AI meshes): its radius by
    // angle taken out, the profile fitted again on the rounded points
    let mut shift = vec![0.0; rh.len()];
    // a slight oval only: a strongly flattened turn may be meant so
    let r_mid = {
        let mut r: Vec<f64> = rh.iter().map(|(q, _)| q.x).collect();
        r.sort_by(|a, b| a.partial_cmp(b).unwrap());
        r[r.len() / 2]
    };
    let amp_max = (p.max_move * 0.75).min(r_mid * 0.07);
    let mut best: Option<(f64, Vec<Option<(DVec2, f64, DVec2)>>, f64)> = None;
    let mut cur = rh.clone();
    for pass in 0..3 {
        let coarse = profile_fits(&cur, cell * if pass == 0 { 2.0 } else { 1.0 });
        let Some(k) = oval_of(&cur, &ang, &coarse) else { break };
        for (i, s) in shift.iter_mut().enumerate() {
            let t = ang[i];
            *s += k[0] * t.cos() + k[1] * t.sin() + k[2] * (2.0 * t).cos() + k[3] * (2.0 * t).sin();
        }
        let amp = shift.iter().fold(0.0f64, |m, s| m.max(s.abs()));
        if amp > amp_max {
            break;
        }
        cur = rh.iter().zip(&shift).map(|((q, b), s)| (DVec2::new(q.x - s, q.y), *b)).collect();
        let fits = profile_fits(&cur, cell);
        let Ok(t) = judge(&cur, &fits, p) else { continue };
        if best.as_ref().map(|b| t < b.0).unwrap_or(true) {
            best = Some((t, fits, amp));
        }
    }
    match (plain_judge, best) {
        (Ok(t0), Some((t1, fits, amp))) if t1 < t0 * 0.95 => Ok(Fit { rh, shift, fits: strip(fits), thick: t1, oval: amp }),
        (Ok(t0), _) => Ok(Fit { shift: vec![0.0; rh.len()], rh, fits: strip(plain), thick: t0, oval: 0.0 }),
        (Err(_), Some((t1, fits, amp))) => Ok(Fit { rh, shift, fits: strip(fits), thick: t1, oval: amp }),
        (Err(e), None) => Err(e),
    }
}

/// The axis of a surface of revolution from its points and normals: every
/// normal line meets the axis, so (Pottmann) the normals' lines belong to
/// a linear complex (c̄, c) with c̄·n + c·(x × n) = 0; c is the axis
/// direction, c × c̄ a point of it. Least squares, |c| = 1.
fn refine_axis(base: &Mesh, vn: &[DVec3], verts: &[usize], c0: DVec3, a0: DVec3) -> Option<(DVec3, DVec3)> {
    let o = c0;
    let mut am = [[0.0f64; 3]; 3];
    let mut bm = [[0.0f64; 3]; 3];
    let mut cm = [[0.0f64; 3]; 3];
    for &v in verts {
        let n = vn[v];
        if n.length_squared() < 0.5 {
            continue;
        }
        let x = base.pos[v] - o;
        let m = x.cross(n);
        let (nn, mm) = (n.to_array(), m.to_array());
        for i in 0..3 {
            for j in 0..3 {
                am[i][j] += nn[i] * nn[j];
                bm[i][j] += nn[i] * mm[j];
                cm[i][j] += mm[i] * mm[j];
            }
        }
    }
    let tr = am[0][0] + am[1][1] + am[2][2];
    if tr <= 0.0 {
        return None;
    }
    for (i, row) in am.iter_mut().enumerate() {
        row[i] += tr * 1e-6;
    }
    let ainv = glam::DMat3::from_cols_array_2d(&am).transpose().inverse();
    let b = glam::DMat3::from_cols_array_2d(&bm).transpose();
    let cmat = glam::DMat3::from_cols_array_2d(&cm).transpose();
    // S = C − Bᵀ A⁻¹ B, its smallest eigenvector is c
    let sm = cmat - b.transpose() * ainv * b;
    let rows = [sm.row(0).to_array(), sm.row(1).to_array(), sm.row(2).to_array()];
    let (vals, vecs) = crate::mesh::eigen_sym(rows);
    let k = (0..3).min_by(|&i, &j| vals[i].partial_cmp(&vals[j]).unwrap())?;
    let cdir = vecs[k].normalize_or_zero();
    if cdir == DVec3::ZERO {
        return None;
    }
    let cbar = -(ainv * (b * cdir));
    // c̄ = p × c + h c: p = c × c̄ (the point nearest to o)
    let p = cdir.cross(cbar);
    let _ = a0;
    Some((o + p, cdir))
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
fn profile_project(pts: &[(DVec2, f64)], q: DVec2) -> Option<(DVec2, f64, DVec2)> {
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
                    return Some((c + cc + v.normalize() * r, circ_rms, v.normalize()));
                }
            }
        }
    }
    Some((on_line, line_rms, nrm))
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
/// `base`: the mesh before the snap (the AI mesh): the profile is read on it
/// (the snap may have cut a turned lip into flats) and the piece's vertices
/// are put on it from there.
pub fn revolve(m: &mut Mesh, base: &Mesh, topo: &Topo, seg: &Segmentation, hints: &[Hint], off: &[DVec3], p: &RevolveParams) -> RevolveReport {
    let mut rep = RevolveReport::default();
    if !p.on {
        return rep;
    }
    let mut list = axes(base, seg, p);
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
    let grid = Grid::new(&base.pos, 1.0);
    let vn = base.vertex_normals(topo);
    let tn = base.normals();
    let mut done = vec![false; base.pos.len()];
    for ax in list {
        // the axis from the cylinders' fits can be off by tens of degrees on
        // a short piece: it is also found again from the normals of the
        // piece (each meets the axis), and the rounder of the two is kept
        let (c0, a0) = (ax.p, ax.a);
        let v0 = gather(base, &tn, topo, seg, &grid, &done, &ax, c0, a0, p);
        let mut tries = vec![(c0, a0, v0)];
        if p.refine && !ax.hint && tries[0].2.len() >= 60 {
            // from the cylinders' axis, and from the nearest axis of the
            // world when it is within 40° (parts are often modelled square)
            let mut starts = vec![a0];
            let e = [DVec3::X, DVec3::Y, DVec3::Z].into_iter().max_by(|x, y| x.dot(a0).abs().partial_cmp(&y.dot(a0).abs()).unwrap()).unwrap();
            let e = if e.dot(a0) < 0.0 { -e } else { e };
            let ang = e.dot(a0).clamp(-1.0, 1.0).acos().to_degrees();
            if ang > 1.0 && ang < 40.0 {
                starts.push(e);
            }
            for (k, &s0) in starts.iter().enumerate() {
                let (mut c, mut a) = (c0, s0);
                let mut verts = if k == 0 { tries[0].2.clone() } else { gather(base, &tn, topo, seg, &grid, &done, &ax, c, a, p) };
                if k > 0 && verts.len() >= 60 {
                    tries.push((c, a, verts.clone()));
                }
                for _ in 0..2 {
                    if verts.len() < 60 {
                        break;
                    }
                    let Some((c2, a2)) = refine_axis(base, &vn, &verts, c, a) else { break };
                    let a2 = if a2.dot(a) < 0.0 { -a2 } else { a2 };
                    if a2.dot(s0).clamp(-1.0, 1.0).acos().to_degrees() > 30.0 {
                        break;
                    }
                    // the new axis through its point nearest to the old centre
                    c = c2 + a2 * a2.dot(c - c2);
                    a = a2;
                    verts = gather(base, &tn, topo, seg, &grid, &done, &ax, c, a, p);
                }
                if (a.dot(s0) < 0.99995 || (c - c0).length() > 0.01) && verts.len() >= 60 {
                    tries.push((c, a, verts));
                }
            }
        }
        let mut best: Option<(DVec3, DVec3, Vec<usize>, Fit)> = None;
        let mut first_err: Option<(usize, String)> = None;
        let debug = std::env::var("RECTIFY_REV_DEBUG").ok().and_then(|s| {
            let v: Vec<f64> = s.split(',').filter_map(|t| t.trim().parse().ok()).collect();
            (v.len() == 4 && (DVec3::new(v[0], v[1], v[2]) - c0).length() < v[3]).then_some(())
        });
        for (c, a, verts) in tries {
            let res = fit(base, topo, &verts, c, a, ax.r_max, p);
            if debug.is_some() {
                eprintln!("REV try c {c:.2} a {a:.3} r {:.2} h {:.2?} verts {}: {:?}", ax.r_max, ax.h, verts.len(), res.as_ref().map(|f| (f.thick, f.oval)));
                if let Ok(dir) = std::env::var("RECTIFY_REV_DUMP") {
                    let u = a.any_orthonormal_vector();
                    let w = a.cross(u);
                    let mut out = String::new();
                    for &v in &verts {
                        let x = base.pos[v] - c;
                        let h = a.dot(x);
                        let rad = x - a * h;
                        out += &format!("{:.4} {:.4} {:.3}\n", rad.length(), h, rad.dot(w).atan2(rad.dot(u)));
                    }
                    let _ = std::fs::write(format!("{dir}/rev-{:.0}-{:.0}-{:.0}-{}.txt", c.x, c.y, c.z, (a.x * 100.0).round() as i64 * 10000 + (a.y * 100.0).round() as i64 * 100 + (a.z * 100.0).round() as i64), out);
                }
            }
            match res {
                Ok(f) => {
                    if best.as_ref().map(|b| f.thick < b.3.thick).unwrap_or(true) {
                        best = Some((c, a, verts, f));
                    }
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some((verts.len(), e));
                    }
                }
            }
        }
        let Some((c, a, mut verts, mut fitted)) = best else {
            let (n, e) = first_err.unwrap_or((0, "too few vertices".into()));
            rep.pieces.push(Piece { p: c0, a: a0, r_max: ax.r_max, h: ax.h, verts: n, moved: 0, details: 0, thick: 0.0, oval: 0.0, hint: ax.hint, skipped: Some(e) });
            continue;
        };
        // the cylinders only reach so far: a turned piece often goes on past
        // them (a muzzle's rounded lip, a button's rim), so it is grown while
        // its profile still holds
        let (mut r_max, mut hr) = (ax.r_max, ax.h);
        if p.grow > 1.0 && !ax.hint {
            // measured against the piece as first found, so it cannot drift
            let thick0 = fitted.thick;
            for _ in 0..2 {
                let r2 = (r_max * p.grow).min(p.max_radius + p.margin);
                if r2 <= r_max + 0.05 {
                    break;
                }
                let dh = (r2 - r_max) * 0.5;
                let big = Axis { p: ax.p, a: ax.a, members: ax.members.clone(), r_max: r2, h: (hr.0 - dh, hr.1 + dh), hint: false };
                let v2 = gather(base, &tn, topo, seg, &grid, &done, &big, c, a, p);
                if v2.len() <= verts.len() {
                    break;
                }
                let Ok(f2) = fit(base, topo, &v2, c, a, r2, p) else { break };
                if debug.is_some() {
                    eprintln!("REV grow r {r2:.2} verts {}: thick {:.4} oval {:.3}", v2.len(), f2.thick, f2.oval);
                }
                if f2.thick > thick0 * 1.25 + 0.003 {
                    break;
                }
                (r_max, hr, verts, fitted) = (r2, big.h, v2, f2);
            }
        }
        let Fit { rh, shift, fits, thick, oval } = fitted;
        let mut piece = Piece { p: c, a, r_max, h: hr, verts: verts.len(), moved: 0, details: 0, thick, oval, hint: ax.hint, skipped: None };
        for (i, &v) in verts.iter().enumerate() {
            let Some((g, _)) = fits[i] else { continue };
            let q = rh[i].0;
            // a detail stands off the rounded profile; the move itself
            // includes the rounding
            let off = (g - DVec2::new(q.x - shift[i], q.y)).length();
            if off > p.outlier || (g - q).length() > p.max_move {
                piece.details += 1;
                continue;
            }
            // fades out at the piece's rim and ends
            let fr = ((r_max - q.x) / p.margin).clamp(0.0, 1.0);
            let fh = ((q.y - hr.0).min(hr.1 - q.y) / p.margin).clamp(0.0, 1.0);
            let f = smooth(fr.min(fh));
            if f <= 0.0 || q.x < 1e-6 {
                continue;
            }
            let x = base.pos[v] - c;
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
        let base = m.clone();
        let rep = revolve(&mut m, &base, &topo, &seg, &[], &[], &p);
        let after = spread(&m);
        assert_eq!(rep.done(), 1, "{:?}", rep.pieces);
        assert!(after < before * 0.4, "{before} -> {after}");
    }
}
