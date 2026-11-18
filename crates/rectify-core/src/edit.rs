//! Hand edits of the regions (Romain, 18 nov.: "le soft propose, toi tu
//! corriges d'un geste"): the brush merges, splits or frees regions, and a
//! shape placed by a gesture fits itself to the faces under it, then grows
//! over the mesh wherever the mesh lies on it. Regions keep their index (an
//! emptied one stays, with no faces), so what is picked or drawn stays valid.

use crate::mesh::{Mesh, Topo, NONE};
use crate::prim::{axis_of, fit_cylinder, fit_plane, fit_sphere, total_weight, Kind, Prim, Sample};
use crate::segment::{Region, SegParams, Segmentation};
use glam::DVec3;
use std::collections::{HashMap, HashSet, VecDeque};

/// One face as a fitting sample (centre, unit normal, area).
pub fn sample(m: &Mesh, t: usize) -> Sample {
    let c = m.tri_cross(t);
    let l = c.length();
    Sample { p: m.tri_centre(t), n: if l > 0.0 { c / l } else { DVec3::Y }, w: l * 0.5 }
}

pub fn samples(m: &Mesh, faces: &[u32]) -> Vec<Sample> {
    faces.iter().map(|&f| sample(m, f as usize)).collect()
}

/// The faces under a brush dab: reached from `seed` through neighbours whose
/// centre is within `radius` of `centre`. With `toward` (the direction to the
/// eye), faces turned away are neither taken nor crossed, so a dab does not
/// go round a thin wall to its back.
pub fn dab(m: &Mesh, topo: &Topo, seed: u32, centre: DVec3, radius: f64, toward: Option<DVec3>, out: &mut HashSet<u32>) -> usize {
    let mut seen: HashSet<u32> = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(seed);
    seen.insert(seed);
    let r2 = radius * radius;
    let mut added = 0;
    while let Some(f) = queue.pop_front() {
        let s = sample(m, f as usize);
        if f != seed {
            if (s.p - centre).length_squared() > r2 {
                continue;
            }
            if let Some(v) = toward {
                if s.n.dot(v) < -0.05 {
                    continue;
                }
            }
        }
        if out.insert(f) {
            added += 1;
        }
        for &o in &topo.adj[f as usize] {
            if o != NONE && seen.insert(o) {
                queue.push_back(o);
            }
        }
    }
    added
}

fn valid(q: &Prim, p: &SegParams) -> bool {
    match *q {
        Prim::Plane { n, d } => n.is_finite() && d.is_finite(),
        Prim::Cylinder { r, p: c, a } => r.is_finite() && c.is_finite() && a.is_finite() && r <= p.max_radius * 2.0 && r >= p.tol,
        Prim::Sphere { r, c } => r.is_finite() && c.is_finite() && r <= p.max_radius * 2.0 && r >= p.tol,
    }
}

/// A cylinder on samples that may hardly turn (a stroke along a large
/// barrel): when the normals do not give the axis, `hint` (the stroke's
/// direction) does, made square to the faces.
fn cylinder(s: &[Sample], hint: Option<DVec3>) -> Option<Prim> {
    let (_, l) = axis_of(s);
    let axis = match hint {
        Some(h) if l[1] < 0.02 => {
            let n: DVec3 = s.iter().map(|x| x.n * x.w).sum::<DVec3>().normalize_or(DVec3::Y);
            let a = h - n * n.dot(h);
            (a.length() > 1e-6).then(|| a.normalize())
        }
        _ => None,
    };
    fit_cylinder(s, axis, None)
}

/// The shape for these samples: of the kind asked, else the one that fits
/// best (a curved shape must clearly beat the plane, a sphere the cylinder).
pub fn fit_best(s: &[Sample], kind: Option<Kind>, hint: Option<DVec3>, p: &SegParams) -> Option<Prim> {
    let fit = |k: Kind| -> Option<Prim> {
        let q = match k {
            Kind::Plane => fit_plane(s),
            Kind::Cylinder => cylinder(s, hint),
            Kind::Sphere => fit_sphere(s, None),
        }?;
        valid(&q, p).then_some(q)
    };
    if let Some(k) = kind {
        return fit(k);
    }
    let plane = fit(Kind::Plane)?;
    let mut best = (plane.rms(s), plane);
    // below this, the plane is as good as it gets: noise
    let floor = p.tol * 0.15;
    if let Some(c) = fit(Kind::Cylinder) {
        let e = c.rms(s);
        if best.0 > floor && e < best.0 * 0.6 {
            best = (e, c);
        }
    }
    if let Some(c) = fit(Kind::Sphere) {
        let e = c.rms(s);
        if best.0 > floor && e < best.0 * 0.6 {
            best = (e, c);
        }
    }
    Some(best.1)
}

/// Grows `prim` from the seed faces over every face that lies on it,
/// whatever region it is in (`skip`: faces it may not take), refitting the
/// shape as the region doubles (the kind stays; a fixed axis hint stays the
/// hint). Returns the faces and the final shape.
pub fn grow(m: &Mesh, topo: &Topo, seed: &[u32], mut prim: Prim, hint: Option<DVec3>, p: &SegParams, skip: &dyn Fn(u32) -> bool) -> (Vec<u32>, Prim) {
    let cos = p.angle.to_radians().cos();
    let mut faces: Vec<u32> = vec![];
    for pass in 0..2 {
        faces.clear();
        let mut seen: HashSet<u32> = HashSet::with_capacity(seed.len() * 4);
        let mut queue: VecDeque<u32> = VecDeque::new();
        for &f in seed {
            if seen.insert(f) {
                queue.push_back(f);
            }
        }
        let mut next_fit = (seed.len() * 2).max(24);
        while let Some(f) = queue.pop_front() {
            let s = sample(m, f as usize);
            // the seed is the user's: kept even where it strays a little
            let is_seed = faces.len() < seed.len();
            if !is_seed && (skip(f) || !prim.fits(&s, p.tol, cos)) {
                continue;
            }
            faces.push(f);
            for &o in &topo.adj[f as usize] {
                if o != NONE && seen.insert(o) {
                    queue.push_back(o);
                }
            }
            if pass == 0 && faces.len() >= next_fit {
                next_fit = faces.len() * 2;
                if let Some(q) = fit_best(&samples(m, &faces), Some(prim.kind()), hint, p) {
                    prim = q;
                }
            }
        }
        if let Some(q) = fit_best(&samples(m, &faces), Some(prim.kind()), hint, p) {
            prim = q;
        }
    }
    (faces, prim)
}

/// Rebuilds the face list, area and fit of the given regions from the labels
/// (an emptied region keeps its index; a locked one keeps its shape).
pub fn refresh(m: &Mesh, seg: &mut Segmentation, regions: &[u32]) {
    let want: HashSet<u32> = regions.iter().copied().collect();
    for &r in &want {
        seg.regions[r as usize].faces.clear();
    }
    for (t, &l) in seg.label.iter().enumerate() {
        if l != NONE && want.contains(&l) {
            seg.regions[l as usize].faces.push(t as u32);
        }
    }
    for &r in &want {
        let reg = &mut seg.regions[r as usize];
        let s = samples(m, &reg.faces);
        reg.area = total_weight(&s);
        if reg.faces.len() >= 6 && !reg.locked {
            let q = match reg.prim.kind() {
                Kind::Plane => fit_plane(&s),
                Kind::Cylinder => fit_cylinder(&s, None, None),
                Kind::Sphere => fit_sphere(&s, None),
            };
            if let Some(q) = q.filter(|q| q.is_finite()) {
                reg.prim = q;
            }
        }
        reg.rms = reg.prim.rms(&s);
        if reg.faces.len() < 3 {
            for &f in &reg.faces {
                seg.label[f as usize] = NONE;
            }
            reg.faces.clear();
            reg.area = 0.0;
        }
    }
}

/// The brush's Merge: the regions touched become one (the first one's index),
/// with the shape that fits them together best; free faces under the brush
/// join when they lie on it. Returns the region, or None when there was
/// nothing to merge.
pub fn merge(m: &Mesh, seg: &mut Segmentation, regions: &[u32], painted: &HashSet<u32>, p: &SegParams) -> Option<u32> {
    let mut rs: Vec<u32> = vec![];
    for &r in regions {
        if !rs.contains(&r) && (r as usize) < seg.regions.len() && !seg.regions[r as usize].faces.is_empty() {
            rs.push(r);
        }
    }
    let free: Vec<u32> = painted.iter().copied().filter(|&f| seg.label[f as usize] == NONE).collect();
    if rs.is_empty() || (rs.len() < 2 && free.is_empty()) {
        return None;
    }
    let target = rs[0];
    let mut faces: Vec<u32> = rs.iter().flat_map(|&r| seg.regions[r as usize].faces.iter().copied()).collect();
    let s = samples(m, &faces);
    let q = fit_best(&s, None, None, p).unwrap_or(seg.regions[target as usize].prim);
    // free faces join when they lie on the new shape (a stroke across a
    // fillet does not flatten it)
    let cos = p.angle.to_radians().cos();
    for f in free {
        if q.fits(&sample(m, f as usize), p.tol * 2.0, cos) {
            faces.push(f);
        }
    }
    for &f in &faces {
        seg.label[f as usize] = target;
    }
    let reg = &mut seg.regions[target as usize];
    reg.prim = q;
    reg.locked = false;
    refresh(m, seg, &rs);
    // the kind chosen above, refitted on all the faces
    if let Some(q) = fit_best(&samples(m, &seg.regions[target as usize].faces), Some(q.kind()), None, p) {
        let reg = &mut seg.regions[target as usize];
        reg.rms = q.rms(&samples(m, &reg.faces));
        reg.prim = q;
    }
    Some(target)
}

/// The brush's Free: the faces under it belong to no region.
pub fn erase(m: &Mesh, seg: &mut Segmentation, painted: &HashSet<u32>) -> usize {
    let mut touched: Vec<u32> = vec![];
    let mut n = 0;
    for &f in painted {
        let l = seg.label[f as usize];
        if l != NONE {
            if !touched.contains(&l) {
                touched.push(l);
            }
            seg.label[f as usize] = NONE;
            n += 1;
        }
    }
    refresh(m, seg, &touched);
    n
}

/// The brush's Split: a stroke across a region cuts it; each piece on either
/// side becomes its own region (the largest keeps the index), fitted on its
/// own, and the faces under the stroke go to the piece whose shape they are
/// closest to. Returns how many regions were added.
pub fn split(m: &Mesh, topo: &Topo, seg: &mut Segmentation, cut: &HashSet<u32>) -> usize {
    let mut by_region: HashMap<u32, Vec<u32>> = HashMap::new();
    for &f in cut {
        let l = seg.label[f as usize];
        if l != NONE {
            by_region.entry(l).or_default().push(f);
        }
    }
    let mut added = 0;
    let mut touched: Vec<u32> = vec![];
    for (r, cut_faces) in by_region {
        // the pieces left once the cut is taken out
        let mut piece: HashMap<u32, usize> = HashMap::new();
        let mut pieces: Vec<(f64, Vec<u32>)> = vec![];
        for &f in &seg.regions[r as usize].faces {
            if cut.contains(&f) || piece.contains_key(&f) {
                continue;
            }
            let id = pieces.len();
            let mut faces = vec![f];
            piece.insert(f, id);
            let mut i = 0;
            while i < faces.len() {
                let g = faces[i];
                i += 1;
                for &o in &topo.adj[g as usize] {
                    if o != NONE && seg.label[o as usize] == r && !cut.contains(&o) && !piece.contains_key(&o) {
                        piece.insert(o, id);
                        faces.push(o);
                    }
                }
            }
            let area = samples(m, &faces).iter().map(|s| s.w).sum();
            pieces.push((area, faces));
        }
        // crumbs along the cut are not pieces
        let big: Vec<usize> = (0..pieces.len()).filter(|&i| pieces[i].0 >= 0.5).collect();
        if big.len() < 2 {
            continue;
        }
        let mut order = big.clone();
        order.sort_by(|&a, &b| pieces[b].0.partial_cmp(&pieces[a].0).unwrap());
        // each big piece its own region, fitted on its faces
        let kind = seg.regions[r as usize].prim.kind();
        let mut ids: HashMap<usize, u32> = HashMap::new();
        for (k, &i) in order.iter().enumerate() {
            let id = if k == 0 {
                r
            } else {
                seg.regions.push(Region { prim: seg.regions[r as usize].prim, faces: vec![], area: 0.0, rms: 0.0, locked: false });
                added += 1;
                (seg.regions.len() - 1) as u32
            };
            ids.insert(i, id);
            let s = samples(m, &pieces[i].1);
            let q = match kind {
                Kind::Plane => fit_plane(&s),
                Kind::Cylinder => fit_cylinder(&s, None, None),
                Kind::Sphere => fit_sphere(&s, None),
            };
            if let Some(q) = q.filter(|q| q.is_finite()) {
                seg.regions[id as usize].prim = q;
            }
            for &f in &pieces[i].1 {
                seg.label[f as usize] = id;
            }
            touched.push(id);
        }
        // the cut and the crumbs: by layers from the pieces, each face to the
        // neighbouring piece whose shape is closest
        let mut rest: HashSet<u32> = cut_faces.into_iter().collect();
        for (i, (_, faces)) in pieces.iter().enumerate() {
            if !ids.contains_key(&i) {
                rest.extend(faces.iter().copied());
            }
        }
        for &f in &rest {
            seg.label[f as usize] = NONE;
        }
        loop {
            let mut layer: Vec<(u32, u32)> = vec![];
            for &f in &rest {
                let s = sample(m, f as usize);
                let mut best: Option<(f64, u32)> = None;
                for &o in &topo.adj[f as usize] {
                    if o == NONE {
                        continue;
                    }
                    let l = seg.label[o as usize];
                    if l == NONE || !touched.contains(&l) {
                        continue;
                    }
                    let d = seg.regions[l as usize].prim.dist(s.p);
                    if best.map(|b| d < b.0).unwrap_or(true) {
                        best = Some((d, l));
                    }
                }
                if let Some((_, l)) = best {
                    layer.push((f, l));
                }
            }
            if layer.is_empty() {
                break;
            }
            for (f, l) in layer {
                seg.label[f as usize] = l;
                rest.remove(&f);
            }
        }
        // what no piece reached stays with the region
        for &f in &rest {
            seg.label[f as usize] = r;
        }
    }
    refresh(m, seg, &touched);
    added
}

/// A shape placed by a gesture: fitted on the faces under the stroke (of the
/// kind asked, else the best), then grown over every face that lies on it,
/// taken from the regions they were in. Returns the new region.
pub fn place(m: &Mesh, topo: &Topo, seg: &mut Segmentation, seed: &[u32], kind: Option<Kind>, hint: Option<DVec3>, p: &SegParams) -> Result<u32, String> {
    if seed.len() < 6 {
        return Err("Paint a little more of the surface".into());
    }
    let s = samples(m, seed);
    let Some(q) = fit_best(&s, kind, hint, p) else {
        return Err(match kind {
            Some(k) => format!("No {} fits there", k.label().to_lowercase()),
            None => "No shape fits there".into(),
        });
    };
    let locked: HashSet<u32> = (0..seg.regions.len() as u32).filter(|&r| seg.regions[r as usize].locked).collect();
    let label = &seg.label;
    let (faces, q) = grow(m, topo, seed, q, hint, p, &|f| {
        let l = label[f as usize];
        l != NONE && locked.contains(&l)
    });
    let id = seg.regions.len() as u32;
    let mut touched: Vec<u32> = vec![];
    for &f in &faces {
        let l = seg.label[f as usize];
        if l != NONE && !touched.contains(&l) {
            touched.push(l);
        }
        seg.label[f as usize] = id;
    }
    let s = samples(m, &faces);
    seg.regions.push(Region { prim: q, area: total_weight(&s), rms: q.rms(&s), faces, locked: false });
    refresh(m, seg, &touched);
    Ok(id)
}

trait Finite {
    fn is_finite(&self) -> bool;
}

impl Finite for Prim {
    fn is_finite(&self) -> bool {
        match *self {
            Prim::Plane { n, d } => n.is_finite() && d.is_finite(),
            Prim::Cylinder { p, a, r } => p.is_finite() && a.is_finite() && r.is_finite(),
            Prim::Sphere { c, r } => c.is_finite() && r.is_finite(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box 10 × 10 × 10 of small triangles, one plane region per side.
    fn cube() -> Mesh {
        let n = 10;
        let mut pos = vec![];
        let mut tris = vec![];
        let sides: [(DVec3, DVec3, DVec3); 6] = [
            (DVec3::new(0.0, 0.0, 0.0), DVec3::X, DVec3::Y),
            (DVec3::new(0.0, 0.0, 10.0), DVec3::Y, DVec3::X),
            (DVec3::new(0.0, 0.0, 0.0), DVec3::Z, DVec3::X),
            (DVec3::new(0.0, 10.0, 0.0), DVec3::X, DVec3::Z),
            (DVec3::new(0.0, 0.0, 0.0), DVec3::Y, DVec3::Z),
            (DVec3::new(10.0, 0.0, 0.0), DVec3::Z, DVec3::Y),
        ];
        for (o, u, v) in sides {
            let base = pos.len() as u32;
            for j in 0..=n {
                for i in 0..=n {
                    pos.push(o + u * i as f64 + v * j as f64);
                }
            }
            for j in 0..n {
                for i in 0..n {
                    let a = base + j * (n + 1) + i;
                    tris.push([a, a + 1, a + n + 2]);
                    tris.push([a, a + n + 2, a + n + 1]);
                }
            }
        }
        let mut m = Mesh { pos, tris, uv: vec![], tri_uv: vec![], unit_mm: 1.0 };
        // weld the shared corners
        let mut map: HashMap<[i64; 3], u32> = HashMap::new();
        let mut remap = vec![0u32; m.pos.len()];
        let mut out = vec![];
        for (i, p) in m.pos.iter().enumerate() {
            let k = [(p.x * 1000.0).round() as i64, (p.y * 1000.0).round() as i64, (p.z * 1000.0).round() as i64];
            remap[i] = *map.entry(k).or_insert_with(|| {
                out.push(*p);
                (out.len() - 1) as u32
            });
        }
        m.pos = out;
        for t in m.tris.iter_mut() {
            *t = t.map(|v| remap[v as usize]);
        }
        m
    }

    fn one_region(m: &Mesh) -> Segmentation {
        // the whole bottom side (y = 0, the third block of faces) as one plane
        let per = 200;
        let faces: Vec<u32> = (2 * per..3 * per).map(|t| t as u32).collect();
        let mut label = vec![NONE; m.tris.len()];
        for &f in &faces {
            label[f as usize] = 0;
        }
        let s = samples(m, &faces);
        Segmentation { label, regions: vec![Region { prim: fit_plane(&s).unwrap(), area: total_weight(&s), rms: 0.0, faces, locked: false }] }
    }

    #[test]
    fn split_then_merge() {
        let m = cube();
        let topo = Topo::new(&m);
        let mut seg = one_region(&m);
        // a cut across the bottom side at x = 5
        let cut: HashSet<u32> = seg.regions[0].faces.iter().copied().filter(|&f| (m.tri_centre(f as usize).x - 5.0).abs() < 0.6).collect();
        assert!(!cut.is_empty());
        let added = split(&m, &topo, &mut seg, &cut);
        assert_eq!(added, 1);
        let n0 = seg.regions[0].faces.len();
        let n1 = seg.regions[1].faces.len();
        assert_eq!(n0 + n1, 200);
        assert!(n0 >= 80 && n1 >= 80, "{n0} {n1}");
        let p = SegParams::default();
        let r = merge(&m, &mut seg, &[1, 0], &HashSet::new(), &p).unwrap();
        assert_eq!(r, 1);
        assert_eq!(seg.regions[1].faces.len(), 200);
        assert!(seg.regions[0].faces.is_empty());
    }

    #[test]
    fn place_grows_over_the_side() {
        let m = cube();
        let topo = Topo::new(&m);
        let mut seg = Segmentation { label: vec![NONE; m.tris.len()], regions: vec![] };
        // a few faces in the middle of the top side (y = 10)
        let seed: Vec<u32> = (0..m.tris.len() as u32).filter(|&f| {
            let c = m.tri_centre(f as usize);
            (c.y - 10.0).abs() < 1e-6 && (c.x - 5.0).abs() < 1.5 && (c.z - 5.0).abs() < 1.5
        }).collect();
        let id = place(&m, &topo, &mut seg, &seed, Some(Kind::Plane), None, &SegParams::default()).unwrap();
        assert_eq!(seg.regions[id as usize].faces.len(), 200);
    }
}
