//! Global regularization (in the spirit of GlobFit, Li et al. 2011): the
//! shapes found one by one are made to agree. Directions that are almost the
//! same become the same (parallel planes, cylinders on the same axis
//! direction), the object's own axes are exact, almost equal radii become
//! equal, almost coplanar planes coplanar, almost coaxial cylinders coaxial,
//! and each shape and its mirror image become exact mirrors.

use crate::prim::{fit_cylinder, fit_plane_dir, fit_sphere, Kind, Prim};
use crate::segment::{Ctx, Segmentation};
use crate::symmetry::Mirror;
use crate::mesh::{Mesh, Topo};
use glam::DVec3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegParams {
    /// Directions closer than this become one (degrees).
    pub angle: f64,
    /// Offsets / axis lines closer than this become one (mm).
    pub offset: f64,
    /// Radii closer than this share one (mm, and relative).
    pub radius_abs: f64,
    pub radius_rel: f64,
    pub axes: bool,
    pub mirror: bool,
    /// A shape that moved away from its faces more than this (mm, RMS)
    /// goes back to its own fit: the rule did not hold there.
    pub max_rms: f64,
}

impl Default for RegParams {
    fn default() -> Self {
        RegParams { angle: 4.0, offset: 0.15, radius_abs: 0.06, radius_rel: 0.04, axes: true, mirror: true, max_rms: 0.12 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RegReport {
    /// The object's frame (X, Y, Z = mirror normal).
    pub frame: Option<[DVec3; 3]>,
    pub directions: usize,
    pub on_axes: usize,
    pub coplanar: usize,
    pub radii: usize,
    pub coaxial: usize,
    pub mirrored: usize,
    /// Shapes the rules pulled too far, put back.
    pub reverted: usize,
}

struct Item {
    region: usize,
    dir: DVec3,
    w: f64,
    cluster: usize,
}

/// The two in-plane axes: the direction (square to `z`) that most plane
/// normals and cylinder axes line up with, modulo 90°.
fn frame_of(z: DVec3, dirs: &[(DVec3, f64)]) -> [DVec3; 3] {
    let u = z.any_orthonormal_vector();
    let v = z.cross(u);
    let mut cands = vec![];
    for (d, w) in dirs {
        if d.dot(z).abs() > 0.17 {
            continue;
        }
        let ang = d.dot(v).atan2(d.dot(u)).rem_euclid(std::f64::consts::FRAC_PI_2);
        cands.push((ang, *w));
    }
    let quarter = std::f64::consts::FRAC_PI_2;
    let diff = |a: f64, b: f64| {
        let d = (a - b).rem_euclid(quarter);
        d.min(quarter - d)
    };
    let mut best = (0.0, -1.0);
    for &(a, _) in &cands {
        let s: f64 = cands.iter().filter(|(b, _)| diff(a, *b) < 0.05).map(|(_, w)| w).sum();
        if s > best.1 {
            best = (a, s);
        }
    }
    let mut ang = best.0;
    // refined: weighted mean of the close ones
    let (mut sum, mut w) = (0.0, 0.0);
    for &(b, wb) in &cands {
        if diff(ang, b) < 0.05 {
            let mut d = (b - ang).rem_euclid(quarter);
            if d > quarter * 0.5 {
                d -= quarter;
            }
            sum += d * wb;
            w += wb;
        }
    }
    if w > 0.0 {
        ang += sum / w;
    }
    let x = u * ang.cos() + v * ang.sin();
    [x, z.cross(x), z]
}

pub fn regularize(m: &Mesh, topo: &Topo, seg: &mut Segmentation, mir: Option<&Mirror>, p: &RegParams) -> RegReport {
    let ctx = Ctx::new(m, topo);
    let before: Vec<(Prim, f64)> = seg.regions.iter().map(|r| (r.prim, r.rms)).collect();
    let cos = p.angle.to_radians().cos();
    let sin = p.angle.to_radians().sin();
    let mut rep = RegReport::default();
    // ------------------------------------------------ directions
    let mut items: Vec<Item> = vec![];
    for (i, r) in seg.regions.iter().enumerate() {
        if r.locked || r.faces.is_empty() {
            continue;
        }
        let dir = match r.prim {
            Prim::Plane { n, .. } => n,
            Prim::Cylinder { a, .. } => a,
            Prim::Sphere { .. } => continue,
        };
        items.push(Item { region: i, dir, w: r.area, cluster: usize::MAX });
    }
    items.sort_by(|a, b| b.w.partial_cmp(&a.w).unwrap());
    let mut clusters: Vec<(DVec3, f64)> = vec![];
    for it in items.iter_mut() {
        let mut found = None;
        for (k, (d, _)) in clusters.iter().enumerate() {
            if d.dot(it.dir).abs() > cos {
                found = Some(k);
                break;
            }
        }
        match found {
            Some(k) => {
                let (d, w) = clusters[k];
                let s = if d.dot(it.dir) >= 0.0 { it.dir } else { -it.dir };
                clusters[k] = ((d * w + s * it.w).normalize(), w + it.w);
                it.cluster = k;
            }
            None => {
                it.cluster = clusters.len();
                clusters.push((it.dir, it.w));
            }
        }
    }
    rep.directions = clusters.len();
    // the frame: Z the mirror normal (else the heaviest direction)
    let z = mir.map(|m| m.n).or_else(|| clusters.first().map(|c| c.0));
    if let (Some(z), true) = (z, p.axes) {
        let f = frame_of(z, &clusters);
        rep.frame = Some(f);
        for c in clusters.iter_mut() {
            if let Some(ax) = f.iter().find(|ax| ax.dot(c.0).abs() > cos) {
                c.0 = if ax.dot(c.0) > 0.0 { *ax } else { -*ax };
                rep.on_axes += 1;
            } else if c.0.dot(z).abs() < sin {
                // square to the mirror normal: stays in the mirror's family
                c.0 = (c.0 - z * c.0.dot(z)).normalize();
            }
        }
    }
    // mirror pairs of directions
    if let (Some(mr), true) = (mir, p.mirror) {
        for i in 0..clusters.len() {
            let m = mr.apply_dir(clusters[i].0);
            if m.dot(clusters[i].0).abs() > 1.0 - 1e-9 {
                continue;
            }
            for j in i + 1..clusters.len() {
                let dj = clusters[j].0;
                if dj.dot(m).abs() > cos {
                    clusters[j].0 = if dj.dot(m) > 0.0 { m } else { -m };
                }
            }
        }
    }
    for it in &items {
        let d = clusters[it.cluster].0;
        let d = if d.dot(it.dir) >= 0.0 { d } else { -d };
        let r = &mut seg.regions[it.region];
        let s = ctx.region_samples(&r.faces);
        match r.prim {
            Prim::Plane { .. } => r.prim = fit_plane_dir(&s, d),
            Prim::Cylinder { .. } => {
                if let Some(q) = fit_cylinder(&s, Some(d), None) {
                    r.prim = q;
                }
            }
            _ => {}
        }
    }
    // ------------------------------------------------ coplanar planes
    let planes: Vec<usize> = (0..seg.regions.len()).filter(|&i| !seg.regions[i].locked && !seg.regions[i].faces.is_empty() && seg.regions[i].prim.kind() == Kind::Plane).collect();
    let mut done = vec![false; seg.regions.len()];
    for &i in &planes {
        if done[i] {
            continue;
        }
        let Prim::Plane { n, d } = seg.regions[i].prim else { continue };
        let group: Vec<usize> = planes
            .iter()
            .copied()
            .filter(|&j| !done[j] && matches!(seg.regions[j].prim, Prim::Plane { n: nj, d: dj } if nj.dot(n) > 1.0 - 1e-9 && (dj - d).abs() < p.offset))
            .collect();
        let w: f64 = group.iter().map(|&j| seg.regions[j].area).sum();
        let dm: f64 = group.iter().map(|&j| if let Prim::Plane { d, .. } = seg.regions[j].prim { d * seg.regions[j].area } else { 0.0 }).sum::<f64>() / w;
        if group.len() > 1 {
            rep.coplanar += group.len() - 1;
        }
        for &j in &group {
            done[j] = true;
            seg.regions[j].prim = Prim::Plane { n, d: dm };
        }
    }
    // ------------------------------------------------ radii
    let mut curved: Vec<usize> = (0..seg.regions.len()).filter(|&i| !seg.regions[i].locked && !seg.regions[i].faces.is_empty() && seg.regions[i].prim.kind() != Kind::Plane).collect();
    let radius = |p: &Prim| match *p {
        Prim::Cylinder { r, .. } | Prim::Sphere { r, .. } => r,
        _ => 0.0,
    };
    curved.sort_by(|&a, &b| radius(&seg.regions[a].prim).partial_cmp(&radius(&seg.regions[b].prim)).unwrap());
    let mut k = 0;
    while k < curved.len() {
        let r0 = radius(&seg.regions[curved[k]].prim);
        let tol = p.radius_abs.max(p.radius_rel * r0);
        let mut e = k + 1;
        while e < curved.len() && radius(&seg.regions[curved[e]].prim) - r0 < tol {
            e += 1;
        }
        let group = &curved[k..e];
        let w: f64 = group.iter().map(|&j| seg.regions[j].area).sum();
        let rm: f64 = group.iter().map(|&j| radius(&seg.regions[j].prim) * seg.regions[j].area).sum::<f64>() / w;
        if group.len() > 1 {
            rep.radii += 1;
        }
        for &j in group {
            let r = &mut seg.regions[j];
            let s = ctx.region_samples(&r.faces);
            match r.prim {
                Prim::Cylinder { a, .. } => {
                    if let Some(q) = fit_cylinder(&s, Some(a), Some(rm)) {
                        r.prim = q;
                    }
                }
                Prim::Sphere { .. } => {
                    if let Some(q) = fit_sphere(&s, Some(rm)) {
                        r.prim = q;
                    }
                }
                _ => {}
            }
        }
        k = e;
    }
    // ------------------------------------------------ coaxial cylinders
    let cyls: Vec<usize> = (0..seg.regions.len()).filter(|&i| !seg.regions[i].locked && !seg.regions[i].faces.is_empty() && seg.regions[i].prim.kind() == Kind::Cylinder).collect();
    let mut done = vec![false; seg.regions.len()];
    for &i in &cyls {
        if done[i] {
            continue;
        }
        let Prim::Cylinder { p: pi, a, .. } = seg.regions[i].prim else { continue };
        let off = |q: DVec3| {
            let v = q - pi;
            v - a * a.dot(v)
        };
        let group: Vec<usize> = cyls
            .iter()
            .copied()
            .filter(|&j| !done[j] && matches!(seg.regions[j].prim, Prim::Cylinder { p: pj, a: aj, .. } if aj.dot(a).abs() > 1.0 - 1e-9 && off(pj).length() < p.offset * 2.0))
            .collect();
        let w: f64 = group.iter().map(|&j| seg.regions[j].area).sum();
        let shift: DVec3 = group.iter().map(|&j| if let Prim::Cylinder { p: pj, .. } = seg.regions[j].prim { off(pj) * seg.regions[j].area } else { DVec3::ZERO }).sum::<DVec3>() / w;
        if group.len() > 1 {
            rep.coaxial += group.len() - 1;
        }
        for &j in &group {
            done[j] = true;
            if let Prim::Cylinder { a: aj, r, .. } = seg.regions[j].prim {
                seg.regions[j].prim = Prim::Cylinder { p: pi + shift, a: aj, r };
            }
        }
    }
    // ------------------------------------------------ mirror
    if let (Some(mr), true) = (mir, p.mirror) {
        rep.mirrored = mirror_pairs(seg, mr, p);
    }
    // the distances after all this; a shape pulled too far from its faces
    // goes back
    for (r, (prim0, rms0)) in seg.regions.iter_mut().zip(before) {
        let s = ctx.region_samples(&r.faces);
        r.rms = r.prim.rms(&s);
        if r.rms > p.max_rms.max(rms0 * 1.5) {
            r.prim = prim0;
            r.rms = rms0;
            rep.reverted += 1;
        }
    }
    rep
}

/// Each shape and the shape matching its mirror image are made exact
/// mirrors (the heavier one leads); a shape lying on the mirror plane is
/// put exactly on it.
fn mirror_pairs(seg: &mut Segmentation, mr: &Mirror, p: &RegParams) -> usize {
    let n = seg.regions.len();
    let mut done = vec![false; n];
    let mut count = 0;
    let mut order: Vec<usize> = (0..n).filter(|&i| !seg.regions[i].locked && !seg.regions[i].faces.is_empty()).collect();
    order.sort_by(|&a, &b| seg.regions[b].area.partial_cmp(&seg.regions[a].area).unwrap());
    for &i in &order {
        if done[i] {
            continue;
        }
        done[i] = true;
        let pi = seg.regions[i].prim;
        let img = mirror_prim(&pi, mr);
        // on the plane: its own mirror
        if same(&pi, &img, p) {
            seg.regions[i].prim = average(&pi, &img);
            if pi != img {
                count += 1;
            }
            continue;
        }
        let mut best: Option<(usize, f64)> = None;
        for &j in &order {
            if done[j] || seg.regions[j].prim.kind() != pi.kind() {
                continue;
            }
            if let Some(d) = gap(&img, &seg.regions[j].prim, p) {
                if best.map(|b| d < b.1).unwrap_or(true) {
                    best = Some((j, d));
                }
            }
        }
        if let Some((j, _)) = best {
            done[j] = true;
            let pj = seg.regions[j].prim;
            let m = average(&pi, &mirror_prim(&pj, mr));
            seg.regions[i].prim = m;
            seg.regions[j].prim = mirror_prim(&m, mr);
            count += 1;
        }
    }
    count
}

pub fn mirror_prim(q: &Prim, mr: &Mirror) -> Prim {
    match *q {
        Prim::Plane { n, d } => {
            let p0 = n * d;
            let n2 = mr.apply_dir(n);
            Prim::Plane { n: n2, d: n2.dot(mr.apply(p0)) }
        }
        Prim::Cylinder { p, a, r } => Prim::Cylinder { p: mr.apply(p), a: mr.apply_dir(a), r },
        Prim::Sphere { c, r } => Prim::Sphere { c: mr.apply(c), r },
    }
}

fn line_gap(p1: DVec3, a1: DVec3, p2: DVec3) -> f64 {
    let v = p2 - p1;
    (v - a1 * a1.dot(v)).length()
}

/// How far apart two shapes of the same kind are, if they are close enough
/// to be one.
fn gap(a: &Prim, b: &Prim, p: &RegParams) -> Option<f64> {
    let cos = p.angle.to_radians().cos();
    match (*a, *b) {
        (Prim::Plane { n: n1, d: d1 }, Prim::Plane { n: n2, d: d2 }) => (n1.dot(n2) > cos && (d1 - d2).abs() < p.offset).then_some((d1 - d2).abs()),
        (Prim::Cylinder { p: p1, a: a1, r: r1 }, Prim::Cylinder { p: p2, a: a2, r: r2 }) => {
            let g = line_gap(p1, a1, p2);
            (a1.dot(a2).abs() > cos && g < p.offset * 2.0 && (r1 - r2).abs() < p.radius_abs.max(p.radius_rel * r1)).then_some(g)
        }
        (Prim::Sphere { c: c1, r: r1 }, Prim::Sphere { c: c2, r: r2 }) => {
            let g = (c1 - c2).length();
            (g < p.offset * 2.0 && (r1 - r2).abs() < p.radius_abs.max(p.radius_rel * r1)).then_some(g)
        }
        _ => None,
    }
}

fn same(a: &Prim, b: &Prim, p: &RegParams) -> bool {
    gap(a, b, p).is_some()
}

fn average(a: &Prim, b: &Prim) -> Prim {
    match (*a, *b) {
        (Prim::Plane { n: n1, d: d1 }, Prim::Plane { n: n2, .. }) => {
            let n = (n1 + n2).normalize();
            let p0 = n1 * d1;
            let pb = if let Prim::Plane { n, d } = *b { n * d } else { p0 };
            Prim::Plane { n, d: n.dot((p0 + pb) * 0.5) }
        }
        (Prim::Cylinder { p: p1, a: a1, r: r1 }, Prim::Cylinder { p: p2, a: a2, r: r2 }) => {
            let a2 = if a2.dot(a1) < 0.0 { -a2 } else { a2 };
            let a = (a1 + a2).normalize();
            // the point of the second axis line nearest to the first's
            let q2 = p2 + a2 * a2.dot(p1 - p2);
            Prim::Cylinder { p: (p1 + q2) * 0.5, a, r: (r1 + r2) * 0.5 }
        }
        (Prim::Sphere { c: c1, r: r1 }, Prim::Sphere { c: c2, r: r2 }) => Prim::Sphere { c: (c1 + c2) * 0.5, r: (r1 + r2) * 0.5 },
        _ => *a,
    }
}
