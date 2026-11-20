//! Optional pass on a baked mesh: a pair bevel on every sharp edge, the way
//! Romain does it by hand in Maya (10 nov. 2026, his `Correctif.obj` in
//! `docs/references`): select the hard edges, Bevel with 2 segments and a
//! round profile, then let the lines of the faces around meet the corners of
//! the bevel on the diagonal.
//!
//! An edge is sharp when the exact surface folds across it (the gradient
//! taken just inside each of its two faces differs by more than
//! `SHARP_DEG`), so the rows of a fillet strip never count, however coarse.
//! Each vertex on sharp edges splits into one vertex per sector (the faces
//! between two sharp edges around it), set where the two offset lines of
//! the sector meet: on a straight run that is the offset itself, at a
//! reflex corner the mitre, which is where the lines of the face around now
//! arrive on the diagonal. Each sharp edge gets two rows of quads, the
//! middle row on a round profile. Where three or more sharp edges meet, the
//! hole between the rows is closed by one quad per sector around a centre
//! vertex (3 quads at a box corner, as Maya does).
//!
//! The SDF stays sharp: the bevel only exists in the exported mesh. A sharp
//! edge that stops in the middle of a smooth surface, or a face too narrow
//! for the width, returns `Err` and the mesh is left as it was.

use super::field::Field;
use super::{BFace, BakeStats, Baked, P_FILLET};
use crate::program::compile;
use crate::scene::Scene;
use glam::DVec3;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// Default width (mm): the bevel of Romain's reference.
pub const BEVEL_WIDTH: f64 = 0.25;
/// An edge is sharp when the surface folds across it by more than this.
pub const SHARP_DEG: f64 = 20.0;

/// `b` with a two-segment bevel of `width` on its sharp edges, stats
/// recomputed (`stats.bevel_edges` counts the bevelled edges).
pub fn bevel_sharp(scene: &Scene, b: &Baked, width: f64) -> Result<Baked, String> {
    let field = Field::new(&compile(scene));
    bevel(&field, b, width)
}

/// Middle of a round profile between `qa` and `qb`, two points on the faces
/// on both sides of an edge through `o` with direction `t`.
fn round_mid(o: DVec3, t: DVec3, qa: DVec3, qb: DVec3) -> DVec3 {
    let along = ((qa - o).dot(t) + (qb - o).dot(t)) * 0.5;
    let o = o + t * along;
    let a = (qa - o) - t * (qa - o).dot(t);
    let b = (qb - o) - t * (qb - o).dot(t);
    let (la, lb) = (a.length(), b.length());
    if la < 1e-12 || lb < 1e-12 {
        return o + (a + b) * 0.5;
    }
    let cos = (a.dot(b) / (la * lb)).clamp(-1.0, 1.0);
    let th = cos.acos();
    if th > 170f64.to_radians() {
        return o + (a + b) * 0.5;
    }
    let d = (la + lb) * 0.5;
    let u = (a / la + b / lb).normalize_or_zero();
    o + u * d * (1.0 - (th * 0.5).sin()) / (th * 0.5).cos()
}

/// Where the offset lines of two edges leaving `v` (directions `ta`, `tb`,
/// inward perpendiculars `pa`, `pb`, offset `w`) meet.
fn sector_point(v: DVec3, ta: DVec3, pa: DVec3, tb: DVec3, pb: DVec3, w: f64) -> DVec3 {
    let c = ta.dot(tb);
    let det = 1.0 - c * c;
    if det < 0.067 {
        // nearly straight through: the offset itself
        let p = (pa + pb).normalize_or_zero();
        return v + if p == DVec3::ZERO { pa * w } else { p * w };
    }
    let r = (pb - pa) * w;
    let (ra, rb) = (ta.dot(r), tb.dot(r));
    let al = (ra - c * rb) / det;
    let be = (c * ra - rb) / det;
    ((v + ta * al + pa * w) + (v + tb * be + pb * w)) * 0.5
}

/// Why one attempt failed: corners too narrow for their width (they get a
/// narrower bevel at the next attempt, as Maya's clamp), or a case not
/// handled.
enum Fail {
    Narrow(Vec<u32>, String),
    Other(String),
}

impl From<String> for Fail {
    fn from(s: String) -> Self {
        Fail::Other(s)
    }
}

impl From<&str> for Fail {
    fn from(s: &str) -> Self {
        Fail::Other(s.into())
    }
}

pub fn bevel(field: &Field, b: &Baked, w: f64) -> Result<Baked, String> {
    let t0 = Instant::now();
    // per-vertex width: where a face is too narrow, its corners on sharp
    // edges get 0.6 of their width at the next attempt, down to 15 %
    let mut wv = vec![w; b.positions.len()];
    loop {
        match bevel_with(field, b, w, &wv) {
            Ok(mut out) => {
                out.stats.bevel_clamped = wv.iter().filter(|&&x| x < w).count();
                out.stats.bevel_seconds = t0.elapsed().as_secs_f64();
                return Ok(out);
            }
            Err(Fail::Other(e)) => return Err(e),
            Err(Fail::Narrow(vs, e)) => {
                let mut changed = false;
                for v in vs {
                    if wv[v as usize] > w * 0.15 {
                        wv[v as usize] *= 0.6;
                        changed = true;
                    }
                }
                if !changed {
                    return Err(e);
                }
            }
        }
    }
}

fn bevel_with(field: &Field, b: &Baked, w: f64, wv: &[f64]) -> Result<Baked, Fail> {
    let pos = &b.positions;
    let faces = &b.faces;
    let n = |f: &BFace| f.verts.len();
    // directed edges: (a, b) -> (face, corner of a)
    let mut dir: HashMap<(u32, u32), (usize, usize)> = HashMap::new();
    for (fi, f) in faces.iter().enumerate() {
        for i in 0..n(f) {
            if dir.insert((f.verts[i], f.verts[(i + 1) % n(f)]), (fi, i)).is_some() {
                return Err("bevel : maillage non manifold".into());
            }
        }
    }
    if dir.keys().any(|&(a, c)| !dir.contains_key(&(c, a))) {
        return Err("bevel : maillage ouvert".into());
    }
    let centre = |fi: usize| {
        let f = &faces[fi];
        f.verts.iter().fold(DVec3::ZERO, |s, v| s + pos[*v as usize]) / n(f) as f64
    };
    let newell = |pts: &[DVec3]| {
        let mut s = DVec3::ZERO;
        for i in 0..pts.len() {
            s += pts[i].cross(pts[(i + 1) % pts.len()]);
        }
        s
    };
    let fnorm: Vec<DVec3> = faces.iter().map(|f| newell(&f.verts.iter().map(|v| pos[*v as usize]).collect::<Vec<_>>()).normalize_or_zero()).collect();
    // sharp edges: the exact surface folds across them
    let cos_sharp = SHARP_DEG.to_radians().cos();
    let mut sharp: HashSet<(u32, u32)> = HashSet::new();
    for (&(a, c), &(fa, _)) in &dir {
        if a > c {
            continue;
        }
        let fb = dir[&(c, a)].0;
        if fnorm[fa].dot(fnorm[fb]) > 10f64.to_radians().cos() {
            continue;
        }
        // at each end (on the surface, unlike the middle of a chord), the
        // gradient a step into each face: they differ across a fold only
        let side = |v: u32, fi: usize| {
            let p = pos[v as usize];
            let to_c = centre(fi) - p;
            let step = (to_c.length() * 0.25).min(0.01);
            field.grad(p + to_c.normalize_or_zero() * step).normalize_or_zero()
        };
        let folds = |v: u32| side(v, fa).dot(side(v, fb)) < cos_sharp;
        if folds(a) && folds(c) {
            sharp.insert((a, c));
        }
    }
    let is_sharp = |a: u32, c: u32| sharp.contains(&(a.min(c), a.max(c)));
    let on_sharp: HashSet<u32> = sharp.iter().flat_map(|&(a, c)| [a, c]).collect();
    // incident corners per vertex
    let mut inc: Vec<Option<(usize, usize)>> = vec![None; pos.len()];
    for (fi, f) in faces.iter().enumerate() {
        for i in 0..n(f) {
            inc[f.verts[i] as usize] = Some((fi, i));
        }
    }
    let mut positions = pos.clone();
    let mut vnormals: Vec<DVec3> = vec![DVec3::ZERO; pos.len()];
    // (face, corner) -> vertex of its sector
    let mut corner_map: HashMap<(usize, usize), u32> = HashMap::new();
    // (vertex, neighbour along a sharp edge) -> middle vertex of that edge's profile there
    let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
    let mut new_faces: Vec<BFace> = vec![];
    let mut corner_quads: Vec<([u32; 4], u32)> = vec![];
    let mut terminal = 0;
    // face -> (corner of v, first line vertex, mitre, last line vertex)
    let mut splits: HashMap<usize, (usize, u32, u32, u32)> = HashMap::new();
    // the quads that close a row where its sharp edge stops: (vertices,
    // normals, material, the edge of the face it takes over)
    let mut caps: Vec<(Vec<u32>, Vec<DVec3>, u32, (u32, u32))> = vec![];
    let add = |p: DVec3, nrm: DVec3, positions: &mut Vec<DVec3>, vnormals: &mut Vec<DVec3>| {
        positions.push(p);
        vnormals.push(nrm.normalize_or_zero());
        (positions.len() - 1) as u32
    };
    // the fan of faces around v, in order; cross[k] = neighbour across the
    // edge between fan[k] and fan[k + 1]
    let fan_at = |v: u32| -> Result<(Vec<(usize, usize)>, Vec<u32>), String> {
        let mut fan: Vec<(usize, usize)> = vec![];
        let mut cross: Vec<u32> = vec![];
        let Some(start) = inc[v as usize] else { return Ok((fan, cross)) };
        let mut cur = start;
        loop {
            let f = &faces[cur.0];
            let prev = f.verts[(cur.1 + n(f) - 1) % n(f)];
            fan.push(cur);
            cross.push(prev);
            let next = dir[&(v, prev)];
            if next == start {
                break;
            }
            cur = next;
            if fan.len() > 64 {
                return Err("bevel : éventail de sommet trop grand".into());
            }
        }
        Ok((fan, cross))
    };
    let onto_surface = |mut p: DVec3| {
        for _ in 0..2 {
            let d = field.dist(p);
            if d.abs() > w * 0.5 {
                break;
            }
            p -= field.grad(p).normalize_or_zero() * d;
        }
        p
    };
    // the angle a sector opens at v
    let opening_of = |v: u32, sec: &[(usize, usize)]| -> f64 {
        let vp = pos[v as usize];
        sec.iter()
            .map(|&(fi, ci)| {
                let f = &faces[fi];
                let a = (pos[f.verts[(ci + 1) % n(f)] as usize] - vp).normalize_or_zero();
                let b = (pos[f.verts[(ci + n(f) - 1) % n(f)] as usize] - vp).normalize_or_zero();
                a.dot(b).clamp(-1.0, 1.0).acos()
            })
            .sum()
    };
    // a sector of three faces cut on the diagonal: the vertex goes onto each
    // of the two lines inside the sector, at the width from its sharp edge
    // (`ta`, `tb`), and the middle face is cut from its far corner to the
    // mitre between them
    let split_points = |v: u32, sec: &[(usize, usize)], ta: DVec3, tb: DVec3| -> Option<(DVec3, DVec3, DVec3, bool)> {
        if sec.len() != 3 || n(&faces[sec[1].0]) != 4 {
            return None;
        }
        let vp = pos[v as usize];
        let (f1, i1) = sec[1];
        let on_line = |l: u32, t: DVec3| -> Option<DVec3> {
            let d = pos[l as usize] - vp;
            let tl = d.normalize_or_zero();
            let sin = tl.cross(t).length();
            let at = wv[v as usize] / sin.max(1e-9);
            (sin > 0.2 && at < d.length() * 0.95).then(|| onto_surface(vp + tl * at))
        };
        let pa = on_line(faces[f1].verts[(i1 + 1) % 4], ta)?;
        let pb = on_line(faces[f1].verts[(i1 + 3) % 4], tb)?;
        let x = onto_surface(pa + pb - vp);
        // fair when the two halves of the middle face are real quads: on a
        // long narrow face the diagonal runs nearly along it (a triangle with
        // a flat corner), and the mitre is better there if it fits
        let q = |k: usize| pos[faces[f1].verts[(i1 + k) % 4] as usize];
        let fair = |pts: [DVec3; 4]| {
            (0..4).all(|k| {
                let (a, c) = ((pts[(k + 1) % 4] - pts[k]).normalize_or_zero(), (pts[(k + 3) % 4] - pts[k]).normalize_or_zero());
                (12.0..=170.0).contains(&a.dot(c).clamp(-1.0, 1.0).acos().to_degrees())
            })
        };
        Some((pa, x, pb, fair([x, pa, q(1), q(2)]) && fair([q(2), q(3), pb, x])))
    };
    // Romain's rule: at a reflex corner whose two sharp edges carry on as
    // lines of the face, those lines stay straight and the face between them
    // is cut on the diagonal; elsewhere the lines of a sector all meet at its
    // mitre
    let reflex_cross = |v: u32, sec: &[(usize, usize)]| sec.len() == 3 && opening_of(v, sec) > std::f64::consts::PI * 1.25;
    // where every corner on a sharp edge would go with the plain rule (its
    // sector's mitre), so that a face is checked with all its corners moved
    let mut tentative: HashMap<(usize, usize), DVec3> = HashMap::new();
    // does moving only corner `ci` of face `fi` to `p` fold it?
    let folds_alone = |fi: usize, ci: usize, p: DVec3| {
        let f = &faces[fi];
        let m = n(f);
        let pts: Vec<DVec3> = (0..m).map(|k| if k == ci { p } else { pos[f.verts[k] as usize] }).collect();
        (0..m).any(|k| (pts[(k + 1) % m] - pts[k]).cross(pts[(k + m - 1) % m] - pts[k]).dot(fnorm[fi]) <= 0.0)
    };
    for v in 0..pos.len() as u32 {
        let (fan, cross) = fan_at(v)?;
        let vp = pos[v as usize];
        let sharp_at: Vec<usize> = (0..fan.len()).filter(|&i| is_sharp(v, cross[i])).collect();
        if sharp_at.len() < 2 {
            continue;
        }
        for (j, &s) in sharp_at.iter().enumerate() {
            let e = sharp_at[(j + 1) % sharp_at.len()];
            // the sector after the sharp edge at s, up to the one at e
            let m = fan.len();
            let sec: Vec<(usize, usize)> = (1..=((e + m - s - 1) % m) + 1).map(|d| fan[(s + d) % m]).collect();
            let (fa, ia) = sec[0];
            let (fb, ib) = *sec.last().unwrap();
            let ta = (pos[faces[fa].verts[(ia + 1) % n(&faces[fa])] as usize] - vp).normalize_or_zero();
            let tb = (pos[faces[fb].verts[(ib + n(&faces[fb]) - 1) % n(&faces[fb])] as usize] - vp).normalize_or_zero();
            let inward = |fi: usize, t: DVec3| {
                let d = centre(fi) - vp;
                (d - t * d.dot(t)).normalize_or_zero()
            };
            let p = onto_surface(sector_point(vp, ta, inward(fa, ta), tb, inward(fb, tb), wv[v as usize]));
            if reflex_cross(v, &sec) {
                if let Some((pa, x, pb, fair)) = split_points(v, &sec, ta, tb) {
                    if fair || sec.iter().any(|&(fi, ci)| folds_alone(fi, ci, p)) {
                        tentative.insert(sec[0], pa);
                        tentative.insert(sec[1], x);
                        tentative.insert(sec[2], pb);
                        continue;
                    }
                }
            }
            for c in sec {
                tentative.insert(c, p);
            }
        }
    }
    for v in 0..pos.len() as u32 {
        let (mut fan, mut cross) = fan_at(v)?;
        if fan.is_empty() {
            continue;
        }
        let k = cross.iter().filter(|&&p| is_sharp(v, p)).count();
        if k == 0 {
            continue;
        }
        if k == 1 {
            // the sharp edge stops here (into a fillet, or where the fold
            // fades out): its two rows end on the vertex, each closed by a
            // quad fanned to the next line of the face on its side
            let s0 = cross.iter().position(|&p| is_sharp(v, p)).unwrap();
            let m = fan.len();
            fan.rotate_left((s0 + 1) % m);
            if m < 3 {
                terminal += 1;
                continue;
            }
            let vp = pos[v as usize];
            let (f0, i0) = fan[0];
            let (fl, il) = fan[m - 1];
            let (n0, nl) = (n(&faces[f0]), n(&faces[fl]));
            let u = faces[f0].verts[(i0 + 1) % n0];
            let a1 = faces[f0].verts[(i0 + n0 - 1) % n0];
            let b1 = faces[fl].verts[(il + 1) % nl];
            let te = (pos[u as usize] - vp).normalize_or_zero();
            // on the bisector of the face's corner, at the width from the edge
            let side_point = |a: u32| -> Option<DVec3> {
                let ta = (pos[a as usize] - vp).normalize_or_zero();
                let th = te.dot(ta).clamp(-1.0, 1.0).acos();
                let bis = (te + ta).normalize_or_zero();
                if bis == DVec3::ZERO || th < 0.05 {
                    return None;
                }
                Some(onto_surface(vp + bis * (wv[v as usize] / (th * 0.5).sin())))
            };
            let (Some(ps), Some(pe)) = (side_point(a1), side_point(b1)) else {
                terminal += 1;
                continue;
            };
            let ns = faces[f0].normals.get(i0).copied().unwrap_or(fnorm[f0]);
            let ne = faces[fl].normals.get(il).copied().unwrap_or(fnorm[fl]);
            let qs = add(ps, ns, &mut positions, &mut vnormals);
            let qe = add(pe, ne, &mut positions, &mut vnormals);
            let mm = add(round_mid(vp, te, ps, pe), ns + ne, &mut positions, &mut vnormals);
            corner_map.insert((f0, i0), qs);
            corner_map.insert((fl, il), qe);
            mid.insert((v, u), mm);
            // the normal of the smooth side at v, from the faces beyond
            let nv = fan[1..m - 1].iter().fold(DVec3::ZERO, |acc, &(fi, ci)| acc + faces[fi].normals.get(ci).copied().unwrap_or(fnorm[fi]));
            let na1 = faces[f0].normals.get((i0 + n0 - 1) % n0).copied().unwrap_or(fnorm[f0]);
            let nb1 = faces[fl].normals.get((il + 1) % nl).copied().unwrap_or(fnorm[fl]);
            caps.push((vec![qs, a1, v, mm], vec![ns, na1, nv.normalize_or_zero(), vnormals[mm as usize]], faces[f0].mat, (a1, v)));
            caps.push((vec![v, b1, qe, mm], vec![nv.normalize_or_zero(), nb1, ne, vnormals[mm as usize]], faces[fl].mat, (v, b1)));
            continue;
        }
        // rotate so that the fan starts right after a sharp edge
        let s0 = cross.iter().position(|&p| is_sharp(v, p)).unwrap();
        let m = fan.len();
        fan.rotate_left((s0 + 1) % m);
        cross.rotate_left((s0 + 1) % m);
        // sectors: runs of faces between two sharp edges
        let mut sectors: Vec<Vec<(usize, usize)>> = vec![vec![]];
        for i in 0..m {
            sectors.last_mut().unwrap().push(fan[i]);
            if is_sharp(v, cross[i]) && i + 1 < m {
                sectors.push(vec![]);
            }
        }
        let vp = pos[v as usize];
        // does moving corner `ci` of face `fi` to `p` fold the face?
        let folds_at = |fi: usize, ci: usize, p: DVec3| {
            let f = &faces[fi];
            let m = n(f);
            // the neighbours along a sharp edge move with v
            let pts: Vec<DVec3> = (0..m)
                .map(|k| {
                    if k == ci {
                        p
                    } else if (k == (ci + 1) % m || k == (ci + m - 1) % m) && is_sharp(v, f.verts[k]) {
                        tentative.get(&(fi, k)).copied().unwrap_or(pos[f.verts[k] as usize])
                    } else {
                        pos[f.verts[k] as usize]
                    }
                })
                .collect();
            (0..m).any(|k| (pts[(k + 1) % m] - pts[k]).cross(pts[(k + m - 1) % m] - pts[k]).dot(fnorm[fi]) <= 0.0)
        };
        // per sector, the chain of vertices that replace v there, from the
        // side of its first sharp edge to the side of its last one: one
        // vertex, or three when its middle face is split on the diagonal
        let mut chains: Vec<Vec<(u32, DVec3, DVec3)>> = vec![]; // (id, position, normal)
        for sec in &sectors {
            let (fa, ia) = sec[0];
            let (fb, ib) = *sec.last().unwrap();
            let na = faces[fa].verts[(ia + 1) % n(&faces[fa])];
            let nb = faces[fb].verts[(ib + n(&faces[fb]) - 1) % n(&faces[fb])];
            let ta = (pos[na as usize] - vp).normalize_or_zero();
            let tb = (pos[nb as usize] - vp).normalize_or_zero();
            let inward = |fi: usize, t: DVec3| {
                let d = centre(fi) - vp;
                (d - t * d.dot(t)).normalize_or_zero()
            };
            let nrm = sec.iter().fold(DVec3::ZERO, |s, &(fi, ci)| s + faces[fi].normals.get(ci).copied().unwrap_or(fnorm[fi])).normalize_or_zero();
            let p = onto_surface(sector_point(vp, ta, inward(fa, ta), tb, inward(fb, tb), wv[v as usize]));
            let opening = opening_of(v, sec);
            let reflex_cross = reflex_cross(v, sec);
            if !reflex_cross && !sec.iter().any(|&(fi, ci)| folds_at(fi, ci, p)) {
                let id = add(p, nrm, &mut positions, &mut vnormals);
                for &c in sec {
                    corner_map.insert(c, id);
                }
                chains.push(vec![(id, p, nrm)]);
                continue;
            }
            // the mitre falls beyond a small face of the sector. With three
            // faces, the vertex goes onto each of the two lines inside the
            // sector, at the width from its sharp edge, and the middle face
            // is cut on the diagonal from its far corner to the mitre.
            // A split whose halves are thin triangles (a long narrow middle
            // face) gives way to the mitre at a reflex corner when the mitre
            // fits; Romain keeps it where nothing else fits (the U of Cube 3
            // on the top, his reference).
            let split = split_points(v, sec, ta, tb).filter(|&(pa, x, pb, _)| {
                let (f1, i1) = sec[1];
                let l1 = faces[f1].verts[(i1 + 1) % 4];
                let l2 = faces[f1].verts[(i1 + 3) % 4];
                !folds_at(fa, ia, pa) && !folds_at(fb, ib, pb) && {
                    // the two halves of the middle face
                    let c = pos[faces[f1].verts[(i1 + 2) % 4] as usize];
                    let quad_ok = |q: [DVec3; 4]| (0..4).all(|k| (q[(k + 1) % 4] - q[k]).cross(q[(k + 3) % 4] - q[k]).dot(fnorm[f1]) > 0.0);
                    quad_ok([x, pa, pos[l1 as usize], c]) && quad_ok([c, pos[l2 as usize], pb, x])
                }
            });
            let mitre_fits = !sec.iter().any(|&(fi, ci)| folds_at(fi, ci, p));
            if reflex_cross && mitre_fits && !split.is_some_and(|s| s.3) {
                let id = add(p, nrm, &mut positions, &mut vnormals);
                for &c in sec {
                    corner_map.insert(c, id);
                }
                chains.push(vec![(id, p, nrm)]);
                continue;
            }
            if let Some((pa, x, pb, _)) = split {
                let (f1, i1) = sec[1];
                {
                    let ida = add(pa, nrm, &mut positions, &mut vnormals);
                    let idx = add(x, nrm, &mut positions, &mut vnormals);
                    let idb = add(pb, nrm, &mut positions, &mut vnormals);
                    corner_map.insert((fa, ia), ida);
                    corner_map.insert((fb, ib), idb);
                    splits.insert(f1, (i1, ida, idx, idb));
                    chains.push(vec![(ida, pa, nrm), (idx, x, nrm), (idb, pb, nrm)]);
                    continue;
                }
            }
            // two faces: the vertex slides along the line between them (as
            // Maya does), where it lies about the width from both sharp edges
            if sec.len() == 2 {
                let (f0, i0) = sec[0];
                let l = faces[f0].verts[(i0 + n(&faces[f0]) - 1) % n(&faces[f0])];
                let d = pos[l as usize] - vp;
                let tl = d.normalize_or_zero();
                let (sa, sb) = (tl.cross(ta).length(), tl.cross(tb).length());
                let at = 2.0 * wv[v as usize] / (sa + sb).max(1e-9);
                if sa.min(sb) > 0.2 && at < d.length() * 0.95 {
                    let p = onto_surface(vp + tl * at);
                    if !sec.iter().any(|&(fi, ci)| folds_at(fi, ci, p)) {
                        let id = add(p, nrm, &mut positions, &mut vnormals);
                        for &c in sec {
                            corner_map.insert(c, id);
                        }
                        chains.push(vec![(id, p, nrm)]);
                        continue;
                    }
                }
            }
            // nothing fits: a narrower bevel here and at the corners next to
            // it along the sharp edges, at the next attempt
            if std::env::var("DEBUG_BEVEL").is_ok() {
                eprintln!("bevel : secteur de {} faces en ({:.3}, {:.3}, {:.3}), ouverture {:.1}°, largeur {:.3}", sec.len(), vp.x, vp.y, vp.z, opening.to_degrees(), wv[v as usize]);
                for &(fi, ci) in sec {
                    let f = &faces[fi];
                    eprintln!("  coin {ci} pli {} : {:?}", folds_at(fi, ci, p), f.verts.iter().map(|&v| pos[v as usize]).collect::<Vec<_>>());
                }
            }
            let mut vs = vec![v];
            vs.extend(cross.iter().copied().filter(|&c| is_sharp(v, c)));
            return Err(Fail::Narrow(vs, format!("bevel : {w:.2} mm trop large pour une face près de ({:.2}, {:.2}, {:.2})", vp.x, vp.y, vp.z)));
        }
        // sharp edge i (between sector i - 1 and sector i) leads to its
        // neighbour; sector i starts with that edge
        let ks = sectors.len();
        let edge_nb: Vec<u32> = sectors.iter().map(|sec| faces[sec[0].0].verts[(sec[0].1 + 1) % n(&faces[sec[0].0])]).collect();
        let mat = faces[sectors[0][0].0].mat;
        if ks == 2 {
            // straight run: one middle vertex shared by both edges
            let t = ((pos[edge_nb[0] as usize] - vp).normalize_or_zero() - (pos[edge_nb[1] as usize] - vp).normalize_or_zero()).normalize_or_zero();
            let side = |c: &Vec<(u32, DVec3, DVec3)>| (c[0].1 + c[c.len() - 1].1) * 0.5;
            let p = round_mid(vp, t, side(&chains[0]), side(&chains[1]));
            let id = add(p, chains[0][0].2 + chains[1][0].2, &mut positions, &mut vnormals);
            mid.insert((v, edge_nb[0]), id);
            mid.insert((v, edge_nb[1]), id);
            // a split sector leaves a quad between the two rows
            for c in &chains {
                if c.len() == 3 {
                    corner_quads.push(([id, c[0].0, c[1].0, c[2].0], mat));
                }
            }
        } else {
            let mut ring: Vec<(u32, DVec3)> = vec![];
            for i in 0..ks {
                let t = (pos[edge_nb[i] as usize] - vp).normalize_or_zero();
                let before = chains[(i + ks - 1) % ks].last().unwrap();
                let after = chains[i][0];
                let p = round_mid(vp, t, before.1, after.1);
                let id = add(p, before.2 + after.2, &mut positions, &mut vnormals);
                mid.insert((v, edge_nb[i]), id);
                ring.push((id, p));
                for c in &chains[i] {
                    ring.push((c.0, c.1));
                }
            }
            // a fan of quads around a centre; middles (and mitres) on the
            // even places of the ring, the sector vertices on the odd ones
            let l = ring.len();
            let mm = ring.iter().step_by(2).fold(DVec3::ZERO, |s, r| s + r.1) / (l / 2) as f64;
            let mq = ring.iter().skip(1).step_by(2).fold(DVec3::ZERO, |s, r| s + r.1) / (l / 2) as f64;
            let cn = ring.iter().step_by(2).fold(DVec3::ZERO, |s, r| s + vnormals[r.0 as usize]);
            let c = add(mm + (mm - mq) * 0.77, cn, &mut positions, &mut vnormals);
            for t in 0..l / 2 {
                corner_quads.push(([ring[2 * t].0, ring[2 * t + 1].0, ring[(2 * t + 2) % l].0, c], mat));
            }
        }
    }
    if terminal > 0 {
        return Err(Fail::Other(format!("bevel : {terminal} arête(s) vive(s) qui s'arrêtent en pointe : pas encore géré")));
    }
    // the faces, their corners moved to their sector's vertex
    let mut out_faces: Vec<BFace> = vec![];
    for (fi, f) in faces.iter().enumerate() {
        let verts: Vec<u32> = (0..n(f)).map(|i| corner_map.get(&(fi, i)).copied().unwrap_or(f.verts[i])).collect();
        // a face cut on the diagonal: [v, l1, c, l2] becomes [x, pa, l1, c]
        // and [c, l2, pb, x]
        let parts: Vec<(Vec<u32>, Vec<DVec3>)> = match splits.get(&fi) {
            Some(&(i1, pa, x, pb)) => {
                let k = |d: usize| (i1 + d) % 4;
                let nv = f.normals.get(i1).copied().unwrap_or(fnorm[fi]);
                let nr = |d: usize| f.normals.get(k(d)).copied().unwrap_or(fnorm[fi]);
                vec![
                    (vec![x, pa, verts[k(1)], verts[k(2)]], vec![nv, nv, nr(1), nr(2)]),
                    (vec![verts[k(2)], verts[k(3)], pb, x], vec![nr(2), nr(3), nv, nv]),
                ]
            }
            None => vec![(verts, f.normals.clone())],
        };
        for (verts, normals) in parts {
            let before = fnorm[fi];
            let pts: Vec<DVec3> = verts.iter().map(|v| positions[*v as usize]).collect();
            let after = newell(&pts);
            let m = pts.len();
            let folded = (0..m).any(|k| (pts[(k + 1) % m] - pts[k]).cross(pts[(k + m - 1) % m] - pts[k]).dot(before) <= 0.0);
            if after.length() < 1e-12 || after.normalize().dot(before) < 0.3 || folded {
                let c = centre(fi);
                let vs = f.verts.iter().copied().filter(|&v| on_sharp.contains(&v)).collect();
                return Err(Fail::Narrow(vs, format!("bevel : {w:.2} mm trop large pour une face près de ({:.2}, {:.2}, {:.2})", c.x, c.y, c.z)));
            }
            out_faces.push(BFace { verts, normals, mat: f.mat, patch: f.patch });
        }
    }
    // two rows along each sharp edge
    let mut nedges = 0;
    for &(a, c) in &sharp {
        nedges += 1;
        let (fa, ia) = dir[&(a, c)];
        let (fb, ib) = dir[&(c, a)];
        let na = n(&faces[fa]);
        let nb = n(&faces[fb]);
        let qa_a = corner_map[&(fa, ia)];
        let qc_a = corner_map[&(fa, (ia + 1) % na)];
        let qc_b = corner_map[&(fb, ib)];
        let qa_b = corner_map[&(fb, (ib + 1) % nb)];
        let (ma, mc) = (mid[&(a, c)], mid[&(c, a)]);
        new_faces.push(BFace { verts: vec![qc_a, qa_a, ma, mc], normals: vec![], mat: faces[fa].mat, patch: P_FILLET });
        new_faces.push(BFace { verts: vec![qa_b, qc_b, mc, ma], normals: vec![], mat: faces[fb].mat, patch: P_FILLET });
    }
    // corner quads, turned to match the rows around them
    let mut used: HashSet<(u32, u32)> = HashSet::new();
    for f in &new_faces {
        for i in 0..4 {
            used.insert((f.verts[i], f.verts[(i + 1) % 4]));
        }
    }
    for (mut qv, mat) in corner_quads {
        if used.contains(&(qv[0], qv[1])) || used.contains(&(qv[1], qv[2])) {
            qv.reverse();
        }
        new_faces.push(BFace { verts: qv.to_vec(), normals: vec![], mat, patch: P_FILLET });
    }
    for f in &mut new_faces {
        f.normals = f.verts.iter().map(|v| vnormals[*v as usize]).collect();
    }
    // two sharp edges that stop at both ends of one edge of a face (the
    // sides of a groove fading into a blend): their caps would both take
    // that edge, so the band between it and the face is made of two quads
    // along it, from the edge to the middles, then from the middles to the
    // face
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (k, c) in caps.iter().enumerate() {
        by_edge.entry(c.3).or_default().push(k);
    }
    for (k, (verts, normals, mat, e)) in caps.iter().enumerate() {
        let pair = &by_edge[e];
        if pair.len() == 1 {
            new_faces.push(BFace { verts: verts.clone(), normals: normals.clone(), mat: *mat, patch: P_FILLET });
            continue;
        }
        if pair.len() > 2 || k != pair[0] {
            if pair.len() > 2 {
                let p = positions[e.0 as usize];
                return Err(Fail::Other(format!("bevel : arêtes vives qui s'arrêtent en pointe près de ({:.2}, {:.2}, {:.2}) : pas encore géré", p.x, p.y, p.z)));
            }
            continue;
        }
        // the cap after the edge [x, y, qe_x, mm_x] and the one before it
        // [qs_y, x, y, mm_y]
        let (c0, c1) = (&caps[pair[0]], &caps[pair[1]]);
        let (b, a) = if c0.0[0] == e.0 { (c0, c1) } else { (c1, c0) };
        let (x, y) = *e;
        let (qe_x, mm_x, qs_y, mm_y) = (b.0[2], b.0[3], a.0[0], a.0[3]);
        new_faces.push(BFace { verts: vec![x, y, mm_y, mm_x], normals: vec![b.1[0], a.1[2], a.1[3], b.1[3]], mat: *mat, patch: P_FILLET });
        new_faces.push(BFace { verts: vec![mm_x, mm_y, qs_y, qe_x], normals: vec![b.1[3], a.1[3], a.1[0], b.1[2]], mat: *mat, patch: P_FILLET });
    }
    for f in &new_faces {
        let pts: Vec<DVec3> = f.verts.iter().map(|v| positions[*v as usize]).collect();
        if newell(&pts).length() < 1e-14 {
            return Err(Fail::Other(format!("bevel : rangée dégénérée près de ({:.2}, {:.2}, {:.2})", pts[0].x, pts[0].y, pts[0].z)));
        }
    }
    out_faces.extend(new_faces);
    let mut seen: HashMap<(u32, u32), usize> = HashMap::new();
    for (k, f) in out_faces.iter().enumerate() {
        for i in 0..f.verts.len() {
            if let Some(o) = seen.insert((f.verts[i], f.verts[(i + 1) % f.verts.len()]), k) {
                let p = positions[f.verts[i] as usize];
                if std::env::var("DEBUG_BEVEL").is_ok() {
                    for g in [o, k] {
                        let h = &out_faces[g];
                        eprintln!("  face {g} ({}) : {:?}", h.patch, h.verts.iter().map(|&v| (v, positions[v as usize])).collect::<Vec<_>>());
                    }
                }
                return Err(Fail::Other(format!("bevel : faces mal orientées près de ({:.2}, {:.2}, {:.2})", p.x, p.y, p.z)));
            }
        }
    }
    // drop the vertices no face uses any more
    let mut remap = vec![u32::MAX; positions.len()];
    let mut kept = vec![];
    for f in &mut out_faces {
        for v in &mut f.verts {
            if remap[*v as usize] == u32::MAX {
                remap[*v as usize] = kept.len() as u32;
                kept.push(positions[*v as usize]);
            }
            *v = remap[*v as usize];
        }
    }
    let mut out = Baked { positions: kept, faces: out_faces, stats: BakeStats::default() };
    let s = &b.stats;
    let st = &mut out.stats;
    st.patches = s.patches;
    st.seconds = s.seconds;
    st.quad_size = s.quad_size;
    st.max_dev = s.max_dev;
    st.mean_dev = s.mean_dev;
    st.flat_loops = s.flat_loops;
    st.faces_before_flat = s.faces_before_flat;
    st.flat_seconds = s.flat_seconds;
    st.verts = out.positions.len();
    for f in &out.faces {
        match f.verts.len() {
            3 => st.tris += 1,
            4 => st.quads += 1,
            _ => st.ngons += 1,
        }
    }
    let mut und: HashMap<(u32, u32), u32> = HashMap::new();
    for f in &out.faces {
        for i in 0..f.verts.len() {
            let (a, c) = (f.verts[i], f.verts[(i + 1) % f.verts.len()]);
            *und.entry((a.min(c), a.max(c))).or_default() += 1;
        }
    }
    st.open_edges = und.values().filter(|&&c| c == 1).count();
    st.nonmanifold_edges = und.values().filter(|&&c| c > 2).count();
    st.bevel_edges = nedges;
    Ok(out)
}
