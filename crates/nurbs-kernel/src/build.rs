//! The document built with OpenCascade (crate forme-occt): each feature's
//! solid, its faces tagged (`FaceTag`), then the booleans in order. Used by
//! the kernel process; on iPad it will run in the app itself.
//!
//! Caches, for live editing: the solid of each feature (by content) and the
//! result after each feature (by the content of everything up to it). While
//! one feature moves, only the booleans from it onwards are made again.

use crate::doc::{Comp, Doc, EdgeRef, FaceRef, Feature, Knife, Op, Prim, Push, Round, Tweak};
use crate::model::{Edge, EdgeKind, Face, FaceTag, Model, SurfKind};
use crate::proto::step;
use forme_occt::{CutKind, Shape, SurfKind as OccSurf};
use glam::{DMat4, DVec3};
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;
use std::time::Instant;

/// Meshing: chord (mm) and angle (rad), fine when idle, coarse while moving.
const FINE: (f64, f64) = (0.02, 0.2);
const COARSE: (f64, f64) = (0.08, 0.35);

/// Writes the step on stderr before an operation that could crash the process.
fn mark(s: &str) {
    let mut e = std::io::stderr();
    let _ = writeln!(e, "STEP {s}");
    let _ = e.flush();
}

/// The result after a feature: the solid (None while nothing is built) and
/// what went wrong up to there.
#[derive(Clone)]
struct Stage {
    shape: Option<Rc<Shape>>,
    errors: Vec<String>,
    /// Fillets and chamfers too big, made at the largest size that works.
    limits: Vec<(u32, f64)>,
    /// The fillets with tension whose faces all took it.
    conic: Vec<u32>,
    /// Fillets whose tension was too full or too flat for a fillet running
    /// round them, made at the nearest that works.
    tension_limits: Vec<(u32, f64)>,
}

#[derive(Default)]
pub struct Cache {
    tools: HashMap<u64, Rc<Shape>>,
    stages: HashMap<u64, Stage>,
}

fn rows(m: DMat4) -> [[f64; 4]; 3] {
    let c = m.to_cols_array_2d();
    [[c[0][0], c[1][0], c[2][0], c[3][0]], [c[0][1], c[1][1], c[2][1], c[3][1]], [c[0][2], c[1][2], c[2][2], c[3][2]]]
}

/// Which face of the primitive (local frame) a face is, from its normal and
/// its middle point. A sketch: 1 top, 2 bottom, 3 + the outline piece a side
/// comes from (`doc::Piece::id`).
fn sub_of(f: &Feature, kind: OccSurf, n: DVec3, at: DVec3) -> u16 {
    match f.prim {
        Prim::Box { .. } => {
            let a = n.abs();
            if a.x >= a.y && a.x >= a.z {
                if n.x > 0.0 { 0 } else { 1 }
            } else if a.y >= a.z {
                if n.y > 0.0 { 2 } else { 3 }
            } else if n.z > 0.0 {
                4
            } else {
                5
            }
        }
        Prim::Cylinder { .. } => {
            if kind != OccSurf::Plane {
                0
            } else if n.y > 0.0 {
                1
            } else {
                2
            }
        }
        Prim::Profile { x, z, .. } => {
            if kind == OccSurf::Plane && n.y > 0.5 {
                1
            } else if kind == OccSurf::Plane && n.y < -0.5 {
                2
            } else {
                // the piece nearest the side's middle, seen from above
                let q = [at.x, at.z];
                let pieces = f.profile.as_ref().map(|p| p.pieces(x, z)).unwrap_or_default();
                let near = pieces.iter().map(|pc| (pc.id, pc.pts.windows(2).map(|w| seg_dist(q, w[0], w[1])).fold(f64::MAX, f64::min))).min_by(|a, b| a.1.total_cmp(&b.1));
                3 + near.map(|(id, _)| id).unwrap_or(0).min(60000) as u16
            }
        }
        Prim::Path { .. } => 0,
    }
}

/// Distance from `q` to the segment `a`–`b` (plane coordinates).
fn seg_dist(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx * dx + dy * dy;
    let t = if l > 0.0 { (((q[0] - a[0]) * dx + (q[1] - a[1]) * dy) / l).clamp(0.0, 1.0) } else { 0.0 };
    ((q[0] - a[0] - t * dx).powi(2) + (q[1] - a[1] - t * dy).powi(2)).sqrt()
}

/// The feature's solid in the world, its faces tagged.
fn tool(f: &Feature) -> Result<Shape, String> {
    let h = f.prim.height().max(0.01);
    let s = match f.prim {
        Prim::Box { x, z, .. } => Shape::cuboid(x.max(0.01), h, z.max(0.01))?,
        Prim::Cylinder { radius, .. } => Shape::cylinder(radius.max(0.01), h)?,
        Prim::Profile { x, z, .. } => {
            let pr = f.profile.as_ref().ok_or("a sketch without its outline")?;
            // OpenCascade extrudes an outline of its XY plane along Z: local
            // (x, z) is OCC (x, -z), then turned so OCC Z is local Y
            let pts: Vec<[f64; 2]> = pr.scaled(x.max(0.01), z.max(0.01)).iter().map(|p| [p[0], -p[1]]).collect();
            let flip = |q: [f64; 2]| [q[0], -q[1]];
            let kind = |c: &crate::doc::Cut| if c.chamfer { CutKind::Chamfer } else if c.rho > 0.0 { CutKind::Conic } else { CutKind::Arc };
            let cuts: Vec<_> = pr.cuts(x.max(0.01), z.max(0.01)).into_iter().map(|c| c.map(|c| (kind(&c), flip(c.a), flip(c.m), flip(c.b)))).collect();
            let s = if cuts.iter().any(|c| c.is_some()) { Shape::prism_cuts(&pts, &pr.smooth, &cuts, h * 0.5)? } else { Shape::prism(&pts, &pr.smooth, h * 0.5)? };
            s.transformed(rows(DMat4::from_rotation_x(-std::f64::consts::FRAC_PI_2)))?
        }
        Prim::Path { .. } => return Err("a path has no solid".into()),
    };
    let mut s = s;
    // the shape is centred on its local origin: tag it in its own frame
    let tags: Vec<i64> = (0..s.face_count())
        .map(|i| {
            let info = s.face_info(i);
            let sub = info.map(|fi| sub_of(f, fi.kind, DVec3::from_array(fi.normal), DVec3::from_array(fi.point))).unwrap_or(0);
            FaceTag { feature: f.id, sub }.pack()
        })
        .collect();
    s.set_tags(&tags);
    let m = f.matrix() * DMat4::from_translation(DVec3::Y * h * 0.5);
    s.transformed(rows(m))
}

fn surf(k: OccSurf) -> SurfKind {
    match k {
        OccSurf::Plane => SurfKind::Plane,
        OccSurf::Cylinder => SurfKind::Cylinder,
        OccSurf::Cone => SurfKind::Cone,
        OccSurf::Sphere => SurfKind::Sphere,
        OccSurf::Torus => SurfKind::Torus,
        OccSurf::Other => SurfKind::Other,
    }
}

/// The edges of `shape` the references name (edge-map indices, 1-based),
/// and how many were not found. Among the edges joining the same two faces,
/// the one nearest the reference's point.
fn resolve(shape: &Shape, refs: &[EdgeRef]) -> (Vec<i32>, usize) {
    let adj = shape.edge_faces();
    let tags: Vec<i64> = (0..shape.face_count()).map(|i| shape.face_tag(i)).collect();
    let tag = |f: i32| if f >= 0 { tags.get(f as usize).copied() } else { None };
    let mut out = vec![];
    let mut missing = 0;
    for r in refs {
        let want = r.key();
        let found: Vec<i32> = adj
            .iter()
            .enumerate()
            .filter_map(|(i, f)| {
                let (a, b) = (tag(f[0])?, tag(f[1])?);
                let k = if a <= b { [a, b] } else { [b, a] };
                (k == want).then_some(i as i32 + 1)
            })
            .collect();
        let best = match found.as_slice() {
            [] => None,
            [one] => Some(*one),
            many => many
                .iter()
                .map(|&i| {
                    let d = shape.edge(i, 9).map(|e| e.pts.iter().map(|p| DVec3::from_array(*p).distance(r.at)).fold(f64::MAX, f64::min)).unwrap_or(f64::MAX);
                    (i, d)
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i),
        };
        match best {
            Some(i) if !out.contains(&i) => out.push(i),
            Some(_) => {}
            None => missing += 1,
        }
    }
    (out, missing)
}

/// A fillet or chamfer step on what is built so far: the new solid (None
/// when there is nothing to round), the edges not found, and the size it
/// was made at when the one asked for is too big (the largest that works,
/// to 0.1 mm, 0.01 under 1 mm: the fillet stops there instead of
/// vanishing).
/// `search`: when the size does not work, look for the largest that does
/// (by halves, for at most about 2 s: next to a face curved tighter than the
/// fillet, OpenCascade can take many seconds for each try near the limit).
fn round(acc: &Shape, f: &Feature, r: &Round, search: bool) -> (Option<Result<Shape, String>>, usize, Option<f64>) {
    let (edges, missing) = resolve(acc, &r.edges);
    if edges.is_empty() || r.size <= 1e-6 {
        return (None, missing, None);
    }
    let fresh = FaceTag { feature: f.id, sub: 0 }.pack();
    mark(&step("round", f));
    let make = |size: f64| if r.chamfer { acc.chamfer(&edges, size, fresh) } else { acc.fillet(&edges, size, fresh) };
    let res = make(r.size);
    if res.is_ok() || !search {
        return (Some(res), missing, None);
    }
    // too big? the largest size that works, by halves
    let (mut lo, mut hi) = (0.0, r.size);
    let t0 = std::time::Instant::now();
    for _ in 0..12 {
        if t0.elapsed().as_secs_f64() > 2.0 {
            break;
        }
        let mid = (lo + hi) * 0.5;
        if make(mid).is_ok() {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo < 0.02 {
            break;
        }
    }
    let q = if lo >= 1.0 { 0.1 } else { 0.01 };
    let size = (lo / q).floor() * q;
    if size >= 0.01 {
        if let Ok(s) = make(size) {
            return (Some(Ok(s)), missing, Some(size));
        }
    }
    (Some(res), missing, None)
}

/// A fillet that blends its corners: rebuilt in one operation with the run
/// of fillets just before it (from the first one it touches, through shared
/// edge ends), on the solid from before that run. None when there is
/// nothing to blend with or OpenCascade cannot make it (then each fillet is
/// made on its own, as before).
fn blend(befores: &[(&Feature, Stage)], f: &Feature, r: &Round) -> Option<(Shape, Vec<(u32, f64)>)> {
    let n = befores.len().checked_sub(1)?;
    let mut k = n;
    while k > 0 && befores[k - 1].0.round.as_ref().map(|g| !g.chamfer).unwrap_or(false) {
        k -= 1;
    }
    if k == n {
        return None;
    }
    let base = befores[k].1.shape.clone()?;
    let limits = &befores[n].1.limits;
    // each fillet of the run on the base: its id, size and edges
    let mut groups = vec![];
    for (g, _) in &befores[k..=n] {
        let gr = g.round.as_ref()?;
        let (edges, missing) = resolve(&base, &gr.edges);
        if missing > 0 || edges.is_empty() {
            return None;
        }
        let size = if g.id == f.id { r.size } else { limits.iter().find(|(i, _)| *i == g.id).map(|(_, l)| *l).unwrap_or(gr.size) };
        if size <= 1e-6 {
            return None;
        }
        groups.push((g.id, size, edges));
    }
    // which fillets touch this one (through shared edge ends, step by step)
    let ends: Vec<Vec<DVec3>> = groups
        .iter()
        .map(|(_, _, es)| es.iter().filter_map(|&i| base.edge(i, 2)).flat_map(|e| [e.pts.first().copied(), e.pts.last().copied()]).flatten().map(DVec3::from_array).collect())
        .collect();
    let meet = |a: usize, b: usize| ends[a].iter().any(|p| ends[b].iter().any(|q| p.distance(*q) < 1e-4));
    let last = groups.len() - 1;
    let mut touch = vec![false; groups.len()];
    touch[last] = true;
    loop {
        let mut more = false;
        for a in 0..groups.len() {
            if !touch[a] && (0..groups.len()).any(|b| touch[b] && meet(a, b)) {
                touch[a] = true;
                more = true;
            }
        }
        if !more {
            break;
        }
    }
    let first = touch.iter().position(|t| *t)?;
    if first == last {
        return None;
    }
    // the run from the first fillet it touches (the ones in between too, so
    // none is lost), all in one operation
    let base = befores[k + first].1.shape.clone()?;
    let mut entries = vec![];
    let mut tensions = vec![];
    for (g, _) in &befores[k + first..=n] {
        let gr = g.round.as_ref()?;
        if g.id != f.id && gr.tension > 0.0 {
            tensions.push((g.id, gr.tension));
        }
        let (edges, missing) = resolve(&base, &gr.edges);
        if missing > 0 {
            return None;
        }
        let (_, size, _) = groups.iter().find(|x| x.0 == g.id)?;
        let tag = FaceTag { feature: g.id, sub: 0 }.pack();
        entries.extend(edges.into_iter().map(|i| (i, *size, tag)));
    }
    mark(&step("round", f));
    base.fillet_multi(&entries, FaceTag { feature: f.id, sub: 0 }.pack()).ok().map(|s| (s, tensions))
}

/// Fillets' straight and circular faces made conic, each with its tension
/// (`Round.tension`), in one go (two meeting in a mitre need each other;
/// when that fails, one after the other); what stays round is listed in
/// `errors`. For each fillet: Some(true) when every face took it, None when
/// none did. A tension too full or too flat for a fillet running round it
/// stops at the nearest that works (Romain: "il bute et stop", as a fillet's
/// size does): `limits` gets (fillet, the tension it was made at).
fn tension(s: Rc<Shape>, ts: &[(u32, f64)], name_of: &dyn Fn(u32) -> String, errors: &mut Vec<String>, limits: &mut Vec<(u32, f64)>) -> (Rc<Shape>, Vec<Option<bool>>) {
    if ts.is_empty() {
        return (s, vec![]);
    }
    let tagged: Vec<(i64, f64)> = ts.iter().map(|&(id, t)| (FaceTag { feature: id, sub: 0 }.pack(), t)).collect();
    match s.fillet_tension(&tagged) {
        Ok((t, counts)) => {
            // too tight for a fillet running round it: again at the nearest
            // tension that works, kept when it then takes everywhere
            let stops: Vec<Option<f64>> = ts
                .iter()
                .zip(&counts)
                .map(|(&(_, rho), c)| {
                    let (lo, hi) = c.tight?.works?;
                    let at = if rho < lo { lo } else { hi };
                    (c.done < c.total && (at - rho).abs() > 1e-6).then_some(at)
                })
                .collect();
            if stops.iter().any(|x| x.is_some()) {
                let again: Vec<(i64, f64)> = tagged.iter().zip(&stops).map(|(&(tag, rho), x)| (tag, x.unwrap_or(rho))).collect();
                if let Ok((t2, c2)) = s.fillet_tension(&again) {
                    if stops.iter().zip(&c2).all(|(x, c)| x.is_none() || c.done == c.total) {
                        let mut took = vec![];
                        for ((&(id, _), x), c) in ts.iter().zip(&stops).zip(&c2) {
                            if let Some(at) = x {
                                limits.push((id, *at));
                            } else if c.done < c.total {
                                errors.push(format!("{}: tension on {} of {} faces, {} stay round (a blended corner or a curved end)", name_of(id), c.done, c.total, c.total - c.done));
                            }
                            took.push(if c.done == 0 { None } else { Some(c.done == c.total) });
                        }
                        return (Rc::new(t2), took);
                    }
                }
            }
            let mut took = vec![];
            for (&(id, rho), c) in ts.iter().zip(&counts) {
                let (done, total) = (c.done, c.total);
                if done < total {
                    let n = total - done;
                    if let Some(k) = c.tight {
                        // a fillet running round it is bigger than its conic's tightest curve
                        let works = k.works.map(|(a, b)| format!(": tension {a:.2} to {b:.2} works")).unwrap_or_default();
                        errors.push(format!("{}: at tension {rho:.2} its curve tightens to {:.1} mm, less than the {:.1} mm fillet running round it{works}", name_of(id), k.radius, k.need));
                    } else if done == 0 {
                        errors.push(format!("{}: tension: only on fillets along straight or circular edges, ending on flat faces or a fillet of the same size and tension", name_of(id)));
                    } else {
                        errors.push(format!("{}: tension on {done} of {total} faces, {n} stay{} round (a blended corner or a curved end)", name_of(id), if n == 1 { "s" } else { "" }));
                    }
                }
                took.push(if done == 0 { None } else { Some(done == total) });
            }
            (Rc::new(t), took)
        }
        Err(e) if ts.len() == 1 => {
            errors.push(format!("{}: {e}", name_of(ts[0].0)));
            (s, vec![None])
        }
        Err(_) => {
            let mut s = s;
            let mut took = vec![];
            for t in ts {
                let (n, k) = tension(s, std::slice::from_ref(t), name_of, errors, limits);
                s = n;
                took.extend(k);
            }
            (s, took)
        }
    }
}

/// The face a reference names: among the faces with its tag, the nearest
/// to its point.
fn find_face(shape: &Shape, r: &FaceRef) -> Option<usize> {
    (0..shape.face_count())
        .filter(|i| shape.face_tag(*i) == r.tag)
        .map(|i| (i, shape.face_distance(i, r.at.to_array()).unwrap_or(f64::MAX)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// A face pushed or pulled on what is built so far (`doc::Push`): the new
/// solid, None when it changes nothing. Its new faces are tagged with the
/// step: 1 the inset (or the end of the extrusion), 2, 3… its sides.
fn push(acc: &Shape, f: &Feature, p: &Push, limit: &mut Option<f64>) -> Result<Option<Shape>, String> {
    let face = find_face(acc, &p.face).ok_or("its face is gone")?;
    let info = acc.face_info(face).ok_or("its face is gone")?;
    let inset = p.inset.max(0.0);
    if p.depth.abs() < 1e-6 && inset < 1e-6 {
        return Ok(None);
    }
    let tag = |sub: u16| FaceTag { feature: f.id, sub }.pack();
    if info.kind != OccSurf::Plane {
        // a curved face: the whole face pulled out along its normals (an
        // offset), or pushed in; no inset on it
        if inset > 0.0 {
            return Err("an inset needs a flat face (a curved face can only be extruded)".into());
        }
        mark(&step("push", f));
        let tool = acc.thicken_face(face, p.depth, [tag(0), tag(1), tag(2)])?;
        return if p.depth > 0.0 { acc.fuse(&tool) } else { acc.cut(&tool) }.map(Some);
    }
    mark(&step("push", f));
    let mut inner = match acc.inset_face(face, inset) {
        Ok(x) => x,
        Err(e) if inset > 0.0 => {
            // too wide? the largest inset that works, by halves (Romain:
            // "il bute et stop", as fillets do)
            let (mut lo, mut hi) = (0.0, inset);
            for _ in 0..14 {
                let mid = (lo + hi) * 0.5;
                if acc.inset_face(face, mid).is_ok() {
                    lo = mid;
                } else {
                    hi = mid;
                }
                if hi - lo < 0.01 {
                    break;
                }
            }
            let q = if lo >= 1.0 { 0.1 } else { 0.01 };
            let size = (lo / q).floor() * q;
            match (size >= 0.01).then(|| acc.inset_face(face, size)) {
                Some(Ok(x)) => {
                    *limit = Some(size);
                    x
                }
                _ => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    // depth 0: the inset only drawn on the face
    if p.depth.abs() < 1e-6 {
        inner.set_tags(&vec![tag(1); inner.face_count().max(1)]);
        return acc.imprint(&inner).map(Some);
    }
    let n = DVec3::from_array(info.normal);
    let o = DVec3::from_array(info.point);
    // carving: from a little outside the face, so no face of the tool lies on it
    let start = if p.depth < 0.0 { 0.1 } else { 0.0 };
    let base = if start > 0.0 { inner.transformed(rows(DMat4::from_translation(n * start)))? } else { inner };
    let mut prism = base.extrude((n * (p.depth - start)).to_array())?;
    let mut side = 2u16;
    let tags: Vec<i64> = (0..prism.face_count())
        .map(|i| {
            let fi = prism.face_info(i);
            let across = fi.map(|fi| fi.kind == OccSurf::Plane && DVec3::from_array(fi.normal).dot(n).abs() > 0.999).unwrap_or(false);
            let s = fi.map(|fi| (DVec3::from_array(fi.point) - o).dot(n)).unwrap_or(0.0);
            if across && (s - p.depth).abs() < 1e-4 {
                tag(1)
            } else if across {
                tag(0)
            } else {
                side = side.saturating_add(1);
                tag(side - 1)
            }
        })
        .collect();
    prism.set_tags(&tags);
    if p.depth > 0.0 { acc.fuse(&prism) } else { acc.cut(&prism) }.map(Some)
}

/// The vertices (1-based) a component names on what is built so far.
fn comp_vertices(acc: &Shape, c: &Comp) -> Result<Vec<i32>, String> {
    match c {
        Comp::Face(r) => {
            let face = find_face(acc, r).ok_or("its face is gone")?;
            Ok(acc.face_vertices(face))
        }
        Comp::Edge(r) => {
            let (e, _) = resolve(acc, std::slice::from_ref(r));
            let e = e.first().ok_or("its edge is gone")?;
            Ok(acc.edge_vertices(*e))
        }
        Comp::Vertex(p) => {
            let pts = acc.vertex_points();
            let best = pts.iter().enumerate().map(|(i, q)| (i, DVec3::from_array(*q).distance(*p))).min_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((i, d)) if d < 1.0 => Ok(vec![i as i32 + 1]),
                _ => Err("its vertex is gone".into()),
            }
        }
    }
}

/// A face, edge or vertex moved (`doc::Tweak`) on what is built so far:
/// its vertices moved by `delta`, the faces round them following. None
/// when it does not move.
fn tweak(acc: &Shape, f: &Feature, t: &Tweak) -> Result<Option<Shape>, String> {
    if t.delta.length() < 1e-6 {
        return Ok(None);
    }
    let vs = comp_vertices(acc, &t.comp)?;
    if vs.is_empty() {
        return Err("nothing to move".into());
    }
    let pts = acc.vertex_points();
    let moves: Vec<(i32, [f64; 3])> = vs.iter().map(|&v| (v, (DVec3::from_array(pts[(v - 1) as usize]) + t.delta).to_array())).collect();
    mark(&step("tweak", f));
    let err = match acc.move_vertices(&moves) {
        Ok(s) => return Ok(Some(s)),
        Err(e) => e,
    };
    // a flat face moved along its normal: the faces round it, curved ones
    // and those with holes too, made longer or shorter to meet it
    if let Comp::Face(r) = &t.comp {
        if let Some((face, info)) = find_face(acc, r).and_then(|i| acc.face_info(i).map(|f| (i, f))) {
            let n = DVec3::from_array(info.normal).normalize_or_zero();
            let d = t.delta.dot(n);
            if info.kind == OccSurf::Plane && (t.delta - n * d).length() < 1e-6 * t.delta.length().max(1.0) {
                return acc.move_face(face, d).map(Some);
            }
        }
    }
    Err(err)
}

/// The Knife: its points found on the edges (the fraction from the end
/// nearer where each was put), cut into them with its lines. None when it
/// has no point.
fn knife(acc: &Shape, f: &Feature, k: &Knife) -> Result<Option<Shape>, String> {
    if k.pts.is_empty() {
        return Ok(None);
    }
    let refs: Vec<EdgeRef> = k.pts.iter().map(|p| p.edge.clone()).collect();
    let mut p3 = vec![];
    for (kp, r) in k.pts.iter().zip(refs.iter()) {
        let (found, _) = resolve(acc, std::slice::from_ref(r));
        let Some(&i) = found.first() else { return Err("its edges are gone".into()) };
        let p = match (acc.edge_point(i, kp.f), acc.edge_point(i, 1.0 - kp.f)) {
            (Some(a), Some(b)) => {
                if DVec3::from_array(a).distance(kp.at) <= DVec3::from_array(b).distance(kp.at) { a } else { b }
            }
            (Some(a), None) | (None, Some(a)) => a,
            _ => return Err("its edges are gone".into()),
        };
        p3.push(p);
    }
    let segs: Vec<[i32; 2]> = k.segs.iter().filter(|s| s[0] < p3.len() && s[1] < p3.len()).map(|s| [s[0] as i32, s[1] as i32]).collect();
    mark(&step("cut", f));
    acc.split(&p3, &segs).map(Some)
}

fn hash2(a: u64, b: u64) -> u64 {
    crate::doc::fnv(&[a.to_le_bytes(), b.to_le_bytes()].concat())
}

/// Builds the document. Never fails: what cannot be made is listed in
/// `Model::errors` and left out.
/// Builds the features `feats` in this order: the stage after the last
/// (the stages on the way cached by the hash of what comes before).
fn run<'a>(doc: &'a Doc, feats: &[&'a Feature], skip: &[String], cache: &mut Cache, model: &mut Model, used_tools: &mut Vec<u64>, used_stages: &mut Vec<u64>) -> Stage {
    let mut stage = Stage { shape: None, errors: vec![], limits: vec![], conic: vec![], tension_limits: vec![] };
    let mut key = 0u64;
    // the stage before each feature (blended fillets start again from there)
    let mut befores: Vec<(&Feature, Stage)> = vec![];
    for (i, &f) in feats.iter().enumerate() {
        befores.push((f, stage.clone()));
        let fh = f.hash();
        key = hash2(key, fh);
        let skipped = |what: &str| skip.iter().any(|s| *s == step(what, f));
        if skipped("tool") || skipped("boolean") || skipped("round") || skipped("push") || skipped("tweak") || skipped("cut") {
            key = hash2(key, 1);
        }
        used_stages.push(key);
        if let Some(s) = cache.stages.get(&key) {
            stage = s.clone();
            model.cached += 1;
            continue;
        }
        // a fillet or chamfer: on the solid built so far
        if let Some(r) = &f.round {
            if let Some(acc) = stage.shape.clone() {
                let name_of = |id: u32| doc.feature(id).map(|g| g.name.clone()).unwrap_or_default();
                let own = (r.tension > 0.0 && !r.chamfer && !skipped("round")).then_some((f.id, r.tension));
                // the tensions `ts` put back on a solid made circular, this
                // step's own too, in one go: the solid, the fillets whose
                // faces all took it, how many tensions are lost (of `ts`
                // those that had all taken it before, this step's own),
                // what stayed round
                let conic = &stage.conic;
                let finish = |p: Shape, ts: &[(u32, f64)]| {
                    let all: Vec<(u32, f64)> = ts.iter().copied().chain(own).collect();
                    if !all.is_empty() {
                        mark(&step("round", f));
                    }
                    let (mut errs, mut stops) = (vec![], vec![]);
                    let (t, took) = tension(Rc::new(p), &all, &name_of, &mut errs, &mut stops);
                    let full: Vec<u32> = all.iter().zip(&took).filter(|(_, k)| **k == Some(true)).map(|(a, _)| a.0).collect();
                    let lost = ts.iter().filter(|(id, _)| conic.contains(id) && !full.contains(id)).count() + own.filter(|o| !full.contains(&o.0)).iter().count();
                    (t, full, lost, (errs, stops))
                };
                let mut made = None;
                // blended with the fillets before it (rebuilt circular from
                // the solid before them), kept when the tensions all go
                // (back) on: else made on its own next to their conic faces
                // when that keeps them, so a fillet with tension does not
                // turn round because one is made next to it (or because
                // its corner is blended)
                let mut partial = None;
                if r.blend && !r.chamfer && !skipped("round") {
                    if let Some((b, run)) = blend(&befores, f, r) {
                        let (t, full, lost, errs) = finish(b, &run);
                        if lost == 0 {
                            stage.errors.extend(errs.0);
                            stage.tension_limits.extend(errs.1);
                            made = Some((t, full));
                        } else {
                            partial = Some((t, full, lost, errs));
                        }
                    }
                }
                if made.is_none() {
                    // (with a blended result at hand, a smaller size made on
                    // its own would not be taken: no search for it)
                    let (res, missing, limit) = if skipped("round") { (Some(Err("skipped, it crashed the kernel".to_string())), 0, None) } else { round(&acc, f, r, partial.is_none()) };
                    if missing > 0 {
                        stage.errors.push(format!("{}: {missing} edge{} not found", f.name, if missing > 1 { "s" } else { "" }));
                    }
                    match (res, partial) {
                        (Some(Ok(d)), partial) if limit.is_none() => {
                            let (t, full, lost, errs) = finish(d, &[]);
                            match partial {
                                Some((pt, pfull, plost, perrs)) if plost < lost => {
                                    stage.errors.extend(perrs.0);
                                    stage.tension_limits.extend(perrs.1);
                                    made = Some((pt, pfull));
                                }
                                _ => {
                                    stage.errors.extend(errs.0);
                            stage.tension_limits.extend(errs.1);
                                    made = Some((t, full));
                                }
                            }
                        }
                        (_, Some((t, full, _, errs))) => {
                            stage.errors.extend(errs.0);
                            stage.tension_limits.extend(errs.1);
                            made = Some((t, full));
                        }
                        (Some(Ok(d)), None) => {
                            stage.limits.extend(limit.map(|l| (f.id, l)));
                            let (t, full, _, errs) = finish(d, &[]);
                            stage.errors.extend(errs.0);
                            stage.tension_limits.extend(errs.1);
                            made = Some((t, full));
                        }
                        (Some(Err(e)), None) => stage.errors.push(format!("{}: OpenCascade cannot make it at {} mm ({e})", f.name, r.size)),
                        (None, None) => {}
                    }
                }
                if let Some((s, full)) = made {
                    model.booleans += 1;
                    stage.shape = Some(s);
                    stage.conic.retain(|id| *id != f.id && !full.contains(id));
                    stage.conic.extend(full);
                }
            }
            cache.stages.insert(key, stage.clone());
            continue;
        }
        // an inset or extrusion of a face: on the solid built so far
        if let Some(p) = &f.push {
            if let Some(acc) = stage.shape.clone() {
                let mut limit = None;
                let res = if skipped("push") { Err("skipped, it crashed the kernel".to_string()) } else { push(&acc, f, p, &mut limit) };
                if let Some(l) = limit {
                    stage.limits.push((f.id, l));
                }
                match res {
                    Ok(Some(s)) => {
                        model.booleans += 1;
                        stage.shape = Some(Rc::new(s));
                    }
                    Ok(None) => {}
                    Err(e) => stage.errors.push(format!("{}: {e}", f.name)),
                }
            }
            cache.stages.insert(key, stage.clone());
            continue;
        }
        // a face, edge or vertex moved: on the solid built so far
        if let Some(t) = &f.tweak {
            if let Some(acc) = stage.shape.clone() {
                let res = if skipped("tweak") { Err("skipped, it crashed the kernel".to_string()) } else { tweak(&acc, f, t) };
                match res {
                    Ok(Some(s)) => {
                        model.booleans += 1;
                        stage.shape = Some(Rc::new(s));
                    }
                    Ok(None) => {}
                    Err(e) => match replay(doc, &feats[..i], f, t, &stage, skip, cache, model, used_tools, used_stages) {
                        Some(s) => stage = s,
                        None => stage.errors.push(format!("{}: {e}", f.name)),
                    },
                }
            }
            cache.stages.insert(key, stage.clone());
            continue;
        }
        // the Knife: on the solid built so far
        if let Some(k) = &f.cut {
            if let Some(acc) = stage.shape.clone() {
                let res = if skipped("cut") { Err("skipped, it crashed the kernel".to_string()) } else { knife(&acc, f, k) };
                match res {
                    Ok(Some(s)) => {
                        model.booleans += 1;
                        stage.shape = Some(Rc::new(s));
                    }
                    Ok(None) => {}
                    Err(e) => stage.errors.push(format!("{}: {e}", f.name)),
                }
            }
            cache.stages.insert(key, stage.clone());
            continue;
        }
        // the feature's solid
        let t = if skipped("tool") {
            Err("skipped, it crashed the kernel".to_string())
        } else if let Some(t) = cache.tools.get(&fh) {
            Ok(t.clone())
        } else {
            mark(&step("tool", f));
            tool(f).map(|t| {
                let t = Rc::new(t);
                cache.tools.insert(fh, t.clone());
                t
            })
        };
        used_tools.push(fh);
        let t = match t {
            Ok(t) => t,
            Err(e) => {
                stage.errors.push(format!("{}: {e}", f.name));
                cache.stages.insert(key, stage.clone());
                continue;
            }
        };
        // the boolean
        let next = match (&stage.shape, f.op) {
            (None, Op::Add) => Some(Ok(t)),
            (None, Op::Carve) => None,
            (Some(_), _) if skipped("boolean") => Some(Err("skipped, it crashed the kernel".to_string())),
            (Some(acc), op) => {
                mark(&step("boolean", f));
                model.booleans += 1;
                Some(if op == Op::Add { acc.fuse(&t) } else { acc.cut(&t) }.map(Rc::new))
            }
        };
        match next {
            Some(Ok(s)) => stage.shape = Some(s),
            Some(Err(e)) => stage.errors.push(format!("{}: {} failed ({e})", f.name, if f.op == Op::Add { "union" } else { "subtraction" })),
            None => {}
        }
        cache.stages.insert(key, stage.clone());
    }
    stage
}

/// A face or edge moved (`Tweak`) that cannot be made where fillets or
/// chamfers made just before it run along it: made before them instead,
/// they made again on what it gives (as if it had come first; their edges
/// are found again by their faces' tags). The stage, when that adds no
/// error.
#[allow(clippy::too_many_arguments)]
fn replay<'a>(doc: &'a Doc, before: &[&'a Feature], f: &'a Feature, t: &Tweak, stage: &Stage, skip: &[String], cache: &mut Cache, model: &mut Model, used_tools: &mut Vec<u64>, used_stages: &mut Vec<u64>) -> Option<Stage> {
    let tags: Vec<i64> = match &t.comp {
        Comp::Face(r) => vec![r.tag],
        Comp::Edge(e) => e.faces.to_vec(),
        Comp::Vertex(_) => return None,
    };
    let mut j = before.len();
    while j > 0 && before[j - 1].round.is_some() {
        j -= 1;
    }
    let touches = before[j..].iter().any(|g| g.round.as_ref().is_some_and(|r| r.edges.iter().any(|e| e.faces.iter().any(|x| tags.contains(x)))));
    if !touches {
        return None;
    }
    let order: Vec<&Feature> = before[..j].iter().copied().chain(std::iter::once(f)).chain(before[j..].iter().copied()).collect();
    let s = run(doc, &order, skip, cache, model, used_tools, used_stages);
    (s.shape.is_some() && s.errors.len() <= stage.errors.len()).then_some(s)
}

pub fn build(doc: &Doc, fine: bool, quads: bool, skip: &[String], cache: &mut Cache) -> Model {
    let t0 = Instant::now();
    let mut model = Model { fine, ..Default::default() };
    let mut used_tools = vec![];
    let mut used_stages = vec![];
    let feats: Vec<&Feature> = doc.features.iter().filter(|f| f.visible && !f.suspended && !f.is_path()).collect();
    let stage = run(doc, &feats, skip, cache, &mut model, &mut used_tools, &mut used_stages);
    // keep what this document uses (the next request is most often the same
    // document with one feature changed)
    cache.stages.retain(|k, _| used_stages.contains(k));
    if cache.tools.len() > 256 {
        cache.tools.retain(|k, _| used_tools.contains(k));
    }
    model.errors = stage.errors.clone();
    model.limits = stage.limits.clone();
    model.tension_limits = stage.tension_limits.clone();
    if let Some(s) = &stage.shape {
        let (defl, angle) = if fine { FINE } else { COARSE };
        mark("mesh:0:0");
        match s.topo(defl, angle) {
            Ok(t) => {
                model.positions = t.positions;
                model.normals = t.normals;
                model.triangles = t.triangles;
                model.tri_face = t.tri_face;
                model.faces = t
                    .faces
                    .iter()
                    .map(|f| Face {
                        tag: FaceTag::unpack(f.tag),
                        kind: surf(f.info.kind),
                        point: f.info.point,
                        normal: f.info.normal,
                        axis_origin: f.info.axis_origin,
                        axis_dir: f.info.axis_dir,
                        radius: f.info.radius,
                    })
                    .collect();
                model.edges = t
                    .edges
                    .into_iter()
                    .map(|e| Edge {
                        index: e.index,
                        faces: e.faces,
                        kind: match e.kind {
                            forme_occt::EdgeKind::Line => EdgeKind::Line,
                            forme_occt::EdgeKind::Circle => EdgeKind::Circle,
                            forme_occt::EdgeKind::Curve => EdgeKind::Curve,
                        },
                        seam: e.seam,
                        pts: e.pts.iter().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect(),
                    })
                    .collect();
            }
            Err(e) => model.errors.push(format!("meshing failed ({e})")),
        }
        // the quads: a segment about a fourteenth of the part's size
        if quads {
            mark("quads:0:0");
            let diag = model.bounds().map(|(lo, hi)| ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt() as f64).unwrap_or(20.0);
            match crate::quads::convert(s, (diag / 14.0).clamp(0.5, 20.0)) {
                Ok(q) => {
                    model.quads = Some(crate::model::Quads {
                        positions: q.positions.iter().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect(),
                        quads: q.quads,
                        quad_face: q.quad_face,
                        skipped: q.skipped.into_iter().map(|(f, s)| (f as u32, s)).collect(),
                    })
                }
                Err(e) => model.errors.push(format!("quads: {e}")),
            }
        }
    }
    model.ms = t0.elapsed().as_secs_f32() * 1000.0;
    model
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::KnifePoint;
    use crate::doc::PrimKind;
    use glam::DQuat;

    #[test]
    fn cube_with_a_hole() {
        let mut d = Doc::default();
        let cube = d.add(Op::Add, PrimKind::Box.default_prim(), DVec3::ZERO, DQuat::IDENTITY);
        let hole = d.add(Op::Carve, Prim::Cylinder { radius: 4.0, height: 30.0 }, DVec3::new(0.0, -5.0, 0.0), DQuat::IDENTITY);
        let mut cache = Cache::default();
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.booleans, 1);
        // the cube's 6 faces and the hole's side
        assert_eq!(m.faces.len(), 7);
        let tags: Vec<FaceTag> = m.faces.iter().map(|f| f.tag.unwrap()).collect();
        assert_eq!(tags.iter().filter(|t| t.feature == cube).count(), 6);
        assert_eq!(tags.iter().filter(|t| t.feature == hole).collect::<Vec<_>>(), vec![&FaceTag { feature: hole, sub: 0 }]);
        // the top face is the cube's top (sub 2), a plane facing up at y = 20
        let top = m.faces.iter().find(|f| f.tag == Some(FaceTag { feature: cube, sub: 2 })).unwrap();
        assert_eq!(top.kind, SurfKind::Plane);
        assert!((top.normal[1] - 1.0).abs() < 1e-9 && (top.point[1] - 20.0).abs() < 1e-9);
        // 12 cube edges, 2 rims, the hole's seam
        assert_eq!(m.edges.len(), 15);
        assert_eq!(m.edges.iter().filter(|e| e.seam).count(), 1);
        // moving the hole: the cube comes from the cache
        d.feature_mut(hole).unwrap().origin.x = 2.0;
        let m2 = build(&d, false, false, &[], &mut cache);
        assert_eq!((m2.cached, m2.booleans), (1, 1));
        // a step to skip: the hole is left out
        let s = step("boolean", d.feature(hole).unwrap());
        let m3 = build(&d, false, false, &[s], &mut cache);
        assert_eq!(m3.faces.len(), 6);
        assert_eq!(m3.errors.len(), 1);
        // a carve first: nothing to cut, no error
        let mut d2 = Doc::default();
        d2.add(Op::Carve, PrimKind::Box.default_prim(), DVec3::ZERO, DQuat::IDENTITY);
        let m4 = build(&d2, true, false, &[], &mut Cache::default());
        assert!(m4.is_empty() && m4.errors.is_empty());
        // round trip through the wire
        assert_eq!(Model::decode(&m.encode()).unwrap(), m);
    }

    #[test]
    fn fillets_blend_their_corner() {
        // three fillets made one after the other on the three edges of the
        // same corner of a block
        let mk = |blend: bool| {
            let mut d = Doc::default();
            let b = d.add(Op::Add, Prim::Box { x: 20.0, y: 20.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
            let e = |a: u16, c: u16, at: DVec3| vec![EdgeRef { faces: { let (x, y) = (tag(b, a), tag(b, c)); if x <= y { [x, y] } else { [y, x] } }, at }];
            let r1 = d.add_round(3.0, false, e(2, 4, DVec3::new(0.0, 20.0, 10.0)));
            let r2 = d.add_round(3.0, false, e(0, 4, DVec3::new(10.0, 10.0, 10.0)));
            let r3 = d.add_round(3.0, false, e(2, 0, DVec3::new(10.0, 20.0, 0.0)));
            for id in [r1, r2, r3] {
                d.feature_mut(id).unwrap().round.as_mut().unwrap().blend = blend;
            }
            (build(&d, true, false, &[], &mut Cache::default()), [r1, r2, r3])
        };
        // one after the other, the second fillet runs on along the first's
        // border (tangent) and takes the third's edge
        let (open, _) = mk(false);
        let (blended, [r1, r2, r3]) = mk(true);
        assert!(blended.errors.is_empty(), "{:?}", blended.errors);
        let count = |m: &Model, id: u32| m.faces.iter().filter(|f| f.tag.map(|t| t.feature == id).unwrap_or(false)).count();
        // each fillet keeps its faces (clicking them selects it), the corner
        // belongs to the last one
        assert!(count(&blended, r1) >= 1 && count(&blended, r2) >= 1 && count(&blended, r3) >= 2, "{} {} {}", count(&blended, r1), count(&blended, r2), count(&blended, r3));
        eprintln!("faces: open {} blended {} ({} {} {})", open.faces.len(), blended.faces.len(), count(&blended, r1), count(&blended, r2), count(&blended, r3));
        // the block's 6 faces, one band per fillet, one corner
        assert_eq!((blended.faces.len(), count(&blended, r1), count(&blended, r2), count(&blended, r3)), (10, 1, 1, 2));
    }

    /// The tag of a face of the built model, packed.
    fn tag(feature: u32, sub: u16) -> i64 {
        FaceTag { feature, sub }.pack()
    }

    #[test]
    fn sketches_are_profiles() {
        // an L of 20 × 20 mm, 10 mm high, its inside corner a curve point
        let mut d = Doc::default();
        let l = d.add(Op::Add, Prim::Profile { x: 20.0, y: 10.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let pr = crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, -0.2], [-0.2, -0.2], [-0.2, 0.5], [-0.5, 0.5]], smooth: vec![false, false, false, true, false, false], corner: vec![], tension: vec![], open: false };
        let pieces = pr.pieces(20.0, 20.0).len();
        d.feature_mut(l).unwrap().profile = Some(pr);
        let mut cache = Cache::default();
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, hi) = m.bounds().unwrap();
        assert!((lo[1] - 0.0).abs() < 1e-3 && (hi[1] - 10.0).abs() < 1e-3, "{lo:?} {hi:?}");
        assert!((lo[0] + 10.0).abs() < 1e-3 && (hi[2] - 10.0).abs() < 1e-3, "{lo:?} {hi:?}");
        // top 1, bottom 2, one side per piece, each its own tag
        let subs: Vec<u16> = m.faces.iter().filter_map(|f| f.tag).filter(|t| t.feature == l).map(|t| t.sub).collect();
        assert_eq!(subs.len(), pieces + 2, "{subs:?}");
        assert!(subs.contains(&1) && subs.contains(&2));
        let mut sides: Vec<u16> = subs.iter().copied().filter(|s| *s >= 3).collect();
        sides.sort();
        sides.dedup();
        assert_eq!(sides.len(), pieces, "{subs:?}");
        // the side after point 0 (piece 1: the front, z = -10) is sub 4: its
        // top edge fillets
        let front = m.faces.iter().find(|f| f.tag == Some(FaceTag { feature: l, sub: 4 })).unwrap();
        assert!(front.normal[2] < -0.99, "{:?}", front.normal);
        d.add_round(1.0, false, vec![EdgeRef { faces: [tag(l, 1), tag(l, 4)], at: DVec3::new(0.0, 10.0, -10.0) }]);
        let m2 = build(&d, true, false, &[], &mut cache);
        assert!(m2.errors.is_empty(), "{:?}", m2.errors);
        // a sketch carved into a block, turned upside down (it grows down
        // from the block's top)
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 40.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let c = d.add(Op::Carve, Prim::Profile { x: 20.0, y: 5.0, z: 20.0 }, DVec3::new(0.0, 20.0, 0.0), DQuat::from_rotation_x(std::f64::consts::PI));
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.0, 0.5]], smooth: vec![false, false, true], corner: vec![], tension: vec![], open: false });
        let m3 = build(&d, true, false, &[], &mut cache);
        assert!(m3.errors.is_empty(), "{:?}", m3.errors);
        assert!(m3.faces.iter().any(|f| f.tag.map(|t| t.feature) == Some(c)), "the pocket's faces carry the sketch's tags");
        // a bow tie crosses itself: an error, the block stays
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, 0.5], [0.5, -0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![], tension: vec![], open: false });
        let m4 = build(&d, true, false, &[], &mut cache);
        assert_eq!(m4.errors.len(), 1, "{:?}", m4.errors);
    }

    #[test]
    fn fillets_find_their_edges() {
        let mut d = Doc::default();
        let cube = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let boss = d.add(Op::Add, Prim::Cylinder { radius: 6.0, height: 10.0 }, DVec3::new(8.0, 20.0, 0.0), DQuat::IDENTITY);
        // the cube's top front edge (top, +Z) and the boss's foot (side, cube top)
        let edges = vec![
            EdgeRef { faces: [tag(cube, 2), tag(cube, 4)], at: DVec3::new(0.0, 20.0, 15.0) },
            EdgeRef { faces: [tag(boss, 0), tag(cube, 2)], at: DVec3::new(14.0, 20.0, 0.0) },
        ];
        let r = d.add_round(2.0, false, edges);
        let mut cache = Cache::default();
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        // two new fillet faces, tagged with the fillet
        assert_eq!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).count(), 2);
        // the boss moves: its foot is found again, from the cache of the cube
        d.feature_mut(boss).unwrap().origin.x = -8.0;
        let m2 = build(&d, true, false, &[], &mut cache);
        assert!(m2.errors.is_empty(), "{:?}", m2.errors);
        assert_eq!(m2.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).count(), 2);
        // a chamfer instead
        d.feature_mut(r).unwrap().round.as_mut().unwrap().chamfer = true;
        let m3 = build(&d, true, false, &[], &mut cache);
        assert!(m3.errors.is_empty(), "{:?}", m3.errors);
        let cone = m3.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).any(|f| f.kind == SurfKind::Cone);
        assert!(cone, "the boss's foot chamfered is a cone");
        // the boss gone: one edge not found, the other still rounded
        d.feature_mut(boss).unwrap().visible = false;
        let m4 = build(&d, true, false, &[], &mut cache);
        assert_eq!(m4.errors.len(), 1, "{:?}", m4.errors);
        assert!(m4.errors[0].contains("1 edge not found"));
        assert_eq!(m4.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).count(), 1);
        // too big: made at the largest size that works, and said so
        d.feature_mut(boss).unwrap().visible = true;
        let rr = d.feature_mut(r).unwrap().round.as_mut().unwrap();
        rr.chamfer = false;
        rr.size = 30.0;
        let m5 = build(&d, true, false, &[], &mut cache);
        assert!(m5.errors.is_empty(), "{:?}", m5.errors);
        let limit = m5.limits.iter().find(|(id, _)| *id == r).map(|(_, l)| *l).expect("a limit");
        assert!(limit > 1.0 && limit < 30.0, "{limit}");
        assert!(m5.faces.iter().any(|f| f.tag.map(|t| t.feature) == Some(r)));
        // just under the limit: no limit
        d.feature_mut(r).unwrap().round.as_mut().unwrap().size = limit;
        let m6 = build(&d, true, false, &[], &mut cache);
        assert!(m6.errors.is_empty() && m6.limits.is_empty(), "{:?} {:?}", m6.errors, m6.limits);
        eprintln!("largest fillet: {limit} mm");
        // of the two edges joining the cube's top and a side split by a slot,
        // the one nearest the point is rounded
        let mut d2 = Doc::default();
        let c2 = d2.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d2.add(Op::Carve, Prim::Box { x: 8.0, y: 10.0, z: 40.0 }, DVec3::new(0.0, 12.0, 0.0), DQuat::IDENTITY);
        let r2 = d2.add_round(1.0, false, vec![EdgeRef { faces: [tag(c2, 2), tag(c2, 4)], at: DVec3::new(15.0, 20.0, 15.0) }]);
        let m6 = build(&d2, true, false, &[], &mut Cache::default());
        assert!(m6.errors.is_empty(), "{:?}", m6.errors);
        let ff: Vec<&Face> = m6.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r2)).collect();
        assert_eq!(ff.len(), 1);
        let on_right = m6.triangles.iter().zip(&m6.tri_face).filter(|(_, f)| m6.faces[**f as usize].tag.map(|t| t.feature) == Some(r2)).all(|(t, _)| t.iter().all(|v| m6.positions[*v as usize][0] > 3.9));
        assert!(on_right, "the right piece of the edge");
    }

    #[test]
    fn sketch_corners_cut() {
        // a 20 mm square sketch, 6 mm high: one corner rounded at 4 mm, the
        // next chamfered at 3 mm, a third asked too big (stops at the most
        // its sides allow)
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Profile { x: 20.0, y: 6.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![4.0, -3.0, 50.0, 0.0], tension: vec![], open: false });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let cyl: Vec<f64> = m.faces.iter().filter(|f| f.kind == SurfKind::Cylinder).map(|f| f.radius).collect();
        assert_eq!(cyl.len(), 2, "two rounded corners: {cyl:?}");
        assert!(cyl.iter().any(|r| (r - 4.0).abs() < 1e-6), "{cyl:?}");
        assert!(cyl.iter().any(|r| (r - 10.0 * 0.999).abs() < 1e-3), "the big one stops at half a side: {cyl:?}");
        // the faces: top, bottom, 4 sides (one is gone: two cuts take half
        // of it each), 2 arcs, 1 chamfer
        assert_eq!(m.faces.len(), 2 + 4 + 2 + 1, "{}", m.faces.len());
        // each cut face is tagged as its corner's piece
        let pr = d.feature(c).unwrap().profile.clone().unwrap();
        assert_eq!(pr.cuts(20.0, 20.0).iter().filter(|c| c.is_some()).count(), 3);
        let tags: Vec<u16> = m.faces.iter().filter(|f| f.kind == SurfKind::Cylinder).filter_map(|f| f.tag.map(|t| t.sub)).collect();
        assert!(tags.contains(&3) && tags.contains(&(3 + 4)), "{tags:?}");
    }

    #[test]
    fn edge_tension() {
        // a block's top front edge, a 5 mm fillet, then tension 0.75 and 0.3
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let r = d.add_round(5.0, false, vec![EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(0.0, 20.0, 15.0) }]);
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m0.errors.is_empty(), "{:?}", m0.errors);
        // the corner of the two tangents (0, 20, 15), the tangent points at
        // (0, 15, 15) and (0, 20, 10): the section's middle by rho
        let mid = DVec3::new(0.0, 17.5, 12.5);
        let corner = DVec3::new(0.0, 20.0, 15.0);
        for t in [0.75, 0.3] {
            d.feature_mut(r).unwrap().round.as_mut().unwrap().tension = t;
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "tension {t}: {:?}", m.errors);
            assert_eq!(m.faces.len(), m0.faces.len(), "tension {t}");
            assert!(m.faces.iter().all(|f| f.kind != SurfKind::Cylinder), "tension {t}: still round");
            let want = mid + (corner - mid) * t;
            // the mesh's point nearest the section's middle line (x = 0)
            let near = m.positions.iter().map(|p| DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64)).filter(|p| p.y > 14.9 && p.z > 9.9).map(|p| DVec3::new(0.0, p.y, p.z).distance(want)).fold(f64::MAX, f64::min);
            assert!(near < 0.3, "tension {t}: {near}");
            // the fillet's face keeps the fillet's tag
            assert!(m.faces.iter().any(|f| f.tag.map(|x| x.feature) == Some(r)));
        }
    }

    #[test]
    fn edge_tension_test_part() {
        // the app's test part (block, hole, boss, slot): the top front edge
        // right of the slot, a fillet with tension
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-10.0, -5.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Add, Prim::Cylinder { radius: 7.0, height: 10.0 }, DVec3::new(10.0, 20.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Box { x: 8.0, y: 8.0, z: 40.0 }, DVec3::new(0.0, 14.0, 0.0), DQuat::IDENTITY);
        let r = d.add_round(4.6, false, vec![EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(12.0, 20.0, 15.0) }]);
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m0.errors.is_empty(), "{:?}", m0.errors);
        for t in [0.63, 0.2] {
            d.feature_mut(r).unwrap().round.as_mut().unwrap().tension = t;
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "tension {t}: {:?}", m.errors);
            assert_eq!(m.faces.len(), m0.faces.len(), "tension {t}");
        }
    }

    /// The model edge whose middle is nearest `p`, named by its faces' tags.
    fn edge_near(m: &Model, p: DVec3) -> EdgeRef {
        // an edge's middle: halfway along its polyline
        let mid = |e: &crate::model::Edge| {
            let v: Vec<DVec3> = e.pts.iter().map(|q| DVec3::new(q[0] as f64, q[1] as f64, q[2] as f64)).collect();
            let len: f64 = v.windows(2).map(|w| w[0].distance(w[1])).sum();
            let mut acc = 0.0;
            for w in v.windows(2) {
                let l = w[0].distance(w[1]);
                if acc + l >= len / 2.0 && l > 0.0 {
                    return w[0].lerp(w[1], (len / 2.0 - acc) / l);
                }
                acc += l;
            }
            v[0]
        };
        let e = m.edges.iter().filter(|e| !e.seam && e.faces[0] >= 0 && e.faces[1] >= 0).min_by(|a, b| mid(a).distance(p).total_cmp(&mid(b).distance(p))).unwrap();
        let t = |i: i32| m.faces[i as usize].tag.unwrap().pack();
        EdgeRef { faces: [t(e.faces[0]), t(e.faces[1])], at: mid(e) }
    }

    /// A fillet of `size` on the edge nearest `p`, then each tension: no
    /// error, as many faces, none of the fillet's faces still a cylinder or
    /// a torus.
    fn tension_case(name: &str, mut d: Doc, p: DVec3, size: f64) {
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m0.errors.is_empty(), "{name}: {:?}", m0.errors);
        let e = edge_near(&m0, p);
        let r = d.add_round(size, false, vec![e]);
        let m1 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m1.errors.is_empty(), "{name}: {:?}", m1.errors);
        let round = |m: &Model| m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r) && matches!(f.kind, SurfKind::Cylinder | SurfKind::Torus)).count();
        assert!(round(&m1) > 0, "{name}: no round face");
        for t in [0.75, 0.25] {
            d.feature_mut(r).unwrap().round.as_mut().unwrap().tension = t;
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "{name} tension {t}: {:?}", m.errors);
            assert_eq!(m.faces.len(), m1.faces.len(), "{name} tension {t}");
            assert_eq!(round(&m), 0, "{name} tension {t}: still round");
        }
    }

    #[test]
    fn tension_boss_ring() {
        // the boss's top edge: a torus all round (its seam)
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Add, Prim::Cylinder { radius: 7.0, height: 10.0 }, DVec3::new(10.0, 20.0, 0.0), DQuat::IDENTITY);
        tension_case("boss top", d, DVec3::new(10.0, 30.0, 7.0), 2.0);
    }

    #[test]
    fn tension_boss_foot() {
        // where the boss meets the block: a hollow torus
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Add, Prim::Cylinder { radius: 7.0, height: 10.0 }, DVec3::new(10.0, 20.0, 0.0), DQuat::IDENTITY);
        tension_case("boss foot", d, DVec3::new(10.0, 20.0, 7.0), 2.0);
    }

    #[test]
    fn tension_run() {
        // a square sketch with its corners rounded: its top edge is one run
        // of lines and arcs, the fillet goes all round (cylinders and tori)
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Profile { x: 30.0, y: 10.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![5.0; 4], tension: vec![], open: false });
        tension_case("rounded square", d, DVec3::new(0.0, 10.0, 10.0), 2.0);
    }

    #[test]
    fn tension_oblique_ends() {
        // a trapezoid prism: the fillet on its long top edge ends on the
        // slanted sides
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Profile { x: 30.0, y: 10.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.25, 0.5], [-0.25, 0.5]], smooth: vec![false; 4], corner: vec![], tension: vec![], open: false });
        tension_case("trapezoid", d, DVec3::new(0.0, 10.0, -10.0), 2.0);
    }

    #[test]
    fn tension_rounded_box() {
        // a block with all its 12 edges filleted in one step (8 sphere
        // corners), then with tension: every face conic, the corners
        // remade to fit, nothing left round
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        let mut mids = vec![];
        for (a, b) in [(20.0, 15.0), (20.0, -15.0), (-20.0, 15.0), (-20.0, -15.0)] {
            mids.push(DVec3::new(a, 10.0, b));
        }
        for (y, z) in [(0.0, 15.0), (0.0, -15.0), (20.0, 15.0), (20.0, -15.0)] {
            mids.push(DVec3::new(0.0, y, z));
        }
        for (x, y) in [(20.0, 0.0), (-20.0, 0.0), (20.0, 20.0), (-20.0, 20.0)] {
            mids.push(DVec3::new(x, y, 0.0));
        }
        let r = d.add_round(3.0, false, mids.iter().map(|p| edge_near(&m0, *p)).collect());
        let m1 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m1.errors.is_empty(), "{:?}", m1.errors);
        let kinds = |m: &Model| m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).map(|f| f.kind).collect::<Vec<_>>();
        assert_eq!(kinds(&m1).iter().filter(|k| **k == SurfKind::Sphere).count(), 8, "{:?}", kinds(&m1));
        for t in [0.7, 0.3] {
            d.feature_mut(r).unwrap().round.as_mut().unwrap().tension = t;
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "tension {t}: {:?}", m.errors);
            assert_eq!(m.faces.len(), m1.faces.len(), "tension {t}");
            assert!(kinds(&m).iter().all(|k| *k == SurfKind::Other), "tension {t}: {:?}", kinds(&m));
        }
    }

    #[test]
    fn tension_after_corner() {
        // the block's front right edge chamfered (the fillet's end then
        // lies on the chamfer and the side: its corner moves onto the conic)
        // or filleted (the fillet runs on round it, a torus; with tension it
        // is not blended into it, which would end it on that round face),
        // then the top front edge filleted with tension
        for chamfer in [true, false] {
            let mut d = Doc::default();
            d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            let m0 = build(&d, true, false, &[], &mut Cache::default());
            d.add_round(3.0, chamfer, vec![edge_near(&m0, DVec3::new(20.0, 10.0, 15.0))]);
            let m1 = build(&d, true, false, &[], &mut Cache::default());
            let r = d.add_round(if chamfer { 4.0 } else { 2.0 }, false, vec![edge_near(&m1, DVec3::new(0.0, 20.0, 15.0))]);
            for t in [0.75, 0.25] {
                d.feature_mut(r).unwrap().round.as_mut().unwrap().tension = t;
                let m = build(&d, true, false, &[], &mut Cache::default());
                let what = format!("after a {} tension {t}", if chamfer { "chamfer" } else { "fillet" });
                assert!(m.errors.is_empty(), "{what}: {:?}", m.errors);
                let kinds: Vec<SurfKind> = m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).map(|f| f.kind).collect();
                assert!(!kinds.is_empty() && kinds.iter().all(|k| !matches!(k, SurfKind::Cylinder | SurfKind::Torus)), "{what}: {kinds:?}");
            }
        }
    }

    #[test]
    fn tension_next_fillets() {
        // a fillet with tension, then another on an edge meeting it at the
        // block's corner: the same size (blended into one mitre), a
        // smaller one (made on its own, on the conic faces): the first
        // keeps its tension, the second takes its own
        let round = |m: &Model, id: u32| m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(id) && matches!(f.kind, SurfKind::Cylinder | SurfKind::Torus)).count();
        for (size, tension2) in [(4.0, 0.7), (4.0, 0.0), (3.0, 0.0), (3.0, 0.3)] {
            let mut d = Doc::default();
            d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            let m0 = build(&d, true, false, &[], &mut Cache::default());
            let a = d.add_round(4.0, false, vec![edge_near(&m0, DVec3::new(0.0, 20.0, 15.0))]);
            d.feature_mut(a).unwrap().round.as_mut().unwrap().tension = 0.7;
            let b = d.add_round(size, false, vec![edge_near(&m0, DVec3::new(20.0, 20.0, 0.0))]);
            d.feature_mut(b).unwrap().round.as_mut().unwrap().tension = tension2;
            let m = build(&d, true, false, &[], &mut Cache::default());
            let what = format!("second fillet {size} mm, tension {tension2}");
            assert!(m.limits.is_empty(), "{what}: {:?}", m.limits);
            assert_eq!(round(&m, a), 0, "{what}: the first one turned round");
            if size == 4.0 {
                // the same size: one mitre, the second conic when it asks
                assert!(m.errors.is_empty(), "{what}: {:?}", m.errors);
                assert_eq!(round(&m, b) == 0, tension2 > 0.0, "{what}");
            } else {
                // smaller: made on its own, its end runs onto the first one's
                // conic face (a face of its own between that face and the
                // block's side, made conic between its two ends)
                assert!(m.errors.is_empty(), "{what}: {:?}", m.errors);
                assert_eq!(round(&m, b) == 0, tension2 > 0.0, "{what}");
            }
        }
    }

    #[test]
    #[ignore]
    fn tension_probe() {
        // what tension does at a box corner: two edges, three edges
        for n in [2usize, 3] {
            let mut d = Doc::default();
            d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            let m0 = build(&d, true, false, &[], &mut Cache::default());
            let pts = [DVec3::new(0.0, 20.0, 15.0), DVec3::new(20.0, 20.0, 0.0), DVec3::new(20.0, 10.0, 15.0)];
            let edges: Vec<EdgeRef> = pts[..n].iter().map(|p| edge_near(&m0, *p)).collect();
            let r = d.add_round(3.0, false, edges);
            let m1 = build(&d, true, false, &[], &mut Cache::default());
            let kinds: Vec<SurfKind> = m1.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).map(|f| f.kind).collect();
            eprintln!("{n} edges: errors {:?} fillet faces {kinds:?}", m1.errors);
            for t in [0.75, 0.25] {
                d.feature_mut(r).unwrap().round.as_mut().unwrap().tension = t;
                let m = build(&d, true, false, &[], &mut Cache::default());
                let kinds: Vec<SurfKind> = m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).map(|f| f.kind).collect();
                eprintln!("  tension {t}: errors {:?} faces {kinds:?}", m.errors);
            }
        }
        // a chamfer next to a tensioned fillet: how far it goes
        for t in [0.0, 0.2, 0.35, 0.4142, 0.45, 0.5, 0.6, 0.75] {
            let mut d = Doc::default();
            d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            let m0 = build(&d, true, false, &[], &mut Cache::default());
            let e = edge_near(&m0, DVec3::new(0.0, 20.0, 15.0));
            let r = d.add_round(4.0, false, vec![e]);
            d.feature_mut(r).unwrap().round.as_mut().unwrap().tension = t;
            let m1 = build(&d, true, false, &[], &mut Cache::default());
            let e2 = edge_near(&m1, DVec3::new(20.0, 10.0, 15.0));
            let c = d.add_round(3.0, true, vec![e2]);
            let m2 = build(&d, true, false, &[], &mut Cache::default());
            eprintln!("chamfer 3 next to fillet 4 tension {t}: errors {:?} limits {:?}", m2.errors, m2.limits.iter().filter(|l| l.0 == c).collect::<Vec<_>>());
            let e3 = edge_near(&m1, DVec3::new(20.0, 20.0, 0.0));
            let f = d.add_round(3.0, false, vec![e3]);
            d.feature_mut(c).unwrap().visible = false;
            let m3 = build(&d, true, false, &[], &mut Cache::default());
            eprintln!("  fillet 3 on the right top edge: errors {:?} limits {:?}", m3.errors, m3.limits.iter().filter(|l| l.0 == f).collect::<Vec<_>>());
        }
    }

    #[test]
    fn tension_two_sizes() {
        // a box's vertical edges rounded at 5 mm and its top edges at 3 mm
        // (then 3 and 5), in both orders: each fillet keeps its own tension, the top one also
        // where it runs round the vertical one (made after it, along its
        // conic; made before it, the vertical one's corner follows it). The
        // vertical one's conic must stay rounder than 3 mm there (rho 0.365
        // to 0.515 at 5 mm) when the top one runs round it
        let kinds = |m: &Model, r: u32| m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).map(|f| f.kind).collect::<Vec<_>>();
        let vert = [DVec3::new(20.0, 10.0, 15.0), DVec3::new(-20.0, 10.0, 15.0), DVec3::new(20.0, 10.0, -15.0), DVec3::new(-20.0, 10.0, -15.0)];
        let top = [DVec3::new(0.0, 20.0, 15.0), DVec3::new(0.0, 20.0, -15.0), DVec3::new(20.0, 20.0, 0.0), DVec3::new(-20.0, 20.0, 0.0)];
        let mut circ = 0;
        // (vertical first, vertical's tension, top's tension)
        let cases = [(true, 0.0, 0.0), (true, 0.5, 0.7), (true, 0.0, 0.7), (true, 0.45, 0.0), (true, 0.45, 0.3), (false, 0.0, 0.0), (false, 0.0, 0.7), (false, 0.5, 0.0), (false, 0.5, 0.7), (false, 0.45, 0.3), (false, 0.38, 0.7)];
        // the vertical one smaller than the top one (3 under 5): OpenCascade's
        // corner between them (a face of the second, a spline) is remade
        // with the tensions of the faces at its ends, and follows the top
        // fillet's face it runs along when that one turns conic
        let small = [(true, 0.0, 0.0), (true, 0.0, 0.7), (true, 0.5, 0.0), (true, 0.5, 0.7), (true, 0.3, 0.6), (false, 0.0, 0.0), (false, 0.0, 0.7), (false, 0.7, 0.0), (false, 0.5, 0.7), (false, 0.6, 0.3)];
        let all = cases.iter().map(|c| (5.0, 3.0, *c)).chain(small.iter().map(|c| (3.0, 5.0, *c)));
        for (rv, rt, (vfirst, tv, tt)) in all {
            let mut d = Doc::default();
            d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            let m0 = build(&d, true, false, &[], &mut Cache::default());
            let (first, r1, t1, second, r2, t2) = if vfirst { (vert, rv, tv, top, rt, tt) } else { (top, rt, tt, vert, rv, tv) };
            let a = d.add_round(r1, false, first.iter().map(|p| edge_near(&m0, *p)).collect());
            d.feature_mut(a).unwrap().round.as_mut().unwrap().tension = t1;
            let m1 = build(&d, true, false, &[], &mut Cache::default());
            // the second edges: their middles moved by the first
            let b = d.add_round(r2, false, second.iter().map(|p| edge_near(&m1, *p)).collect());
            d.feature_mut(b).unwrap().round.as_mut().unwrap().tension = t2;
            let m = build(&d, true, false, &[], &mut Cache::default());
            let what = format!("vertical {rv} first {vfirst}, tensions {t1} {t2}");
            assert!(m.errors.is_empty(), "{what}: {:?}", m.errors);
            if t1 == 0.0 && t2 == 0.0 {
                circ = m.faces.len();
            }
            assert_eq!(m.faces.len(), circ, "{what}");
            assert!(m.tension_limits.is_empty(), "{what}: {:?}", m.tension_limits);
            // a fillet with tension keeps no round face (made before the top
            // one, the vertical one's corners are the top one's section: round
            // when the top one has none)
            let round = |k: &SurfKind| matches!(k, SurfKind::Cylinder | SurfKind::Torus);
            if t1 > 0.0 {
                assert!(!kinds(&m, a).iter().any(round), "{what}: {:?}", kinds(&m, a));
            }
            if t2 > 0.0 && (vfirst || tt > 0.0 || rv < rt) {
                assert!(!kinds(&m, b).iter().any(round), "{what}: {:?}", kinds(&m, b));
            }
        }
        // the vertical one too full (0.7: its conic curves down to 1.5 mm,
        // less than the 3 mm fillet running round it) or too flat (0.3):
        // its tension stops at the nearest that works (its curve 3.3 mm at
        // least), 0.515 or 0.365
        for (tv, stop) in [(0.7, 0.515), (0.3, 0.365)] {
            let mut d = Doc::default();
            d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            let m0 = build(&d, true, false, &[], &mut Cache::default());
            let a = d.add_round(3.0, false, top.iter().map(|p| edge_near(&m0, *p)).collect());
            d.feature_mut(a).unwrap().round.as_mut().unwrap().tension = 0.7;
            let m1 = build(&d, true, false, &[], &mut Cache::default());
            let b = d.add_round(5.0, false, vert.iter().map(|p| edge_near(&m1, *p)).collect());
            d.feature_mut(b).unwrap().round.as_mut().unwrap().tension = tv;
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "vertical {tv}: {:?}", m.errors);
            let l = m.tension_limits.iter().find(|l| l.0 == b).map(|l| l.1).expect("a tension limit");
            assert!((l - stop).abs() < 0.006, "vertical {tv}: stops at {l}");
            assert!(!kinds(&m, b).iter().any(|k| matches!(k, SurfKind::Cylinder | SurfKind::Torus)), "vertical {tv}: {:?}", kinds(&m, b));
        }
    }

    #[test]
    #[ignore]
    fn tension_probe2() {
        // two sizes on a box: the vertical edges first, then the top ones
        // (they run round the first), and the other way round
        let kinds = |m: &Model, r: u32| m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).map(|f| f.kind).collect::<Vec<_>>();
        let vert = [DVec3::new(20.0, 10.0, 15.0), DVec3::new(-20.0, 10.0, 15.0), DVec3::new(20.0, 10.0, -15.0), DVec3::new(-20.0, 10.0, -15.0)];
        let top = [DVec3::new(0.0, 20.0, 15.0), DVec3::new(0.0, 20.0, -15.0), DVec3::new(20.0, 20.0, 0.0), DVec3::new(-20.0, 20.0, 0.0)];
        for (name, first, second, r1, r2) in [("vertical 5 then top 3", vert, top, 5.0, 3.0), ("top 3 then vertical 5", top, vert, 3.0, 5.0), ("top 5 then vertical 3", top, vert, 5.0, 3.0), ("vertical 3 then top 5", vert, top, 3.0, 5.0)] {
            for (t1, t2) in [(0.7, 0.7), (0.0, 0.7), (0.7, 0.0), (0.3, 0.3)] {
                let mut d = Doc::default();
                d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
                let m0 = build(&d, true, false, &[], &mut Cache::default());
                let a = d.add_round(r1, false, first.iter().map(|p| edge_near(&m0, *p)).collect());
                d.feature_mut(a).unwrap().round.as_mut().unwrap().tension = t1;
                let m1 = build(&d, true, false, &[], &mut Cache::default());
                // the second edges: their middles moved by the first
                let b = d.add_round(r2, false, second.iter().map(|p| edge_near(&m1, *p)).collect());
                d.feature_mut(b).unwrap().round.as_mut().unwrap().tension = t2;
                let m = build(&d, true, false, &[], &mut Cache::default());
                eprintln!("{name}, tensions {t1} {t2}: errors {:?}\n   first {:?}\n   second {:?}", m.errors, kinds(&m, a), kinds(&m, b));
            }
        }
    }

    #[test]
    #[ignore]
    fn tension_probe3() {
        // the topology of the top 3 then vertical 5 case, circular
        let vert = [DVec3::new(20.0, 10.0, 15.0), DVec3::new(-20.0, 10.0, 15.0), DVec3::new(20.0, 10.0, -15.0), DVec3::new(-20.0, 10.0, -15.0)];
        let top = [DVec3::new(0.0, 20.0, 15.0), DVec3::new(0.0, 20.0, -15.0), DVec3::new(20.0, 20.0, 0.0), DVec3::new(-20.0, 20.0, 0.0)];
        let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        let size = |k: &str, or: f64| Some(env(k)).filter(|v| *v > 0.0).unwrap_or(or);
        // FORME_VFIRST=1: the vertical edges first (R1 / T1 still the top's)
        let (first, second) = if env("FORME_VFIRST") > 0.0 { (vert, top) } else { (top, vert) };
        let a = d.add_round(size("FORME_R1", 3.0), false, first.iter().map(|p| edge_near(&m0, *p)).collect());
        d.feature_mut(a).unwrap().round.as_mut().unwrap().tension = env("FORME_T1");
        let m1 = build(&d, true, false, &[], &mut Cache::default());
        let b = d.add_round(size("FORME_R2", 5.0), false, second.iter().map(|p| edge_near(&m1, *p)).collect());
        d.feature_mut(b).unwrap().round.as_mut().unwrap().tension = env("FORME_T2");
        let m = build(&d, true, false, &[], &mut Cache::default());
        eprintln!("fillets {a} {b}, errors {:?}, tension limits {:?}", m.errors, m.tension_limits);
        for (i, f) in m.faces.iter().enumerate() {
            let t = f.tag.unwrap();
            if t.feature != a && t.feature != b {
                continue;
            }
            if !(f.point[0] > 0.0 && f.point[2] > 0.0) {
                continue;
            }
            let nb: Vec<String> = m.edges.iter().filter(|e| e.faces.contains(&(i as i32))).map(|e| {
                let o = if e.faces[0] == i as i32 { e.faces[1] } else { e.faces[0] };
                let of = &m.faces[o as usize];
                format!("{}:{}/{:?}{}", of.tag.unwrap().feature, of.tag.unwrap().sub, of.kind, if e.seam { " seam" } else { "" })
            }).collect();
            eprintln!("face {i} fillet {} {:?} r {:.2} at {:.2?}: {nb:?}", t.feature, f.kind, f.radius, f.point);
        }
    }

    #[test]
    #[ignore]
    fn tension_probe4() {
        // two top edges meeting at a corner (the vertical edge there sharp):
        // the front one (R1, T1), then the -X one (R2, T2)
        let env = |k: &str, or: f64| std::env::var(k).ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(or);
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let a = d.add_round(env("FORME_R1", 2.0), false, vec![EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(0.0, 20.0, 15.0) }]);
        d.feature_mut(a).unwrap().round.as_mut().unwrap().tension = env("FORME_T1", 0.0);
        let c = d.add_round(env("FORME_R2", 3.0), false, vec![EdgeRef { faces: [tag(b, 2), tag(b, 1)], at: DVec3::new(-20.0, 20.0, 0.0) }]);
        d.feature_mut(c).unwrap().round.as_mut().unwrap().tension = env("FORME_T2", 0.7);
        d.feature_mut(c).unwrap().round.as_mut().unwrap().blend = env("FORME_BLEND", 1.0) > 0.0;
        let m = build(&d, true, false, &[], &mut Cache::default());
        eprintln!("fillets {a} {c}, errors {:?}, tension limits {:?}", m.errors, m.tension_limits);
        for (i, f) in m.faces.iter().enumerate() {
            let t = f.tag.unwrap();
            if t.feature != a && t.feature != c {
                continue;
            }
            let nb: Vec<String> = m.edges.iter().filter(|e| e.faces.contains(&(i as i32))).map(|e| {
                let o = if e.faces[0] == i as i32 { e.faces[1] } else { e.faces[0] };
                let of = &m.faces[o as usize];
                format!("{}:{}/{:?}", of.tag.unwrap().feature, of.tag.unwrap().sub, of.kind)
            }).collect();
            eprintln!("face {i} fillet {} {:?} r {:.2} at {:.2?}: {nb:?}", t.feature, f.kind, f.radius, f.point);
        }
    }

    #[test]
    fn corner_tension() {
        // a 20 mm square sketch, one corner a 6 mm fillet: circular, then
        // with tension 0.7 (fuller) and 0.3 (flatter)
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Profile { x: 20.0, y: 6.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let mut pr = crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![0.0, 6.0, 0.0, 0.0], tension: vec![], open: false };
        d.feature_mut(c).unwrap().profile = Some(pr.clone());
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m0.errors.is_empty(), "{:?}", m0.errors);
        // the circle's own rho at a right angle: sin 45° / (1 + sin 45°)
        let cut = pr.cuts(20.0, 20.0)[1].unwrap();
        assert!((cut.rho_or_circle() - crate::doc::circle_rho(std::f64::consts::FRAC_PI_2)).abs() < 1e-9, "{}", cut.rho_or_circle());
        for t in [0.7, 0.3] {
            pr.tension = vec![0.0, t, 0.0, 0.0];
            d.feature_mut(c).unwrap().profile = Some(pr.clone());
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "tension {t}: {:?}", m.errors);
            assert_eq!(m.faces.len(), m0.faces.len(), "tension {t}");
            let cut = pr.cuts(20.0, 20.0)[1].unwrap();
            assert!((cut.rho - t).abs() < 1e-9);
            // the corner's face is no longer a cylinder
            assert!(m.faces.iter().filter(|f| f.kind == SurfKind::Cylinder).count() == 0, "tension {t}");
            // the mesh reaches the conic's middle
            let mid = DVec3::new(cut.m[0], 3.0, cut.m[1]);
            let near = m.positions.iter().map(|p| DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64).distance(mid)).fold(f64::MAX, f64::min);
            assert!(near < 0.6, "tension {t}: {near}");
        }
    }

    #[test]
    fn paths_have_no_solid() {
        // a block and an open path (an L of 3 points, its middle corner
        // rounded at 5 mm) over it: the path changes nothing in the part
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        let p = d.add(Op::Add, Prim::Path { x: 20.0, z: 20.0 }, DVec3::new(0.0, 20.0, 0.0), DQuat::IDENTITY);
        let pr = crate::doc::Profile { points: vec![[-0.5, 0.5], [-0.5, -0.5], [0.5, -0.5]], smooth: vec![false; 3], corner: vec![0.0, 5.0, 0.0], tension: vec![], open: true };
        d.feature_mut(p).unwrap().profile = Some(pr.clone());
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.len(), m0.faces.len());
        // open: a side, the arc, a side; no closing side; the ends uncut
        let ids: Vec<usize> = pr.pieces(20.0, 20.0).iter().map(|pc| pc.id).collect();
        assert_eq!(ids, vec![1, 2, 3], "{ids:?}");
        let line = pr.polyline(20.0, 20.0);
        assert!((line[0][0] + 10.0).abs() < 1e-9 && (line[0][1] - 10.0).abs() < 1e-9, "{:?}", line[0]);
        assert!((line.last().unwrap()[0] - 10.0).abs() < 1e-9, "it ends on its last point");
        let c = pr.cuts(20.0, 20.0);
        assert!(c[0].is_none() && c[2].is_none() && c[1].map(|c| (c.size - 5.0).abs() < 1e-9).unwrap_or(false));
        // its outline: one polyline, on its plane
        let o = d.feature(p).unwrap().outline();
        assert_eq!(o.len(), 1);
        assert!(o[0].iter().all(|q| (q.y - 20.0).abs() < 1e-9));
    }

    #[test]
    fn knife_cuts() {
        // a block: a point in its top front edge, then a line across its
        // top from that edge's middle to the back edge's middle
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        let front = KnifePoint { edge: EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(0.0, 20.0, 15.0) }, f: 0.5, at: DVec3::new(0.0, 20.0, 15.0) };
        let back = KnifePoint { edge: EdgeRef { faces: [tag(b, 2), tag(b, 5)], at: DVec3::new(0.0, 20.0, -15.0) }, f: 0.5, at: DVec3::new(0.0, 20.0, -15.0) };
        let k = d.add_knife(Knife { pts: vec![front.clone()], segs: vec![] }, "Points");
        let m1 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m1.errors.is_empty(), "{:?}", m1.errors);
        assert_eq!(m1.faces.len(), m0.faces.len());
        assert_eq!(m1.edges.len(), m0.edges.len() + 1, "the edge cut in two");
        d.feature_mut(k).unwrap().cut = Some(Knife { pts: vec![front, back], segs: vec![[0, 1]] });
        assert_eq!(d.feature(k).unwrap().kind_label(), "Knife");
        let m2 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m2.errors.is_empty(), "{:?}", m2.errors);
        assert_eq!(m2.faces.len(), m0.faces.len() + 1, "the top in two");
        let tops = m2.faces.iter().filter(|f| f.tag.map(|t| t.pack()) == Some(tag(b, 2))).count();
        assert_eq!(tops, 2, "both halves keep the top's tag");
        // a fillet after it still finds the front right edge
        d.add_round(2.0, false, vec![EdgeRef { faces: [tag(b, 0), tag(b, 4)], at: DVec3::new(20.0, 10.0, 15.0) }]);
        let m3 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m3.errors.is_empty(), "{:?}", m3.errors);
    }

    #[test]
    fn knife_on_test_part() {
        // the test part (block, hole, boss, slot): a line across the top
        // left of the slot (through the hole or not) and right of the boss
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-10.0, -5.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Add, Prim::Cylinder { radius: 7.0, height: 10.0 }, DVec3::new(10.0, 20.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Box { x: 8.0, y: 8.0, z: 40.0 }, DVec3::new(0.0, 14.0, 0.0), DQuat::IDENTITY);
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        for x in [-10.0, -15.0, 18.0] {
            let mut d = d.clone();
            let kp = |z: f64, side: u16| KnifePoint { edge: EdgeRef { faces: [tag(b, 2), tag(b, side)], at: DVec3::new(x, 20.0, z) }, f: 0.5, at: DVec3::new(x, 20.0, z) };
            // the fraction along the edge for x: the front edge runs -20..-4 or 4..20
            let (lo, hi) = if x < 0.0 { (-20.0, -4.0) } else { (4.0, 20.0) };
            let f = (x - lo) / (hi - lo);
            let mut a = kp(15.0, 4);
            a.f = f;
            let mut c = kp(-15.0, 5);
            c.f = f;
            d.add_knife(Knife { pts: vec![a, c], segs: vec![[0, 1]] }, "Knife");
            let m = build(&d, true, false, &[], &mut Cache::default());
            // the top cut in two, across the hole when it crosses it
            assert!(m.errors.is_empty(), "x {x}: {:?}", m.errors);
            assert_eq!(m.faces.len(), m0.faces.len() + 1, "x {x}");
        }
    }

    #[test]
    fn knife_on_curved_faces() {
        // the boss's side and the hole's wall cut from their top circle to
        // their bottom one, twice each: straight up, and turning round
        // (a line straight in the face's (u, v), on the surface)
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let h = d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-10.0, -5.0, 0.0), DQuat::IDENTITY);
        let s = d.add(Op::Add, Prim::Cylinder { radius: 7.0, height: 10.0 }, DVec3::new(10.0, 20.0, 0.0), DQuat::IDENTITY);
        let mut cache = Cache::default();
        let m0 = build(&d, true, false, &[], &mut cache);
        assert!(m0.errors.is_empty(), "{:?}", m0.errors);
        let part = cache.stages.values().filter_map(|s| s.shape.clone()).max_by_key(|s| s.face_count()).unwrap();
        let on = |c: DVec3, r: f64, y: f64, a: f64| c + DVec3::new(r * a.to_radians().cos(), y, r * a.to_radians().sin());
        // the point on the edge nearest `at`, by its fraction (as a click)
        let kp = |faces: [i64; 2], at: DVec3| {
            let edge = EdgeRef { faces, at };
            let i = resolve(&part, std::slice::from_ref(&edge)).0[0];
            let f = (0..=1000).map(|k| k as f64 / 1000.0).min_by(|a, b| {
                let d = |f: f64| DVec3::from_array(part.edge_point(i, f).unwrap()).distance(at);
                d(*a).total_cmp(&d(*b))
            });
            KnifePoint { edge, f: f.unwrap(), at }
        };
        // (side tag, top tag, bottom tag, centre, radius, top y, bottom y)
        let cases = [
            (tag(s, 0), tag(s, 1), tag(b, 2), DVec3::new(10.0, 0.0, 0.0), 7.0, 30.0, 20.0),
            (tag(h, 0), tag(b, 2), tag(b, 3), DVec3::new(-10.0, 0.0, 0.0), 5.0, 20.0, 0.0),
        ];
        for (side, top, bottom, c, r, y1, y0) in cases {
            // from a point off the seam, from the seam's own vertex (90°)
            for (a0, turn) in [(60.0, 0.0), (60.0, 40.0), (60.0, -70.0), (90.0, 0.0), (90.0, 40.0)] {
                let mut d = d.clone();
                let pts = vec![
                    kp([side, top], on(c, r, y1, a0)),
                    kp([side, bottom], on(c, r, y0, a0 + turn)),
                    kp([side, bottom], on(c, r, y0, a0 + 180.0)),
                    kp([side, top], on(c, r, y1, a0 + 180.0 + turn)),
                ];
                d.add_knife(Knife { pts, segs: vec![[0, 1], [2, 3]] }, "Knife");
                let m = build(&d, true, false, &[], &mut Cache::default());
                assert!(m.errors.is_empty(), "{a0} turn {turn}: {:?}", m.errors);
                // the side in two, both halves keep its tag
                let n = |m: &Model| m.faces.iter().filter(|f| f.tag.map(|t| t.pack()) == Some(side)).count();
                assert_eq!(n(&m), n(&m0) + 1, "{a0} turn {turn}");
                assert_eq!(m.faces.len(), m0.faces.len() + 1, "{a0} turn {turn}");
            }
        }
        // across a fillet's face, from its edge on the top to its edge on
        // the front, slanted
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let r = d.add_round(4.0, false, vec![EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(0.0, 20.0, 15.0) }]);
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m0.errors.is_empty(), "{:?}", m0.errors);
        let kp = |faces: [i64; 2], at: DVec3, f: f64| KnifePoint { edge: EdgeRef { faces, at }, f, at };
        // (a fillet on a box's edge runs it all, x -20 to 20)
        d.add_knife(
            Knife { pts: vec![kp([tag(b, 2), tag(r, 0)], DVec3::new(-5.0, 20.0, 11.0), 0.375), kp([tag(b, 4), tag(r, 0)], DVec3::new(5.0, 16.0, 15.0), 0.625)], segs: vec![[0, 1]] },
            "Knife",
        );
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.len(), m0.faces.len() + 1);
    }

    #[test]
    fn tilted_faces_with_holes() {
        // a face with holes, bosses or pockets that tilts (an edge of it
        // raised or lowered, the face moved along its normal): their walls
        // run along to the new plane; what cannot is refused with a reason
        let block = |d: &mut Doc| d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let left_edge = |b: u32, dy: f64| Tweak { comp: Comp::Edge(EdgeRef { faces: [tag(b, 2), tag(b, 1)], at: DVec3::new(-20.0, 20.0, 0.0) }), delta: DVec3::new(0.0, dy, 0.0) };
        let right_edge = |b: u32, dy: f64| Tweak { comp: Comp::Edge(EdgeRef { faces: [tag(b, 2), tag(b, 0)], at: DVec3::new(20.0, 20.0, 0.0) }), delta: DVec3::new(0.0, dy, 0.0) };
        let mut cases: Vec<(&str, Doc, Option<&str>)> = vec![];
        // a boss on the top, the top's -X edge up then down
        for dy in [3.0, -3.0] {
            let mut d = Doc::default();
            let b = block(&mut d);
            d.add(Op::Add, Prim::Cylinder { radius: 6.0, height: 8.0 }, DVec3::new(8.0, 20.0, 0.0), DQuat::IDENTITY);
            d.add_tweak(left_edge(b, dy));
            cases.push((if dy > 0.0 { "boss, edge up" } else { "boss, edge down" }, d, None));
        }
        // a hole through, the top moved up along its normal
        let mut d = Doc::default();
        let b = block(&mut d);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-5.0, -5.0, 0.0), DQuat::IDENTITY);
        d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(b, 2), at: DVec3::new(10.0, 20.0, 10.0) }), delta: DVec3::new(0.0, 3.0, 0.0) });
        cases.push(("hole, top moved up", d, None));
        // a square pocket 6 deep: the top's +X edge up; its -X edge down
        // 10 (the tilt would pass the pocket's floor)
        for (name, dy, want) in [("pocket, edge up", 3.0, None), ("pocket, past its floor", -10.0, Some("past"))] {
            let mut d = Doc::default();
            let b = block(&mut d);
            d.add(Op::Carve, Prim::Box { x: 10.0, y: 6.0, z: 8.0 }, DVec3::new(-8.0, 14.0, 0.0), DQuat::IDENTITY);
            d.add_tweak(if dy > 0.0 { right_edge(b, dy) } else { left_edge(b, dy) });
            cases.push((name, d, want));
        }
        // a sketch carved 10 deep, one side a curve
        let mut d = Doc::default();
        let b = block(&mut d);
        let s = d.add(Op::Carve, Prim::Profile { x: 12.0, y: 20.0, z: 10.0 }, DVec3::new(-6.0, 10.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(s).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [0.0, 0.1], [-0.5, 0.5]], smooth: vec![false, false, false, true, false], corner: vec![], tension: vec![], open: false });
        d.add_tweak(right_edge(b, 3.0));
        cases.push(("sketch pocket, curved side", d, None));
        // a hole whose mouth is chamfered: its slanted wall cannot slide,
        // the top is tilted before the chamfer, the chamfer made again
        let mut d = Doc::default();
        let b = block(&mut d);
        let h = d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-5.0, -5.0, 0.0), DQuat::IDENTITY);
        d.add_round(1.0, true, vec![EdgeRef { faces: [tag(b, 2), tag(h, 0)], at: DVec3::new(0.0, 20.0, 0.0) }]);
        d.add_tweak(right_edge(b, 3.0));
        cases.push(("hole, chamfered mouth", d, None));
        // a hole tilted, then its mouth chamfered 1.5 mm (its new edge
        // smooth where it closes: a kink there held the chamfer to 0.9)
        let mut d = Doc::default();
        let b = block(&mut d);
        let h = d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-5.0, -5.0, 0.0), DQuat::IDENTITY);
        d.add_tweak(Tweak { comp: Comp::Edge(EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(6.0, 20.0, 15.0) }), delta: DVec3::new(0.0, 6.0, 0.0) });
        d.add_round(1.5, true, vec![EdgeRef { faces: [tag(b, 2), tag(h, 0)], at: DVec3::new(0.0, 23.0, 0.0) }]);
        cases.push(("hole tilted, then chamfered", d, None));
        // the test part: the top's left half (the hole) tilted round the
        // slot's edge
        let (mut d, b, _) = test_part();
        d.add_tweak(left_edge(b, 3.0));
        cases.push(("test part, hole side", d, None));
        let (mut d, b, _) = test_part();
        d.add_tweak(right_edge(b, 3.0));
        // (the boss's foot, cut by the slot, has a point off the line the
        // top turns round: the top bends round it, a smooth plate)
        cases.push(("test part, boss side", d, None));
        // a corner of a top with a hole raised: the top bends (not yet)
        let mut d = Doc::default();
        block(&mut d);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-5.0, -5.0, 0.0), DQuat::IDENTITY);
        d.add_tweak(Tweak { comp: Comp::Vertex(DVec3::new(20.0, 20.0, 15.0)), delta: DVec3::new(0.0, 3.0, 0.0) });
        cases.push(("hole, corner up (bends)", d, Some("cannot bend")));

        let mut bad = vec![];
        for (name, d, want) in &cases {
            let m = build(d, true, false, &[], &mut Cache::default());
            eprintln!("{name:<28} {} faces, errors {:?}", m.faces.len(), m.errors);

            match want {
                None if !m.errors.is_empty() || !m.limits.is_empty() => bad.push(format!("{name}: {:?}, limits {:?}", m.errors, m.limits)),
                Some(w) if !m.errors.iter().any(|e| e.contains(w)) => bad.push(format!("{name}: wanted \"{w}\", got {:?}", m.errors)),
                _ => {}
            }
            if want.is_none() && m.errors.is_empty() {
                // a fillet on every edge of the tilted top's holes and
                // bosses still works (1 mm)
                let tops: Vec<i32> = m.faces.iter().enumerate().filter(|(_, f)| f.tag == Some(FaceTag { feature: 1, sub: 2 })).map(|(i, _)| i as i32).collect();
                let edges: Vec<EdgeRef> = m.edges.iter().filter(|e| !e.seam && (tops.contains(&e.faces[0]) || tops.contains(&e.faces[1]))).filter_map(|e| {
                    let o = if tops.contains(&e.faces[0]) { e.faces[1] } else { e.faces[0] };
                    let t = m.faces.get(usize::try_from(o).ok()?)?.tag?;
                    // (to the walls: not to a chamfer made after)
                    (t.feature != 1 && d.feature(t.feature).is_some_and(|g| g.round.is_none())).then(|| EdgeRef { faces: [tag(1, 2), tag(t.feature, t.sub)], at: { let p = e.pts[e.pts.len() / 2]; DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64) } })
                }).collect();
                if !edges.is_empty() {
                    let mut d = d.clone();
                    let n = edges.len();
                    d.add_round(1.0, false, edges);
                    let m = build(&d, true, false, &[], &mut Cache::default());
                    eprintln!("{:<28} + fillet on {n} edges: errors {:?}", "", m.errors);
                    if !m.errors.is_empty() {
                        bad.push(format!("{name}, fillet after: {:?}", m.errors));
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{} failed:\n{}", bad.len(), bad.join("\n"));
    }

    /// The tools one after another on the test part: every ordered pair of
    /// a list of operations (fillets, chamfers, insets, extrusions, moves,
    /// a tilt, the Knife), each pair built on its own; prints what fails.
    /// `FORME_COMBO=name` keeps the pairs whose names contain it.
    #[test]
    #[ignore]
    fn combo_battery() {
        let (base, b, s) = test_part();
        let h = 2u32;
        let er = |a: i64, c: i64, x: f64, y: f64, z: f64| EdgeRef { faces: [a, c], at: DVec3::new(x, y, z) };
        let fr = |t: i64, x: f64, y: f64, z: f64| FaceRef { tag: t, at: DVec3::new(x, y, z) };
        type Op_ = Box<dyn Fn(&mut Doc)>;
        let round = |size: f64, chamfer: bool, tension: f64, e: EdgeRef| -> Op_ {
            Box::new(move |d: &mut Doc| {
                let r = d.add_round(size, chamfer, vec![e.clone()]);
                d.feature_mut(r).unwrap().round.as_mut().unwrap().tension = tension;
            })
        };
        let push = |face: FaceRef, inset: f64, depth: f64| -> Op_ { Box::new(move |d: &mut Doc| { d.add_push(Push { face: face.clone(), inset, depth }); }) };
        let tweak = |comp: Comp, delta: DVec3| -> Op_ { Box::new(move |d: &mut Doc| { d.add_tweak(Tweak { comp: comp.clone(), delta }); }) };
        let ops: Vec<(&str, Op_)> = vec![
            ("fillet top-front", round(2.0, false, 0.0, er(tag(b, 2), tag(b, 4), -12.0, 20.0, 15.0))),
            ("tension fillet top -X", round(3.0, false, 0.7, er(tag(b, 2), tag(b, 1), -20.0, 20.0, 0.0))),
            ("chamfer side vertical", round(2.0, true, 0.0, er(tag(b, 0), tag(b, 4), 20.0, 10.0, 15.0))),
            ("fillet boss top", round(1.0, false, 0.0, er(tag(s, 0), tag(s, 1), 17.0, 30.0, 0.0))),
            ("fillet hole mouth", round(1.0, false, 0.0, er(tag(b, 2), tag(h, 0), -5.0, 20.0, 0.0))),
            ("inset front", push(fr(tag(b, 4), -12.0, 6.0, 15.0), 2.0, 0.0)),
            ("extrude front", push(fr(tag(b, 4), -12.0, 6.0, 15.0), 0.0, 3.0)),
            ("extrude +X in", push(fr(tag(b, 0), 20.0, 8.0, 0.0), 0.0, -2.0)),
            ("extrude boss side", push(fr(tag(s, 0), 17.0, 25.0, 0.0), 0.0, 1.0)),
            ("inset slot floor", push(fr(tag(4, 3), 0.0, 14.0, 0.0), 1.0, -1.0)),
            ("move top-left up", tweak(Comp::Face(fr(tag(b, 2), -12.0, 20.0, 8.0)), DVec3::new(0.0, 3.0, 0.0))),
            ("tilt top-left", tweak(Comp::Edge(er(tag(b, 2), tag(b, 1), -20.0, 20.0, 0.0)), DVec3::new(0.0, 3.0, 0.0))),
            ("move +X out", tweak(Comp::Face(fr(tag(b, 0), 20.0, 8.0, 0.0)), DVec3::new(4.0, 0.0, 0.0))),
            ("knife front", Box::new(move |d: &mut Doc| {
                let pts = vec![
                    KnifePoint { edge: er(tag(b, 1), tag(b, 4), -20.0, 10.0, 15.0), f: 0.5, at: DVec3::new(-20.0, 10.0, 15.0) },
                    KnifePoint { edge: er(tag(b, 0), tag(b, 4), 20.0, 10.0, 15.0), f: 0.5, at: DVec3::new(20.0, 10.0, 15.0) },
                ];
                d.add_knife(Knife { pts, segs: vec![[0, 1]] }, "Knife");
            })),
        ];
        let only = std::env::var("FORME_COMBO").ok();
        let mut bad = vec![];
        let mut n = 0;
        for (an, a) in &ops {
            // each alone first
            let mut d = base.clone();
            a(&mut d);
            let m = build(&d, true, false, &[], &mut Cache::default());
            if !m.errors.is_empty() {
                eprintln!("ALONE {an}: {:?}", m.errors);
                bad.push(format!("{an}: {:?}", m.errors));
                continue;
            }
            for (bn, bo) in &ops {
                if an == bn {
                    continue;
                }
                let name = format!("{an} → {bn}");
                if only.as_ref().is_some_and(|o| !name.contains(o.as_str())) {
                    continue;
                }
                let mut d = base.clone();
                a(&mut d);
                bo(&mut d);
                let t = Instant::now();
                let m = build(&d, true, false, &[], &mut Cache::default());
                n += 1;
                // (the second naming a face or an edge the first took away
                // is no fault: listed apart)
                let lost = |e: &String| e.contains("is gone") || e.contains("are gone") || e.contains("not found");
                if m.errors.iter().any(|e| !lost(e)) {
                    eprintln!("FAIL {name}: {:?} ({:.2} s)", m.errors, t.elapsed().as_secs_f64());
                    bad.push(format!("{name}: {:?}", m.errors));
                } else if !m.errors.is_empty() {
                    eprintln!("GONE {name}: {:?}", m.errors);
                }
            }
        }
        eprintln!("{} of {n} pairs failed", bad.len());
        assert!(bad.is_empty(), "{} failed:\n{}", bad.len(), bad.join("\n"));
    }

    #[test]
    fn moves_next_to_fillets() {
        // a face moved where a fillet or chamfer made before runs along it:
        // made as if moved before them, they made again on it
        let top_edges = |b: u32| {
            vec![
                EdgeRef { faces: [tag(b, 2), tag(b, 0)], at: DVec3::new(20.0, 20.0, 0.0) },
                EdgeRef { faces: [tag(b, 2), tag(b, 1)], at: DVec3::new(-20.0, 20.0, 0.0) },
                EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(0.0, 20.0, 15.0) },
                EdgeRef { faces: [tag(b, 2), tag(b, 5)], at: DVec3::new(0.0, 20.0, -15.0) },
            ]
        };
        let top = |b: u32, dy: f64| Tweak { comp: Comp::Face(FaceRef { tag: tag(b, 2), at: DVec3::new(0.0, 20.0, 0.0) }), delta: DVec3::new(0.0, dy, 0.0) };
        let mut cases: Vec<(&str, Doc, f64, u32)> = vec![];
        for (name, chamfer, tension, dy) in [("top up, filleted", false, 0.0, 5.0), ("top down, filleted", false, 0.0, -4.0), ("top up, chamfered", true, 0.0, 5.0), ("top up, fillet with tension", false, 0.7, 5.0)] {
            let mut d = Doc::default();
            let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            let r = d.add_round(3.0, chamfer, top_edges(b));
            d.feature_mut(r).unwrap().round.as_mut().unwrap().tension = tension;
            d.add_tweak(top(b, dy));
            cases.push((name, d, 20.0 + dy, r));
        }
        // a vertical edge filleted, the +X side moved out
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let r = d.add_round(4.0, false, vec![EdgeRef { faces: [tag(b, 0), tag(b, 4)], at: DVec3::new(20.0, 10.0, 15.0) }]);
        d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(b, 0), at: DVec3::new(20.0, 10.0, 0.0) }), delta: DVec3::new(5.0, 0.0, 0.0) });
        cases.push(("side out, vertical fillet", d, 20.0, r));
        // the same, the top front edge raised (the front tilts)
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let r = d.add_round(4.0, false, vec![EdgeRef { faces: [tag(b, 0), tag(b, 4)], at: DVec3::new(20.0, 10.0, 15.0) }]);
        d.add_tweak(Tweak { comp: Comp::Edge(EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(0.0, 20.0, 15.0) }), delta: DVec3::new(0.0, 3.0, 0.0) });
        cases.push(("top front edge up, vertical fillet", d, 23.0, r));
        // the test part, its outer vertical edges filleted, its top moved
        // up (the hole, the boss, the slot follow)
        let (mut d, b, _) = test_part();
        let r = d.add_round(3.0, false, [(20.0, 15.0, 0, 4), (20.0, -15.0, 0, 5), (-20.0, 15.0, 1, 4), (-20.0, -15.0, 1, 5)].iter().map(|&(x, z, a, c)| EdgeRef { faces: [tag(b, a), tag(b, c)], at: DVec3::new(x, 10.0, z) }).collect());
        d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(b, 0), at: DVec3::new(20.0, 10.0, 0.0) }), delta: DVec3::new(4.0, 0.0, 0.0) });
        cases.push(("test part, side out, filleted", d, 30.0, r));
        let mut bad = vec![];
        for (name, d, high, r) in &cases {
            let m = build(d, true, false, &[], &mut Cache::default());
            let rounds = m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(*r)).count();
            let (_, hi) = m.bounds().unwrap_or(([0.0; 3], [0.0; 3]));
            eprintln!("{name:<36} errors {:?}, {rounds} round faces, top {:.3}, +x {:.3}", m.errors, hi[1], hi[0]);
            if d.feature(*r).and_then(|g| g.round.as_ref()).is_some_and(|g| g.tension > 0.0) && m.faces.iter().any(|f| f.tag.map(|t| t.feature) == Some(*r) && matches!(f.kind, SurfKind::Cylinder)) {
                bad.push(format!("{name}: the tension is lost"));
            }
            if !m.errors.is_empty() || rounds == 0 || (hi[1] as f64 - high).abs() > 1e-3 {
                bad.push(format!("{name}: {:?}, {rounds} round faces, top {}", m.errors, hi[1]));
            }
        }
        assert!(bad.is_empty(), "{} failed:\n{}", bad.len(), bad.join("\n"));
    }

    #[test]
    fn faces_moved_next_to_curved_faces() {
        // the test part: the boss's top up and down (its round side
        // follows), the block's top up and down (the hole's wall, the
        // boss's side and the slot's walls follow)
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-10.0, -5.0, 0.0), DQuat::IDENTITY);
        let s = d.add(Op::Add, Prim::Cylinder { radius: 7.0, height: 10.0 }, DVec3::new(10.0, 20.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Box { x: 8.0, y: 8.0, z: 40.0 }, DVec3::new(0.0, 14.0, 0.0), DQuat::IDENTITY);
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m0.errors.is_empty(), "{:?}", m0.errors);
        let cases = [
            (tag(s, 1), DVec3::new(10.0, 30.0, 0.0), 5.0, 35.0),
            (tag(s, 1), DVec3::new(10.0, 30.0, 0.0), -4.0, 26.0),
            (tag(b, 2), DVec3::new(-15.0, 20.0, 10.0), 3.0, 23.0),
            (tag(b, 2), DVec3::new(-15.0, 20.0, 10.0), -2.0, 18.0),
            // the top's right part, under the boss: up, the boss shorter;
            // down, the boss's side runs down to it
            (tag(b, 2), DVec3::new(15.0, 20.0, 10.0), 3.0, 23.0),
            (tag(b, 2), DVec3::new(15.0, 20.0, 10.0), -2.0, 18.0),
        ];
        for (t, at, dy, y) in cases {
            let mut d = d.clone();
            d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: t, at }), delta: DVec3::new(0.0, dy, 0.0) });
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "{dy}: {:?}", m.errors);
            assert_eq!(m.faces.len(), m0.faces.len(), "{dy}");
            // (the block's top is in two, either side of the slot)
            let f = m.faces.iter().find(|f| f.tag.map(|g| g.pack()) == Some(t) && f.point[0].signum() == at.x.signum()).unwrap();
            assert!((f.point[1] - y).abs() < 1e-6, "{dy}: {:?}", f.point);
            // the part's top: the boss's top
            let top = m.positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
            assert!((top - if t == tag(s, 1) { y as f32 } else { 30.0 }).abs() < 1e-3, "{dy}: top {top}");
        }
        // a fillet round the boss's top: it does not run along the move,
        // the top is moved before it and the fillet made again
        let mut d = d.clone();
        let r = d.add_round(1.0, false, vec![EdgeRef { faces: [tag(s, 0), tag(s, 1)], at: DVec3::new(17.0, 30.0, 0.0) }]);
        let before = build(&d, true, false, &[], &mut Cache::default()).positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(s, 1), at: DVec3::new(10.0, 30.0, 0.0) }), delta: DVec3::new(0.0, 2.0, 0.0) });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let top = m.positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        assert!((top - before - 2.0).abs() < 1e-3, "top {top}, before {before}");
        assert!(m.faces.iter().any(|f| f.tag.map(|t| t.feature) == Some(r)), "the fillet is there");
    }

    #[test]
    fn components_moved() {
        // a block with a hole through its top: its +X face moved out 5 mm,
        // then its top front edge raised, then a corner pulled
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let h = d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-5.0, -5.0, 0.0), DQuat::IDENTITY);
        let mv = d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(b, 0), at: DVec3::new(20.0, 10.0, 0.0) }), delta: DVec3::new(5.0, 0.0, 0.0) });
        assert_eq!(d.feature(mv).unwrap().kind_label(), "Move face");
        let mut cache = Cache::default();
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let side = m.faces.iter().find(|f| f.tag == Some(FaceTag { feature: b, sub: 0 })).unwrap();
        assert!((side.point[0] - 25.0).abs() < 1e-6, "{:?}", side.point);
        // the front top edge (top, +Z) raised 4 mm: the top tilts
        d.add_tweak(Tweak { comp: Comp::Edge(EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(2.5, 20.0, 15.0) }), delta: DVec3::new(0.0, 4.0, 0.0) });
        let m = build(&d, true, false, &[], &mut cache);
        // the top has the hole: it tilts, the hole's wall runs up to it
        // (its top edge 22 mm high at z = 0, 22.67 at its front, z = 5)
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let top = m.faces.iter().find(|f| f.tag == Some(FaceTag { feature: b, sub: 2 })).unwrap();
        assert!(matches!(top.kind, SurfKind::Plane) && top.normal[2] < -0.1, "the top leans: {:?}", top.normal);
        let wall = m.faces.iter().position(|f| f.tag == Some(FaceTag { feature: h, sub: 0 })).unwrap();
        let high = m.tri_face.iter().enumerate().filter(|(_, &f)| f as usize == wall).flat_map(|(t, _)| m.triangles[t].iter().map(|&i| m.positions[i as usize][1])).fold(0.0f32, f32::max);
        assert!((high - 22.667).abs() < 0.01, "the wall's top: {high}");
        // a fillet on the hole's new edge
        d.add_round(1.0, false, vec![EdgeRef { faces: [tag(b, 2), tag(h, 0)], at: DVec3::new(0.0, 22.0, 0.0) }]);
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        d.features.pop();
        d.features.pop();
        // the front face's bottom edge pushed forward 4 mm: the front tilts
        d.add_tweak(Tweak { comp: Comp::Edge(EdgeRef { faces: [tag(b, 3), tag(b, 4)], at: DVec3::new(2.5, 0.0, 15.0) }), delta: DVec3::new(0.0, 0.0, 4.0) });
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let front = m.faces.iter().find(|f| f.tag == Some(FaceTag { feature: b, sub: 4 })).unwrap();
        assert!(front.normal[1] > 0.1, "the front leans back: {:?}", front.normal);
        // a fillet after the moves finds its edge (front, +X side)
        d.add_round(2.0, false, vec![EdgeRef { faces: [tag(b, 0), tag(b, 4)], at: DVec3::new(25.0, 10.0, 17.0) }]);
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        // a cube's top corner pulled up: three faces bend; a fillet after it
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Box { x: 20.0, y: 20.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add_tweak(Tweak { comp: Comp::Vertex(DVec3::new(10.0, 20.0, 10.0)), delta: DVec3::new(0.0, 4.0, 0.0) });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!(m.positions.iter().any(|p| (p[1] - 24.0).abs() < 1e-4), "the corner is up");
        d.add_round(1.0, false, vec![EdgeRef { faces: [tag(c, 1), tag(c, 5)], at: DVec3::new(-10.0, 10.0, -10.0) }]);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        // a face of five sides that bends: a cube's top front edge
        // chamfered (its +X side a pentagon), then the bottom front corner
        // of that side pushed out 3 mm; a fillet after it
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Box { x: 20.0, y: 20.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add_round(3.0, true, vec![EdgeRef { faces: [tag(c, 2), tag(c, 4)], at: DVec3::new(0.0, 20.0, 10.0) }]);
        d.add_tweak(Tweak { comp: Comp::Vertex(DVec3::new(10.0, 0.0, 10.0)), delta: DVec3::new(3.0, 0.0, 0.0) });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!(m.positions.iter().any(|p| (p[0] - 13.0).abs() < 1e-3), "the corner is out");
        let side = m.faces.iter().find(|f| f.tag == Some(FaceTag { feature: c, sub: 0 })).unwrap();
        assert!(!matches!(side.kind, SurfKind::Plane), "the side bends: {:?}", side.kind);
        d.add_round(1.0, false, vec![EdgeRef { faces: [tag(c, 0), tag(c, 3)], at: DVec3::new(11.5, 0.0, 0.0) }]);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        // an L sketch's top (six sides) bent: an outer corner raised 3 mm
        let mut d = Doc::default();
        let l = d.add(Op::Add, Prim::Profile { x: 20.0, y: 10.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(l).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, -0.2], [-0.2, -0.2], [-0.2, 0.5], [-0.5, 0.5]], smooth: vec![false; 6], corner: vec![], tension: vec![], open: false });
        d.add_tweak(Tweak { comp: Comp::Vertex(DVec3::new(10.0, 10.0, -10.0)), delta: DVec3::new(0.0, 3.0, 0.0) });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!(m.positions.iter().any(|p| (p[1] - 13.0).abs() < 1e-3), "the corner is up");
        let top = m.faces.iter().find(|f| f.tag == Some(FaceTag { feature: l, sub: 1 })).unwrap();
        assert!(!matches!(top.kind, SurfKind::Plane), "the top bends: {:?}", top.kind);
    }

    /// The test part of the app (block, hole, boss, slot).
    fn test_part() -> (Doc, u32, u32) {
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-10.0, -5.0, 0.0), DQuat::IDENTITY);
        let boss = d.add(Op::Add, Prim::Cylinder { radius: 7.0, height: 10.0 }, DVec3::new(10.0, 20.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Box { x: 8.0, y: 8.0, z: 40.0 }, DVec3::new(0.0, 14.0, 0.0), DQuat::IDENTITY);
        (d, b, boss)
    }

    #[test]
    fn inset_battery() {
        // Romain (19 nov.): "l'Inset ne fonctionne pas toujours et pas dans
        // tous les cas". Many faces, several sizes, line only, raised, sunk.
        let mut cases: Vec<(String, Doc, FaceRef)> = vec![];
        let (d, b, boss) = test_part();
        cases.push(("box top".into(), { let mut d = Doc::default(); d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY); d }, FaceRef { tag: tag(1, 2), at: DVec3::new(0.0, 20.0, 0.0) }));
        cases.push(("top with hole".into(), d.clone(), FaceRef { tag: tag(b, 2), at: DVec3::new(-15.0, 20.0, 10.0) }));
        cases.push(("top with boss".into(), d.clone(), FaceRef { tag: tag(b, 2), at: DVec3::new(15.0, 20.0, 10.0) }));
        cases.push(("front, notched".into(), d.clone(), FaceRef { tag: tag(b, 4), at: DVec3::new(10.0, 8.0, 15.0) }));
        cases.push(("bottom with hole".into(), d.clone(), FaceRef { tag: tag(b, 3), at: DVec3::new(10.0, 0.0, 10.0) }));
        cases.push(("side".into(), d.clone(), FaceRef { tag: tag(b, 0), at: DVec3::new(20.0, 8.0, 0.0) }));
        cases.push(("boss top (disc)".into(), d.clone(), FaceRef { tag: tag(boss, 1), at: DVec3::new(10.0, 30.0, 0.0) }));
        cases.push(("slot floor".into(), d.clone(), FaceRef { tag: tag(4, 3), at: DVec3::new(0.0, 10.0, 0.0) }));
        // a sketch: an L with a curve point, and a triangle (sharp corners)
        let mut l = Doc::default();
        let li = l.add(Op::Add, Prim::Profile { x: 20.0, y: 10.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        l.feature_mut(li).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, -0.2], [-0.2, -0.2], [-0.2, 0.5], [-0.5, 0.5]], smooth: vec![false, false, false, true, false, false], corner: vec![], tension: vec![], open: false });
        cases.push(("sketch L, curve".into(), l, FaceRef { tag: tag(li, 1), at: DVec3::new(-7.0, 10.0, -7.0) }));
        let mut t = Doc::default();
        let ti = t.add(Op::Add, Prim::Profile { x: 30.0, y: 6.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        t.feature_mut(ti).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.0, 0.5]], smooth: vec![false; 3], corner: vec![], tension: vec![], open: false });
        cases.push(("sketch triangle".into(), t, FaceRef { tag: tag(ti, 1), at: DVec3::new(0.0, 6.0, -3.0) }));
        let mut r = Doc::default();
        let ri = r.add(Op::Add, Prim::Profile { x: 30.0, y: 6.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        r.feature_mut(ri).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![3.0, 3.0, -2.0, 6.0], tension: vec![], open: false });
        cases.push(("sketch rounded corners".into(), r, FaceRef { tag: tag(ri, 1), at: DVec3::new(0.0, 6.0, 0.0) }));
        // a top face after a fillet on its front edge
        let mut f = Doc::default();
        let fb = f.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        f.add_round(4.0, false, vec![EdgeRef { faces: [tag(fb, 2), tag(fb, 4)], at: DVec3::new(0.0, 20.0, 15.0) }]);
        cases.push(("top next to a fillet".into(), f, FaceRef { tag: tag(fb, 2), at: DVec3::new(0.0, 20.0, 0.0) }));
        // a cylinder lying down: its end
        let mut c = Doc::default();
        let ci = c.add(Op::Add, Prim::Cylinder { radius: 8.0, height: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        cases.push(("cylinder end".into(), c, FaceRef { tag: tag(ci, 1), at: DVec3::new(0.0, 20.0, 0.0) }));

        let mut bad = vec![];
        for (name, doc, face) in &cases {
            let only = std::env::var("INSET_ONLY").ok();
            if only.as_ref().map(|o| !name.starts_with(o.as_str())).unwrap_or(false) {
                continue;
            }
            for (inset, depth) in [(2.0, 0.0), (2.0, 3.0), (2.0, -2.0), (0.5, -1.0), (6.0, 0.0), (0.0, 2.0)] {
                let mut d = doc.clone();
                let base = build(&d, true, false, &[], &mut Cache::default());
                let p = d.add_push(Push { face: face.clone(), inset, depth });
                let m = build(&d, true, false, &[], &mut Cache::default());
                let mine = m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(p)).count();
                let ok = m.errors.is_empty() && mine > 0;
                let limit = m.limits.iter().find(|l| l.0 == p).map(|l| l.1);
                eprintln!("{:<24} inset {inset:>4} depth {depth:>5}: {} faces ({} before), its own {mine}, limit {limit:?}, errors {:?}", name, m.faces.len(), base.faces.len(), m.errors);
                if !ok {
                    bad.push(format!("{name} inset {inset} depth {depth}: {:?}", m.errors));
                }
            }
        }
        assert!(bad.is_empty(), "{} failed:\n{}", bad.len(), bad.join("\n"));
    }

    #[test]
    fn curved_faces_extruded() {
        // a cylinder's side pulled out 3 mm (a thicker cylinder), pushed in
        // 2 mm; a fillet's face pulled out 1 mm; an inset on a curved face
        // is refused
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Cylinder { radius: 10.0, height: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let side = FaceRef { tag: tag(c, 0), at: DVec3::new(10.0, 10.0, 0.0) };
        let p = d.add_push(Push { face: side, inset: 0.0, depth: 3.0 });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, hi) = m.bounds().unwrap();
        // (the bounds are the mesh's: a little inside on round faces)
        assert!((hi[0] - 13.0).abs() < 0.02 && (lo[2] + 13.0).abs() < 0.02 && (hi[1] - 20.0).abs() < 1e-3, "{lo:?} {hi:?}");
        let mine = |m: &Model, sub: u16| m.faces.iter().filter(|f| f.tag == Some(FaceTag { feature: p, sub })).count();
        assert!(mine(&m, 1) >= 1, "{:?}", m.faces.iter().map(|f| f.tag).collect::<Vec<_>>());
        d.feature_mut(p).unwrap().push.as_mut().unwrap().depth = -2.0;
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, hi) = m.bounds().unwrap();
        assert!((hi[0] - 8.0).abs() < 0.02 && (lo[0] + 8.0).abs() < 0.02, "{lo:?} {hi:?}");
        d.feature_mut(p).unwrap().push.as_mut().unwrap().inset = 1.0;
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.iter().any(|e| e.contains("inset needs a flat face")), "{:?}", m.errors);
        // a block's fillet face pulled out
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let r = d.add_round(5.0, false, vec![EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(0.0, 20.0, 15.0) }]);
        let m1 = build(&d, true, false, &[], &mut Cache::default());
        let fl = m1.faces.iter().find(|f| f.tag.map(|t| t.feature) == Some(r)).unwrap();
        let p = d.add_push(Push { face: FaceRef { tag: tag(r, 0), at: DVec3::from_array(fl.point) }, inset: 0.0, depth: 1.0 });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!(m.faces.iter().any(|f| f.tag == Some(FaceTag { feature: p, sub: 1 }) && f.kind == SurfKind::Cylinder), "{:?}", m.faces.iter().map(|f| (f.tag, f.kind)).collect::<Vec<_>>());
    }

    #[test]
    fn faces_pushed_and_pulled() {
        // a block with a hole through it; its top inset by 3 mm
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(0.0, -5.0, 0.0), DQuat::IDENTITY);
        let top = FaceRef { tag: tag(b, 2), at: DVec3::new(12.0, 20.0, 8.0) };
        let p = d.add_push(Push { face: top, inset: 3.0, depth: 0.0 });
        assert_eq!(d.feature(p).unwrap().kind_label(), "Inset");
        let mut cache = Cache::default();
        let mine = |m: &Model, sub: u16| m.faces.iter().filter(|f| f.tag == Some(FaceTag { feature: p, sub })).count();
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        // the top in three: the frame, the ring round the hole, the inside
        assert_eq!(mine(&m, 1), 1);
        assert_eq!(m.faces.iter().filter(|f| f.tag == Some(FaceTag { feature: b, sub: 2 })).count(), 2);
        // a pocket 4 mm deep: its floor at 16 mm, facing up, and its walls
        d.feature_mut(p).unwrap().push.as_mut().unwrap().depth = -4.0;
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let floor = m.faces.iter().find(|f| f.tag == Some(FaceTag { feature: p, sub: 1 })).unwrap();
        assert!((floor.point[1] - 16.0).abs() < 1e-6 && floor.normal[1] > 0.99, "{:?} {:?}", floor.point, floor.normal);
        assert!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature == p && t.sub >= 2).unwrap_or(false)).count() >= 5);
        // its floor's edges fillet like any other
        let e = m.edges.iter().find(|e| e.faces.iter().all(|f| *f >= 0 && m.faces[*f as usize].tag.map(|t| t.feature) == Some(p))).unwrap();
        let at = DVec3::from_array(e.pts[e.pts.len() / 2].map(|x| x as f64));
        let faces = e.faces.map(|f| m.faces[f as usize].tag.unwrap().pack());
        let r = d.add_round(1.0, false, vec![EdgeRef { faces, at }]);
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).count(), 1);
        d.remove(&[r]);
        // a raised panel: 2 mm up
        d.feature_mut(p).unwrap().push.as_mut().unwrap().depth = 2.0;
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!((m.bounds().unwrap().1[1] - 22.0).abs() < 1e-3);
        // the whole top (Extrude) down 5 mm, then up 5 mm
        let pu = d.feature_mut(p).unwrap().push.as_mut().unwrap();
        pu.inset = 0.0;
        pu.depth = -5.0;
        assert_eq!(d.feature(p).unwrap().kind_label(), "Extrude");
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!((m.bounds().unwrap().1[1] - 15.0).abs() < 1e-3);
        assert_eq!(mine(&m, 1), 1);
        d.feature_mut(p).unwrap().push.as_mut().unwrap().depth = 5.0;
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!((m.bounds().unwrap().1[1] - 25.0).abs() < 1e-3);
        // its flat sides lie on the block's: merged, they keep the block's
        // names (a fillet on them is found again); the hole's wall goes on
        // as a face of its own
        let flat = m.faces.iter().filter(|f| f.kind == SurfKind::Plane && f.tag.map(|t| t.feature == p && t.sub >= 2).unwrap_or(false)).count();
        assert_eq!(flat, 0);
        assert!(m.faces.iter().any(|f| f.tag == Some(FaceTag { feature: b, sub: 4 })));
        // wider than the face: it stops at the largest that works
        let pu = d.feature_mut(p).unwrap().push.as_mut().unwrap();
        pu.inset = 20.0;
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let l = m.limits.iter().find(|l| l.0 == p).map(|l| l.1).expect("a limit");
        assert!(l > 0.0 && l < 20.0, "{l}");
        // a round face (the hole's wall): moved along its normals; no inset on it
        d.feature_mut(p).unwrap().push = Some(Push { face: FaceRef { tag: tag(2, 0), at: DVec3::new(5.0, 10.0, 0.0) }, inset: 0.0, depth: 1.0 });
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        d.feature_mut(p).unwrap().push.as_mut().unwrap().inset = 1.0;
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.len() == 1 && m.errors[0].contains("flat"), "{:?}", m.errors);
    }

    #[test]
    fn rotated_and_on_a_face() {
        // a cylinder lying on its side (axis along X) on top of a cube
        let mut d = Doc::default();
        d.add(Op::Add, PrimKind::Box.default_prim(), DVec3::ZERO, DQuat::IDENTITY);
        let c = d.add(Op::Add, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-15.0, 20.0, 0.0), DQuat::from_rotation_z(-std::f64::consts::FRAC_PI_2));
        let f = d.feature(c).unwrap();
        assert!((f.axis() - DVec3::X).length() < 1e-9);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, hi) = m.bounds().unwrap();
        assert!((lo[0] + 15.0).abs() < 0.01 && (hi[0] - 15.0).abs() < 0.01, "{lo:?} {hi:?}");
        assert!((hi[1] - 25.0).abs() < 0.01);
    }
}
