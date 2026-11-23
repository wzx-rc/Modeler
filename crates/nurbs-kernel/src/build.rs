//! The document built with OpenCascade (crate forme-occt): each feature's
//! solid, its faces tagged (`FaceTag`), then the booleans in order. Used by
//! the kernel process; on iPad it will run in the app itself.
//!
//! Caches, for live editing: the solid of each feature (by content) and the
//! result after each feature (by the content of everything up to it). While
//! one feature moves, only the booleans from it onwards are made again.

use crate::doc::{Comp, Doc, EdgeRef, FaceRef, Feature, FeatureId, Knife, Makes, Mirror, Op, Prim, Push, Round, Shell, Tweak};
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
    // FORME_STAGE_MS=1: how long the step before took (the time from its
    // mark to this one; "mesh" ends a build, the next build's first mark
    // gives the meshing's)
    if std::env::var_os("FORME_STAGE_MS").is_some() {
        thread_local! { static LAST: std::cell::RefCell<Option<(Instant, String)>> = const { std::cell::RefCell::new(None) }; }
        LAST.with(|l| {
            let mut l = l.borrow_mut();
            if let Some((t, what)) = l.take() {
                let _ = writeln!(e, "TIME {what} {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
            }
            *l = Some((Instant::now(), s.to_string()));
        });
    }
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
    /// The move just made took its mirror image with it (one move of
    /// both: a face across the plane bends once): the image step is done.
    paired: bool,
    /// Where it has been: the boxes (min, max) of its lumps after each
    /// step, and of the shapes added to it or cut from it (`build_islands`:
    /// islands whose boxes come near each other are built as one).
    ext: Vec<[f64; 6]>,
    /// The shape `ext` was last told of (its address; 0: none).
    noted: usize,
    /// The Mirrors made on it: false when none of it was on the side kept
    /// (an island: it is gone from there on).
    mirrors: Vec<(FeatureId, bool)>,
    /// The plane it was last made symmetric across (a Mirror's axis and
    /// offset) and how many errors it had then: the same Mirror again,
    /// with no error since, changes nothing.
    even: Option<(u8, u64, usize)>,
    /// Its faces of one surface may not be one yet: the last step that
    /// changed it was no union or cut (a fillet, a move, the Knife, a first
    /// shape). In one history the next union or cut anywhere joins them:
    /// an island does it when one is made in another island meanwhile.
    rough: bool,
}

impl Stage {
    fn new() -> Stage {
        Stage { shape: None, errors: vec![], limits: vec![], conic: vec![], tension_limits: vec![], paired: false, ext: vec![], noted: 0, mirrors: vec![], even: None, rough: false }
    }
    /// Its faces of one surface made one (the shape kept when none is).
    /// The faces of one surface made one, as a union or a cut made
    /// anywhere in one history does to the whole part; `times`: how many of
    /// them were made meanwhile (one pass may leave more to merge: a face
    /// that took another's tag is then no longer kept from its neighbour).
    /// How many passes changed it (fewer than `times`: nothing is left to
    /// merge, `rough` is off).
    fn join_faces(&mut self, times: usize) -> usize {
        let mut n = 0;
        for _ in 0..times {
            let (true, Some(s)) = (self.rough, self.shape.clone()) else { break };
            match s.simplified() {
                Ok(t) if t.face_count() != s.face_count() || t.edge_faces().len() != s.edge_faces().len() => {
                    self.shape = Some(Rc::new(t));
                    n += 1;
                }
                // (nothing left to merge)
                _ => self.rough = false,
            }
        }
        if self.shape.is_none() {
            self.rough = false;
        }
        n
    }
    /// Its shape's lumps added to where it has been, once per shape.
    fn note(&mut self) {
        let p = self.shape.as_ref().map(|s| Rc::as_ptr(s) as usize).unwrap_or(0);
        if p != self.noted {
            self.noted = p;
            if let Some(s) = &self.shape {
                boxes_add(&mut self.ext, &s.lump_boxes());
            }
        }
    }
}

/// The stage after a feature, kept for the next builds.
fn keep(cache: &mut Cache, key: u64, stage: &mut Stage) {
    stage.note();
    cache.stages.insert(key, stage.clone());
}

#[derive(Default)]
pub struct Cache {
    tools: HashMap<u64, Rc<Shape>>,
    stages: HashMap<u64, Stage>,
    /// The stages the build before used: kept one build more, so a
    /// document asked again after another (Isolate on, then off) is found.
    prev: Vec<u64>,
    /// The Curves set's 2D booleans by their own hash and their curves'
    /// (they do not depend on the part, so a drag of a shape does not make
    /// them again).
    regions: HashMap<u64, Result<Vec<Vec<[f32; 3]>>, String>>,
    /// Pairs of features whose islands turned out to touch though their
    /// boxes in the document do not (an extrusion, a mirror image, a copy):
    /// built as one from the start the next times (`build_islands`).
    hints: Vec<(FeatureId, FeatureId)>,
    /// By the key of a stage: how many passes of `join_faces` change it
    /// before nothing is left to merge. An island's keys then do not depend
    /// on how many unions and cuts are made in the others (a shape deleted,
    /// Isolate: the islands that did not change are found again).
    passes: HashMap<u64, usize>,
    /// The stages the last fine builds used (documents at rest: the undo
    /// steps), the newest last: kept, so going back a few steps builds
    /// nothing again.
    rest: std::collections::VecDeque<Vec<u64>>,
    /// The last request's features (id, hash): what a drag changes from one
    /// step to the next (`drag_order`).
    last: Vec<(FeatureId, u64)>,
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

/// What the C layer takes for a profile in a box of `x` × `z`: its points in
/// OpenCascade's XY plane (local (x, z) is (x, −z); the caller turns it so
/// that Z is local Y), whether each is a curve point, and each corner's cut
/// (a fillet, a chamfer, a conic; a curve point with tension its conic, an
/// ellipse's quarter; a spline's spans their cubics).
type OccCut = Option<(CutKind, [f64; 2], [f64; 2], [f64; 2])>;
fn occ_outline(pr: &crate::doc::Profile, x: f64, z: f64) -> (Vec<[f64; 2]>, Vec<bool>, Vec<OccCut>) {
    let flip = |q: [f64; 2]| [q[0], -q[1]];
    let pts: Vec<[f64; 2]> = pr.scaled(x, z).into_iter().map(flip).collect();
    if pr.spline {
        // (an open one: its n − 1 spans, the last point's side straight
        // when closed by the caller)
        let mut cuts: Vec<OccCut> = vec![None; pts.len()];
        for (i, (c1, c2)) in pr.spans(x, z).into_iter().enumerate() {
            cuts[i] = Some((CutKind::Span, flip(c1), flip(c2), [0.0; 2]));
        }
        return (pts.clone(), vec![false; pts.len()], cuts);
    }
    let kind = |c: &crate::doc::Cut| if c.chamfer { CutKind::Chamfer } else if c.rho > 0.0 { CutKind::Conic } else { CutKind::Arc };
    let mut cuts: Vec<OccCut> = pr.cuts(x, z).into_iter().map(|c| c.map(|c| (kind(&c), flip(c.a), flip(c.m), flip(c.b)))).collect();
    for (i, c) in cuts.iter_mut().enumerate() {
        let rho = pr.tension.get(i).copied().unwrap_or(0.0);
        if c.is_none() && pr.smooth.get(i).copied().unwrap_or(false) && rho > 1e-6 {
            *c = Some((CutKind::Curve, [0.0; 2], [rho, 0.0], [0.0; 2]));
        }
    }
    let mut smooth = pr.smooth.clone();
    if pr.open && !smooth.is_empty() {
        // an open path's ends are plain points
        let n = smooth.len();
        smooth[0] = false;
        smooth[n - 1] = false;
        cuts[0] = None;
        if let Some(c) = cuts.get_mut(n - 1) {
            *c = None;
        }
    }
    (pts, smooth, cuts)
}

/// A path's line (world): its outline as a wire, open or closed.
fn path_wire(f: &Feature, pr: &crate::doc::Profile, close: bool) -> Result<Shape, String> {
    let Prim::Path { x, z } = f.prim else { return Err("not a curve".into()) };
    let (pts, smooth, cuts) = occ_outline(pr, x.max(0.01), z.max(0.01));
    let w = Shape::outline_wire(&pts, &smooth, &cuts, pr.open && !close)?;
    let w = w.transformed(rows(DMat4::from_rotation_x(-std::f64::consts::FRAC_PI_2)))?;
    w.transformed(rows(f.matrix()))
}

/// A path's region (world) when it is closed, else its line.
fn section(doc: &Doc, f: &Feature) -> Result<Shape, String> {
    if doc.is_closed_curve(f.id) {
        return region(doc, f, 0);
    }
    let pr = f.profile.as_ref().ok_or("a curve without its points")?;
    if pr.points.len() < 2 {
        return Err("a curve needs 2 points".into());
    }
    path_wire(f, pr, false)
}

/// Each face of a shape made whole from a curve (a revolve, a loft, a fill)
/// tagged by its place: (feature, 0), (feature, 1)…
fn tag_all(mut s: Shape, id: FeatureId) -> Shape {
    let tags: Vec<i64> = (0..s.face_count()).map(|i| FaceTag { feature: id, sub: i.min(60000) as u16 }.pack()).collect();
    s.set_tags(&tags);
    s
}

/// A curve turned round its axis (Revolve): its region (closed) or its line
/// closed along the axis (both ends on it) turned into a solid, else its
/// line turned into a sheet.
fn revolve_tool(doc: &Doc, f: &Feature) -> Result<Shape, String> {
    let rv = f.revolve.ok_or("not a revolve")?;
    let (o, d) = f.revolve_axis().ok_or("a revolve needs a curve")?;
    let m = f.matrix();
    let (wo, wd) = (m.transform_point3(o), f.rot * d);
    let angle = rv.angle.clamp(1.0, 360.0).to_radians();
    let profile = if doc.is_closed_curve(f.id) {
        region(doc, f, 0)?
    } else {
        let pr = f.profile.as_ref().ok_or("a curve without its points")?;
        if pr.points.len() < 2 {
            return Err("a curve needs 2 points".into());
        }
        if f.revolve_closes() {
            // both ends put on the axis exactly, closed along it
            let Prim::Path { x, z } = f.prim else { unreachable!() };
            let mut pr = pr.clone();
            let n = pr.points.len();
            for k in [0, n - 1] {
                if d.x.abs() > 0.5 {
                    pr.points[k][1] = o.z / z.max(0.01);
                } else {
                    pr.points[k][0] = o.x / x.max(0.01);
                }
            }
            let (pts, smooth, cuts) = occ_outline(&pr, x.max(0.01), z.max(0.01));
            let face = Shape::region_cuts(&pts, &smooth, &cuts)?;
            let face = face.transformed(rows(DMat4::from_rotation_x(-std::f64::consts::FRAC_PI_2)))?;
            face.transformed(rows(m))?
        } else {
            path_wire(f, pr, false)?
        }
    };
    Ok(tag_all(profile.revolve(wo.to_array(), wd.to_array(), angle)?, f.id))
}

/// A loft through its curves in order (closed: their regions' outlines,
/// a solid; else their lines, a sheet).
fn loft_tool(doc: &Doc, f: &Feature) -> Result<Shape, String> {
    let l = f.loft.as_ref().ok_or("not a loft")?;
    let secs: Vec<&Feature> = l.sections.iter().filter_map(|id| doc.feature(*id)).collect();
    if secs.len() < 2 {
        return Err("a loft needs 2 curves at least".into());
    }
    let closed = secs.iter().all(|g| doc.is_closed_curve(g.id));
    if !closed && secs.iter().any(|g| doc.is_closed_curve(g.id)) {
        return Err("its curves must be all closed or all open".into());
    }
    let shapes: Vec<Shape> = secs.iter().map(|g| section(doc, g)).collect::<Result<_, _>>()?;
    let refs: Vec<&Shape> = shapes.iter().collect();
    Ok(tag_all(Shape::loft(&refs, closed, l.ruled)?, f.id))
}

/// What a sheet-making feature makes (not the Surface tool's sheets): a
/// curve filled, turned open, a loft of open curves.
fn sheet_tool(doc: &Doc, f: &Feature) -> Result<Shape, String> {
    if f.loft.is_some() {
        return loft_tool(doc, f);
    }
    if f.revolve.is_some() {
        return revolve_tool(doc, f);
    }
    if !doc.is_closed_curve(f.id) {
        return Err("only a closed curve can be filled".into());
    }
    Ok(tag_all(region(doc, f, 0)?, f.id))
}

/// A closed curve's region (a flat face, world), or a 2D boolean's
/// (`Feature::shape_bool`: its operands' regions combined, in order). The
/// operands must lie on one plane.
pub fn region(doc: &Doc, f: &Feature, depth: usize) -> Result<Shape, String> {
    if depth > 16 {
        return Err("a boolean of itself".into());
    }
    if let Some(b) = &f.shape_bool {
        let ops: Vec<&Feature> = b.operands.iter().map(|id| doc.feature(*id).ok_or_else(|| "one of its curves was deleted".to_string())).collect::<Result<_, _>>()?;
        if ops.is_empty() {
            return Err("it has no curves left".into());
        }
        // one plane: the first's
        let (n0, o0) = (ops[0].axis(), ops[0].origin);
        for g in &ops[1..] {
            let n = g.axis();
            if n.cross(n0).length() > 1e-6 || (g.origin - o0).dot(n0).abs() > 1e-4 {
                return Err(format!("{} is not on the plane of {}", g.name, ops[0].name));
            }
        }
        let mut acc = region(doc, ops[0], depth + 1).map_err(|e| format!("{}: {e}", ops[0].name))?;
        for g in &ops[1..] {
            let r = region(doc, g, depth + 1).map_err(|e| format!("{}: {e}", g.name))?;
            acc = match b.op {
                crate::doc::CurveOp::Union => acc.fuse(&r)?,
                crate::doc::CurveOp::Subtract => acc.cut(&r)?,
                crate::doc::CurveOp::Intersect => acc.common(&r)?,
            };
        }
        // its own corners (where its curves cross) rounded or cut
        if !b.corners.is_empty() {
            let corners: Vec<([f64; 3], f64)> = b.corners.iter().map(|c| ((f.origin + f.rot * c.at).to_array(), c.size)).collect();
            if let Ok((s, _)) = acc.fillet2d(&corners, crate::doc::BOOL_CORNER_TOL) {
                acc = s;
            }
        }
        return Ok(acc);
    }
    let (Prim::Path { x, z }, Some(pr)) = (f.prim, f.profile.as_ref()) else { return Err("not a curve".into()) };
    if pr.open || pr.points.len() < 3 {
        return Err("an open curve has no inside".into());
    }
    let (pts, smooth, cuts) = occ_outline(pr, x.max(0.01), z.max(0.01));
    let s = Shape::region_cuts(&pts, &smooth, &cuts)?;
    let s = s.transformed(rows(DMat4::from_rotation_x(-std::f64::consts::FRAC_PI_2)))?;
    s.transformed(rows(f.matrix()))
}

/// A 2D boolean pulled into a solid (step 3 of the Curves set): its region
/// swept along its axis by its height, tagged as a sketch's (1 top, 2
/// bottom, a side 3 + 1000 × its curve's place among the leaves + the
/// piece of that curve nearest it).
fn bool_tool(doc: &Doc, f: &Feature) -> Result<Shape, String> {
    let h = f.prim.height().max(0.01);
    let r = region(doc, f, 0)?;
    let v = f.axis() * h;
    let mut s = r.extrude(v.to_array())?;
    let leaves: Vec<&Feature> = doc.with_operands(&[f.id]).iter().filter_map(|id| doc.feature(*id)).filter(|g| g.shape_bool.is_none() && g.profile.is_some()).collect();
    let tags: Vec<i64> = (0..s.face_count())
        .map(|i| {
            let Some(fi) = s.face_info(i) else { return FaceTag { feature: f.id, sub: 0 }.pack() };
            let n = f.rot.inverse() * DVec3::from_array(fi.normal);
            let sub = if fi.kind == OccSurf::Plane && n.y > 0.5 {
                1
            } else if fi.kind == OccSurf::Plane && n.y < -0.5 {
                2
            } else {
                let at = DVec3::from_array(fi.point);
                let mut best = (f64::MAX, 3u16);
                for (li, g) in leaves.iter().enumerate() {
                    let (Some(pr), Prim::Path { x, z }) = (g.profile.as_ref(), g.prim) else { continue };
                    let q = g.matrix().inverse().transform_point3(at);
                    for pc in pr.pieces(x, z) {
                        let d = pc.pts.windows(2).map(|w| seg_dist([q.x, q.z], w[0], w[1])).fold(f64::MAX, f64::min);
                        if d < best.0 {
                            best = (d, (3 + 1000 * li.min(60) + (pc.id as usize).min(999)) as u16);
                        }
                    }
                }
                best.1
            };
            FaceTag { feature: f.id, sub }.pack()
        })
        .collect();
    s.set_tags(&tags);
    Ok(s)
}

/// A feature's hash with its 2D boolean's curves' (they are built with it).
fn deep_hash(doc: &Doc, f: &Feature) -> u64 {
    if f.shape_bool.is_none() && f.loft.is_none() {
        return f.hash();
    }
    doc.with_operands(&[f.id]).iter().filter_map(|id| doc.feature(*id)).fold(0u64, |h, g| hash2(h, g.hash()))
}

/// The feature's solid in the world, its faces tagged.
fn tool(f: &Feature) -> Result<Shape, String> {
    // a form: its BSpline through its poles (world), one face
    if let Some(fm) = &f.form {
        let poles: Vec<[f64; 3]> = f.form_poles().iter().map(|p| p.to_array()).collect();
        return Ok(tag_all(Shape::form(&poles, fm.nu, fm.nv)?, f.id));
    }
    let h = f.prim.height().max(0.01);
    let s = match f.prim {
        Prim::Box { x, z, .. } => Shape::cuboid(x.max(0.01), h, z.max(0.01))?,
        Prim::Cylinder { radius, .. } => Shape::cylinder(radius.max(0.01), h)?,
        Prim::Profile { x, z, .. } => {
            let pr = f.profile.as_ref().ok_or("a sketch without its outline")?;
            let (pts, smooth, cuts) = occ_outline(pr, x.max(0.01), z.max(0.01));
            let s = if cuts.iter().any(|c| c.is_some()) { Shape::prism_cuts(&pts, &smooth, &cuts, h * 0.5)? } else { Shape::prism(&pts, &smooth, h * 0.5)? };
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

/// How thin a wall, step or gap left between a shape and the part may be
/// before the shape's face is put on the part's (mm; Romain, 20 nov. 5 h 57:
/// "éviter des épaisseurs fines").
const THIN: f64 = 0.3;
/// How far a Carve's face is taken past the part's face it nearly meets.
const PAST: f64 = 0.1;

/// The feature with its flat faces that lie just off a flat face of the
/// part (parallel, within `THIN`, overlapping it) moved: a Carve's face that
/// would leave a thin wall or floor (both faces outward the same way, the
/// Carve's just inside) taken `PAST` beyond; an Add's face that would make
/// a thin step (the same way) put flush, one that floats just above a face
/// it sits on (facing each other) put on it. None when nothing moves. Each
/// face moves at most a quarter of the shape's size across it.
fn settle(acc: &Shape, f: &Feature, t: &Shape) -> Option<Feature> {
    if !f.is_solid() || f.is_path() || f.is_sheet() || f.shape_bool.is_some() || f.form.is_some() {
        return None;
    }
    let rot = f.rot;
    let h = f.prim.height();
    // each tool face's sub → (outward normal in the feature's frame, size
    // across it)
    let (sx, sz) = match f.prim {
        Prim::Box { x, z, .. } | Prim::Profile { x, z, .. } => (x, z),
        Prim::Cylinder { radius, .. } => (radius * 2.0, radius * 2.0),
        Prim::Path { .. } => return None,
    };
    let is_box = matches!(f.prim, Prim::Box { .. });
    let local = |sub: u16| -> Option<(DVec3, f64)> {
        match (is_box, sub) {
            (true, 0) => Some((DVec3::X, sx)),
            (true, 1) => Some((-DVec3::X, sx)),
            (true, 2) | (false, 1) => Some((DVec3::Y, h)),
            (true, 3) | (false, 2) => Some((-DVec3::Y, h)),
            (true, 4) => Some((DVec3::Z, sz)),
            (true, 5) => Some((-DVec3::Z, sz)),
            _ => None,
        }
    };
    let part: Vec<(DVec3, DVec3, ([f64; 3], [f64; 3]))> = (0..acc.face_count())
        .filter_map(|j| {
            let fi = acc.face_info(j)?;
            (fi.kind == OccSurf::Plane).then_some(())?;
            Some((DVec3::from_array(fi.point), DVec3::from_array(fi.normal), acc.face_box(j)?))
        })
        .collect();
    let mut g = f.clone();
    let mut moved = false;
    for i in 0..t.face_count() {
        let Some(fi) = t.face_info(i).filter(|fi| fi.kind == OccSurf::Plane) else { continue };
        let sub = FaceTag::unpack(t.face_tag(i)).map(|x| x.sub).unwrap_or(u16::MAX);
        let Some((ln, size)) = local(sub) else { continue };
        let (pt, nt) = (DVec3::from_array(fi.point), DVec3::from_array(fi.normal));
        let Some(bt) = t.face_box(i) else { continue };
        let tol = THIN.min(size * 0.25);
        let mut best: Option<f64> = None;
        for (pa, na, ba) in &part {
            let s = nt.dot(*na);
            if s.abs() < 1.0 - 1e-9 {
                continue;
            }
            // the boxes meet (grown by the tolerance)
            if (0..3).any(|k| bt.0[k] > ba.1[k] + tol || ba.0[k] > bt.1[k] + tol) {
                continue;
            }
            // where the tool's face lies from the part's, along its normal
            let gap = (pt - *pa).dot(*na);
            if gap.abs() > tol {
                continue;
            }
            // how far the tool face moves along its own outward normal
            let d = match (f.op, s > 0.0) {
                (Op::Carve, true) if gap < PAST - 1e-6 => PAST - gap,
                (Op::Add, true) if gap.abs() > 1e-6 => -gap,
                (Op::Add, false) if gap > 1e-6 => gap,
                _ => continue,
            };
            if best.map(|b| d.abs() < b.abs()).unwrap_or(true) {
                best = Some(d);
            }
        }
        let Some(d) = best.filter(|d| d.abs() <= size * 0.25) else { continue };
        // the size across grows by d, the centre moves half of it that way
        // (along Y the origin is the base: it moves only for the bottom)
        let w = rot * ln;
        match (&mut g.prim, ln) {
            (Prim::Box { x, .. }, v) if v.x.abs() > 0.5 => {
                *x += d;
                g.origin += w * d * 0.5;
            }
            (Prim::Box { z, .. }, v) if v.z.abs() > 0.5 => {
                *z += d;
                g.origin += w * d * 0.5;
            }
            (Prim::Box { y, .. } | Prim::Profile { y, .. } | Prim::Cylinder { height: y, .. }, v) => {
                *y += d;
                if v.y < 0.0 {
                    g.origin += w * d;
                }
            }
            _ => continue,
        }
        moved = true;
    }
    moved.then_some(g)
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
/// (`round_search`). `one`: when it fails, it may try the edges one at a
/// time (`one_by_one`; not after that crashed the kernel).
fn round(acc: &Shape, f: &Feature, r: &Round, search: bool, one: bool) -> (Option<Result<Shape, String>>, usize, Option<f64>) {
    let (edges, missing) = resolve(acc, &r.edges);
    if edges.is_empty() || r.size <= 1e-6 {
        return (None, missing, None);
    }
    let fresh = FaceTag { feature: f.id, sub: 0 }.pack();
    mark(&step("round", f));
    let make = |size: f64| made_on(acc, &edges, r, size, fresh);
    let mut res = make(r.size).map_err(|e| plain_error(&e));
    // on a part that is not a valid solid no order or size will do (21 nov.:
    // the drone's hub dragged, its arms' fillets each searched 12 sizes,
    // 13 s for one step of the drag)
    if res.as_ref().is_err_and(|e| !e.starts_with("its edges are smooth")) && !acc.is_valid() {
        return (Some(Err(NOT_VALID.to_string())), missing, None);
    }
    // two or three of the edges meeting at a corner of four (Romain 20 nov.,
    // a cap standing flush with a pocket's end): OpenCascade rounds one of
    // them, or all four, but not two or three in one go; one at a time in
    // some order it does
    if res.is_err() && one && (2..=4).contains(&edges.len()) {
        // OpenCascade can recurse without end in there (a chamfer at such a
        // corner after a fillet ran into it, 20 nov.): a crash skips only
        // this search the next time
        mark(&step("onebyone", f));
        if let Some(s) = one_by_one(acc, r, fresh) {
            res = Ok(s);
        }
        mark(&step("round", f));
    }
    if res.is_ok() || !search {
        return (Some(res), missing, None);
    }
    match round_search(acc, &edges, r, fresh) {
        Some((s, size)) => (Some(Ok(s)), missing, Some(size)),
        None => (Some(res), missing, None),
    }
}

/// A fillet's error when what it is made on is not a valid solid.
const NOT_VALID: &str = "the part before it is not a valid solid";

/// OpenCascade's words for a fillet it cannot make, put plainly when they say
/// nothing to the user.
fn plain_error(e: &str) -> String {
    if e.contains("no suitable edges") {
        "its edges are smooth (the faces meet tangent): there is nothing to round".to_string()
    } else {
        e.to_string()
    }
}

/// A fillet or chamfer of `edges` on `acc` at `size`: with Only Selected,
/// cut back to them (`only_selected`).
fn made_on(acc: &Shape, edges: &[i32], r: &Round, size: f64, fresh: i64) -> Result<Shape, String> {
    let s = if r.chamfer { acc.chamfer(edges, size, fresh) } else { acc.fillet(edges, size, fresh) }?;
    if r.only {
        only_selected(acc, edges, size, fresh, s)
    } else {
        Ok(s)
    }
}

/// Only Selected (Romain 20 nov., 10 h 54: "forcer à Fillet ou Chamfer que
/// l'edge sélectionné"): OpenCascade carries a fillet or a chamfer along
/// the edges tangent to the ones asked (a pill's top edge: round all of
/// it). `made` is that; within a region round each edge asked (a box from
/// its start to its end, square to it there, `size` × 3 + 1 mm round it;
/// a curved edge: its samples' box less the half-spaces behind its ends)
/// the part is `made`, elsewhere as it was: (made ∩ R) ∪ (acc − R). The
/// fillet then ends square where the edge ends, its end face tagged as the
/// fillet's. When `made`'s new faces all lie within the regions already
/// (nothing carried along) it is kept as it is.
fn only_selected(acc: &Shape, edges: &[i32], size: f64, fresh: i64, made: Shape) -> Result<Shape, String> {
    let w0 = size * 3.0 + 1.0;
    // the borders of the faces made (along the whole tangent chain)
    let is_fresh: Vec<bool> = (0..made.face_count()).map(|i| made.face_tag(i) == fresh).collect();
    let mut fresh_pts: Vec<DVec3> = vec![];
    for (k, f) in made.edge_faces().iter().enumerate() {
        if f.iter().any(|&i| i >= 0 && is_fresh.get(i as usize).copied().unwrap_or(false)) {
            if let Some(e) = made.edge(k as i32 + 1, 16) {
                fresh_pts.extend(e.pts.iter().map(|p| DVec3::from_array(*p)));
            }
        }
    }
    let mut boxes: Vec<(DVec3, DVec3)> = vec![];
    let mut region: Option<Shape> = None;
    let rows = |o: DVec3, x: DVec3, y: DVec3, z: DVec3| [[x.x, y.x, z.x, o.x], [x.y, y.y, z.y, o.y], [x.z, y.z, z.z, o.z]];
    // a box of sizes (l, 2a, 2a) from o along x, centred across (a cuboid
    // is made centred on the origin)
    let slab = |o: DVec3, x: DVec3, l: f64, a: f64| -> Result<Shape, String> {
        let y = x.any_orthonormal_vector();
        let z = x.cross(y);
        Shape::cuboid(l, 2.0 * a, 2.0 * a)?.transformed(rows(o + x * (l * 0.5), x, y, z))
    };
    for &e in edges {
        let Some(ed) = acc.edge(e, 64) else { return Ok(made) };
        let pts: Vec<DVec3> = ed.pts.iter().map(|p| DVec3::from_array(*p)).collect();
        let (a, b) = (pts[0], pts[pts.len() - 1]);
        if pts.len() < 2 || a.distance(b) < 1e-6 {
            // a closed edge: nothing to carry it along to
            return Ok(made);
        }
        let t = (b - a).normalize();
        // as far round it as its own fillet reaches (its borders seen beside
        // the edge, not past its ends), a little more: the fillet carried
        // along the other side of a narrow pocket stays out (Romain 20 nov.,
        // 10 h 47: a chamfer on a pocket's rim cut its other rim too)
        let w = setback(&pts, &fresh_pts).map(|s| (s * 1.3 + 0.2).min(w0)).unwrap_or(w0);
        if std::env::var("FORME_ONLY_DEBUG").is_ok() {
            eprintln!("ONLY edge {e} {:?} -> {:?}: setback {:?} w {w} fresh pts {} faces {}", a, b, setback(&pts, &fresh_pts), fresh_pts.len(), is_fresh.iter().filter(|x| **x).count());
        }
        let straight = pts.iter().all(|p| (*p - a - t * (*p - a).dot(t)).length() < 1e-4);
        let piece = if straight {
            slab(a, t, a.distance(b), w)?
        } else {
            let lo = pts.iter().fold(DVec3::splat(f64::MAX), |m, p| m.min(*p)) - DVec3::splat(w);
            let hi = pts.iter().fold(DVec3::splat(f64::MIN), |m, p| m.max(*p)) + DVec3::splat(w);
            let ta = (pts[1] - pts[0]).normalize();
            let tb = (pts[pts.len() - 1] - pts[pts.len() - 2]).normalize();
            let big = (hi - lo).length() * 2.0 + 10.0;
            let mut bx = Shape::cuboid(hi.x - lo.x, hi.y - lo.y, hi.z - lo.z)?.transformed(rows((lo + hi) * 0.5, DVec3::X, DVec3::Y, DVec3::Z))?;
            // the half-spaces behind its start and past its end, unless the
            // edge itself would go into them (an arc of more than half a turn)
            let inside = pts[1..pts.len() - 1].iter().all(|p| (*p - a).dot(ta) > -1e-6 && (*p - b).dot(tb) < 1e-6);
            if inside {
                bx = bx.cut(&slab(a - ta * big, ta, big, big)?)?;
                bx = bx.cut(&slab(b, tb, big, big)?)?;
            }
            bx
        };
        boxes.push(piece_box(&pts, w));
        region = Some(match region {
            None => piece,
            Some(r) => r.fuse(&piece)?,
        });
    }
    let Some(mut region) = region else { return Ok(made) };
    // nothing carried along: the fillet as it is
    let carried = (0..made.face_count()).filter(|&i| made.face_tag(i) == fresh).any(|i| {
        made.face_box(i).map_or(true, |(lo, hi)| {
            let (lo, hi) = (DVec3::from_array(lo), DVec3::from_array(hi));
            !boxes.iter().any(|(a, b)| lo.cmpge(*a - DVec3::splat(1e-3)).all() && hi.cmple(*b + DVec3::splat(1e-3)).all())
        })
    });
    if !carried {
        return Ok(made);
    }
    let n = region.face_count();
    region.set_tags(&vec![fresh; n.max(1)]);
    let inner = made.common(&region)?;
    let outer = acc.cut(&region)?;
    outer.fuse(&inner)
}

/// How far from an edge (a polyline) the nearest of `pts` lie beside it
/// (seen square to it, not past its ends): a fillet's contact lines.
fn setback(line: &[DVec3], pts: &[DVec3]) -> Option<f64> {
    let n = line.len();
    if n < 2 {
        return None;
    }
    // past its ends: behind the plane square to it at either end, where
    // its fillet's end sections lie (with a few degrees to spare: the
    // samples' chords lean from the curve's tangent)
    let (a, b) = (line[0], line[n - 1]);
    let ta = (line[1] - a).normalize_or_zero();
    let tb = (b - line[n - 2]).normalize_or_zero();
    let mut best: Option<f64> = None;
    for p in pts {
        if (*p - a).dot(ta) < 1e-3 + 0.06 * p.distance(a) || (*p - b).dot(tb) > -1e-3 - 0.06 * p.distance(b) {
            continue;
        }
        let mut near = (f64::MAX, false);
        for (i, w) in line.windows(2).enumerate() {
            let d = w[1] - w[0];
            let l2 = d.length_squared();
            if l2 < 1e-18 {
                continue;
            }
            let u = ((*p - w[0]).dot(d) / l2).clamp(0.0, 1.0);
            let dist = p.distance(w[0] + d * u);
            if dist < near.0 {
                let past = (i == 0 && u <= 1e-6) || (i + 2 == n && u >= 1.0 - 1e-6);
                near = (dist, past);
            }
        }
        if !near.1 && near.0 > 1e-4 && near.0 < best.unwrap_or(f64::MAX) {
            best = Some(near.0);
        }
    }
    best
}

/// The box round a run of points, `w` more each way.
fn piece_box(pts: &[DVec3], w: f64) -> (DVec3, DVec3) {
    let lo = pts.iter().fold(DVec3::splat(f64::MAX), |m, p| m.min(*p)) - DVec3::splat(w);
    let hi = pts.iter().fold(DVec3::splat(f64::MIN), |m, p| m.max(*p)) + DVec3::splat(w);
    (lo, hi)
}

/// The largest size that works for a fillet too big, by halves: at most 12
/// tries (a count, so the same document gives the same size whatever the
/// load), and a last stop after 6 s (next to a face curved tighter than the
/// fillet, OpenCascade can take many seconds for each try near the limit).
fn round_search(acc: &Shape, edges: &[i32], r: &Round, fresh: i64) -> Option<(Shape, f64)> {
    let make = |size: f64| made_on(acc, edges, r, size, fresh);
    let (mut lo, mut hi) = (0.0, r.size);
    let t0 = std::time::Instant::now();
    for _ in 0..12 {
        if t0.elapsed().as_secs_f64() > SEARCH_STOP {
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
            return Some((s, size));
        }
    }
    None
}

/// A fillet or chamfer of a few edges made one edge at a time, in the first
/// order that makes them all at the size asked (depth first, so the orders
/// share their first steps; at most 40 tries, a count so that the answer
/// does not depend on the load, and a last stop after 6 s). An edge already
/// rounded by one before it (OpenCascade carries a fillet along tangent
/// edges) counts as made. All the faces made carry the step's tag.
fn one_by_one(acc: &Shape, r: &Round, fresh: i64) -> Option<Shape> {
    fn go(s: &Shape, left: &[EdgeRef], r: &Round, fresh: i64, t0: Instant, tries: &mut u32) -> Option<Shape> {
        for k in 0..left.len() {
            if *tries >= 40 || t0.elapsed().as_secs_f64() > SEARCH_STOP {
                return None;
            }
            let rest: Vec<EdgeRef> = left.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, e)| e.clone()).collect();
            let (e, missing) = resolve(s, std::slice::from_ref(&left[k]));
            let next = if missing > 0 || e.is_empty() {
                continue;
            } else {
                *tries += 1;
                made_on(s, &e, r, r.size, fresh)
            };
            if let Ok(n) = next {
                // the edges left that this one's fillet took along with it
                let rest: Vec<EdgeRef> = rest.into_iter().filter(|x| resolve(&n, std::slice::from_ref(x)).1 == 0).collect();
                if rest.is_empty() {
                    return Some(n);
                }
                if let Some(done) = go(&n, &rest, r, fresh, t0, tries) {
                    return Some(done);
                }
            }
        }
        None
    }
    go(acc, &r.edges, r, fresh, Instant::now(), &mut 0)
}

/// The last stop of the fillet searches (`round_search`, `one_by_one`): they
/// end on their count of tries first, this only keeps a pathological case
/// from holding the kernel. (Ten times longer in the tests: run in
/// parallel on a loaded PC, three of them passed the 6 s and failed.)
const SEARCH_STOP: f64 = if cfg!(test) { 60.0 } else { 6.0 };

/// A fillet that blends its corners: rebuilt in one operation with the run
/// of fillets just before it (from the first one it touches, through shared
/// edge ends), on the solid from before that run. None when there is
/// nothing to blend with or OpenCascade cannot make it (then each fillet is
/// made on its own, as before).
fn blend(befores: &[(&Feature, Stage)], f: &Feature, r: &Round) -> Option<(Shape, Vec<(u32, f64)>)> {
    let n = befores.len().checked_sub(1)?;
    let mut k = n;
    while k > 0 && befores[k - 1].0.round.as_ref().map(|g| !g.chamfer && !g.only).unwrap_or(false) && !gap(befores[k - 1].0, befores[k].0, true) {
        k -= 1;
    }
    if k == n {
        return None;
    }
    // (an island: a fillet of another island in the same run that is not
    // found before the run stops every blend of the run in one history)
    if let Some(p) = place_of(f) {
        if strangers_lost(run_start(p), p) {
            return None;
        }
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
    if let (Some(a), Some(p)) = (place_of(befores[k + first].0), place_of(f)) {
        if strangers_lost(a, p) {
            return None;
        }
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
/// size does): `limits` gets (fillet, the tension it was made at). `before`:
/// the solid the fillets were made on; their faces that were there already
/// (another copy's of an Array, same tags) are left as they are.
fn tension(s: Rc<Shape>, before: Option<&Shape>, ts: &[(u32, f64)], name_of: &dyn Fn(u32) -> String, errors: &mut Vec<String>, limits: &mut Vec<(u32, f64)>) -> (Rc<Shape>, Vec<Option<bool>>) {
    if ts.is_empty() {
        return (s, vec![]);
    }
    let tagged: Vec<(i64, f64)> = ts.iter().map(|&(id, t)| (FaceTag { feature: id, sub: 0 }.pack(), t)).collect();
    match s.fillet_tension_since(before, &tagged) {
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
                if let Ok((t2, c2)) = s.fillet_tension_since(before, &again) {
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
                let (n, k) = tension(s, before, std::slice::from_ref(t), name_of, errors, limits);
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
/// A move next to fillets or chamfers made earlier (20 nov.: Romain's
/// flank, a Knife after its top fillets): the faces of those round round
/// what moves taken away (the faces round them grown back sharp,
/// BRepAlgoAPI_Defeaturing), the move made, then each made again on the
/// edges between the faces it ran along, near where it was.
fn tweak_unrounded(doc: &Doc, acc: &Shape, f: &Feature, t: &Tweak) -> Result<Shape, String> {
    let mut vs: Vec<i32> = vec![];
    for c in t.comps() {
        for v in comp_vertices(acc, c)? {
            if !vs.contains(&v) {
                vs.push(v);
            }
        }
    }
    let round_of = |i: usize| FaceTag::unpack(acc.face_tag(i)).and_then(|tg| doc.feature(tg.feature)).filter(|g| g.round.is_some() && g.id != f.id);
    // the round steps with a face at a moved vertex, in the document's order
    let mut rids: Vec<FeatureId> = vec![];
    for i in 0..acc.face_count() {
        if let Some(g) = round_of(i) {
            if !rids.contains(&g.id) && acc.face_vertices(i).iter().any(|v| vs.contains(v)) {
                rids.push(g.id);
            }
        }
    }
    if rids.is_empty() {
        return Err("no fillet round it".into());
    }
    rids.sort_by_key(|id| doc.features.iter().position(|g| g.id == *id));
    let faces: Vec<usize> = (0..acc.face_count()).filter(|&i| round_of(i).is_some_and(|g| rids.contains(&g.id))).collect();
    mark(&step("unround", f));
    let mut base = acc.defeature(&faces)?;
    // the Knife's straight lines between two pieces of one face that ran
    // to those faces: the pieces were made one again; cut again, the line
    // carried on to the face's new border
    let tags0: Vec<i64> = (0..acc.face_count()).map(|i| acc.face_tag(i)).collect();
    let span = vs.iter().map(|&v| DVec3::from_array(acc.vertex_points()[(v - 1) as usize])).fold(0.0f64, |m, p| m.max(p.length())) * 4.0 + 1000.0;
    for (i, fs) in acc.edge_faces().iter().enumerate() {
        if fs[0] < 0 || fs[1] < 0 || fs[0] == fs[1] || tags0[fs[0] as usize] != tags0[fs[1] as usize] {
            continue;
        }
        let Some(e) = acc.edge(i as i32 + 1, 9) else { continue };
        if e.kind != forme_occt::EdgeKind::Line || e.pts.len() < 2 {
            continue;
        }
        let (a, b) = (DVec3::from_array(e.pts[0]), DVec3::from_array(*e.pts.last().unwrap()));
        let mid = DVec3::from_array(e.pts[4]);
        let t = tags0[fs[0] as usize];
        let btags: Vec<i64> = (0..base.face_count()).map(|i| base.face_tag(i)).collect();
        let kept = base.edge_faces().iter().enumerate().any(|(j, g)| {
            g[0] >= 0 && g[1] >= 0 && btags[g[0] as usize] == t && btags[g[1] as usize] == t && base.edge(j as i32 + 1, 9).is_some_and(|x| x.pts.iter().any(|p| DVec3::from_array(*p).distance(mid) < 1e-3))
        });
        if kept || a.distance(b) < 1e-6 {
            continue;
        }
        let d = (b - a).normalize();
        if let Ok(s) = base.split(&[(a - d * span).to_array(), (b + d * span).to_array()], &[[0, 1]]) {
            base = s;
        }
    }
    let mut s = match tweak(&base, f, t)? {
        Some(s) => s,
        None => base,
    };
    let reach = t.delta.length() + 0.5;
    for rid in rids {
        let g = doc.feature(rid).unwrap();
        let rd = g.round.as_ref().unwrap();
        let old: Vec<usize> = faces.iter().copied().filter(|&i| round_of(i).is_some_and(|x| x.id == rid)).collect();
        let keys: Vec<[i64; 2]> = rd.edges.iter().map(|e| e.key()).collect();
        let tags: Vec<i64> = (0..s.face_count()).map(|i| s.face_tag(i)).collect();
        let mut edges = vec![];
        for (i, fs) in s.edge_faces().iter().enumerate() {
            if fs[0] < 0 || fs[1] < 0 {
                continue;
            }
            let (a, b) = (tags[fs[0] as usize], tags[fs[1] as usize]);
            let k = if a <= b { [a, b] } else { [b, a] };
            if !keys.contains(&k) {
                continue;
            }
            let Some(mid) = s.edge(i as i32 + 1, 9).and_then(|e| e.pts.get(4).copied()) else { continue };
            if old.iter().any(|&o| acc.face_distance(o, mid).is_some_and(|d| d <= rd.size + reach)) {
                edges.push(EdgeRef { faces: k, at: DVec3::from_array(mid) });
            }
        }
        let again = Round { edges, ..rd.clone() };
        match round(&s, g, &again, true, false) {
            (Some(Ok(r)), _, _) => s = r,
            _ => return Err(format!("{} could not be made again", g.name)),
        }
    }
    Ok(s)
}

/// Duplicate faces (`Tweak.copy`): the picked faces copied as a piece,
/// scaled round their points' centre and moved, then fused with the part
/// (beside it as a sheet when they close no volume).
fn duplicate(acc: &Shape, f: &Feature, t: &Tweak) -> Result<Option<Shape>, String> {
    let mut faces = vec![];
    for c in t.comps() {
        if let Comp::Face(r) = c {
            let i = find_face(acc, r).ok_or("its face is gone")?;
            if !faces.contains(&i) {
                faces.push(i);
            }
        }
    }
    if faces.is_empty() {
        return Err("no face to copy".into());
    }
    let c = t.about.unwrap_or_else(|| t.at());
    let m = DMat4::from_translation(c + t.delta) * DMat4::from_scale(t.scale) * DMat4::from_translation(-c);
    mark(&step("copy", f));
    let piece = acc.piece(&faces, rows(m), FaceTag { feature: f.id, sub: 0 }.pack())?;
    // in place (not moved yet) or a sheet: beside the part, no boolean
    // (two coincident solids make a union fail); a union that fails also
    // leaves the copy beside the part rather than nothing
    let moved = t.delta.length() > 1e-6 || (t.scale - DVec3::ONE).abs().max_element() > 1e-9;
    if moved && piece.volume().abs() > 1e-9 {
        if let Ok(s) = acc.fuse(&piece) {
            return Ok(Some(s));
        }
    }
    acc.compound(&piece).map(Some)
}

fn tweak(acc: &Shape, f: &Feature, t: &Tweak) -> Result<Option<Shape>, String> {
    if t.copy {
        return duplicate(acc, f, t);
    }
    // a part with sheets beside its solids (a copy of open faces): the move
    // made on the side its components are on, the other kept as it is
    if let (Ok(sheets), Ok(solids)) = (acc.lumps_of(false), acc.lumps_of(true)) {
        let on = |s: &Shape| t.comps().all(|c| comp_vertices(s, c).map(|v| !v.is_empty()).unwrap_or(false));
        for (side, other) in [(&solids, &sheets), (&sheets, &solids)] {
            if on(side) {
                return match tweak(side, f, t)? {
                    Some(r) => r.compound(other).map(Some),
                    None => Ok(None),
                };
            }
        }
    }
    let scaled = (t.scale - DVec3::ONE).abs().max_element() > 1e-9;
    if t.delta.length() < 1e-6 && !scaled {
        return Ok(None);
    }
    let mut vs: Vec<i32> = vec![];
    for c in t.comps() {
        for v in comp_vertices(acc, c)? {
            if !vs.contains(&v) {
                vs.push(v);
            }
        }
    }
    if vs.is_empty() {
        return Err("nothing to move".into());
    }
    let pts = acc.vertex_points();
    let at = |v: i32| DVec3::from_array(pts[(v - 1) as usize]);
    let centre = t.about.unwrap_or_else(|| vs.iter().map(|&v| at(v)).sum::<DVec3>() / vs.len() as f64);
    let moves: Vec<(i32, [f64; 3])> = vs.iter().map(|&v| (v, (centre + (at(v) - centre) * t.scale + t.delta).to_array())).collect();
    mark(&step("tweak", f));
    // a whole piece moved (a copy's faces all picked, a lump beside the
    // part): carried as it is, whatever its faces
    let m = DMat4::from_translation(centre + t.delta) * DMat4::from_scale(t.scale) * DMat4::from_translation(-centre);
    if let Ok(s) = acc.move_lumps(&vs, rows(m)) {
        return Ok(Some(s));
    }
    let err = match acc.move_vertices(&moves) {
        Ok(s) => return Ok(Some(s)),
        Err(e) => e,
    };
    // a flat face moved along its normal: the faces round it, curved ones
    // and those with holes too, made longer or shorter to meet it
    if let (Comp::Face(r), true, false) = (&t.comp, t.more.is_empty(), scaled) {
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

/// The last way to move a flat face, when the faces round it cannot
/// follow it (the ring of an inset left as a line, a slanted face, a
/// fillet) and making the move before the fillets did not work either:
/// the face swept into new walls, as Maya's Move of an extruded face
/// (Romain 23 nov., 7 h 05: "Pourquoi c'est si compliqué de faire que les
/// components se déplacent?"). Off its normal, the walls slant.
fn tweak_swept(acc: &Shape, f: &Feature, t: &Tweak) -> Option<Shape> {
    let (Comp::Face(r), true, true, false, false) = (&t.comp, t.sweep, t.more.is_empty(), t.copy, (t.scale - DVec3::ONE).abs().max_element() > 1e-9) else { return None };
    let face = find_face(acc, r)?;
    let info = acc.face_info(face)?;
    if info.kind != OccSurf::Plane {
        return None;
    }
    let n = DVec3::from_array(info.normal).normalize_or_zero();
    sweep_face(acc, f, r, face, n, DVec3::from_array(info.point), t.delta).ok()
}

/// Two flat faces of the part joined by a solid (Bridge, Maya's): their
/// outlines lofted, straight from one to the other, fused with the part.
/// When the loft cannot pair their outlines and the faces face each other
/// on parallel planes, each swept across the gap to the other's plane and
/// the two sweeps' common part taken. Its faces tagged `(step, 1…)`.
fn bridge(acc: &Shape, f: &Feature, b: &crate::doc::Bridge) -> Result<Shape, String> {
    let fa = find_face(acc, &b.faces[0]).ok_or("its first face is gone")?;
    let fb = find_face(acc, &b.faces[1]).ok_or("its second face is gone")?;
    if fa == fb {
        return Err("it needs two different faces".into());
    }
    let (ia, ib) = (acc.face_info(fa).ok_or("its first face is gone")?, acc.face_info(fb).ok_or("its second face is gone")?);
    if ia.kind != OccSurf::Plane || ib.kind != OccSurf::Plane {
        return Err("only flat faces can be bridged for now".into());
    }
    mark(&step("bridge", f));
    let (a, c) = (acc.inset_face(fa, 0.0)?, acc.inset_face(fb, 0.0)?);
    let (na, nb) = (DVec3::from_array(ia.normal).normalize_or_zero(), DVec3::from_array(ib.normal).normalize_or_zero());
    let (pa, pb) = (DVec3::from_array(ia.point), DVec3::from_array(ib.point));
    let loft = Shape::loft(&[&a, &c], true, true);
    let mut tool = match loft {
        Ok(t) => t,
        Err(e) => {
            // two faces facing each other across a gap
            let gap = (pb - pa).dot(na);
            if na.dot(nb) > -0.999 || gap <= 1e-3 {
                return Err(e);
            }
            let sa = a.extrude((na * gap).to_array())?;
            let sb = c.extrude((nb * gap).to_array())?;
            sa.common(&sb)?
        }
    };
    let tags: Vec<i64> = (0..tool.face_count()).map(|i| FaceTag { feature: f.id, sub: (i + 1).min(u16::MAX as usize) as u16 }.pack()).collect();
    tool.set_tags(&tags);
    acc.fuse(&tool)
}

/// A flat face swept by `delta` (with a part along its outward normal `n`)
/// into a prism, fused when it goes out, cut when it goes in: the face's
/// end keeps the face's tag (it is the face moved), the walls are the
/// step's `(id, 2…)`, the base `(id, 0)`.
fn sweep_face(acc: &Shape, f: &Feature, r: &FaceRef, face: usize, n: DVec3, o: DVec3, delta: DVec3) -> Result<Shape, String> {
    let d = delta.dot(n);
    if d.abs() < 1e-3 {
        return Err("the move runs along the face".into());
    }
    let tag = |sub: u16| FaceTag { feature: f.id, sub }.pack();
    let inner = acc.inset_face(face, 0.0)?;
    // carving: from a little outside the face, so no face of the tool lies on it
    let start = if d < 0.0 { 0.1 } else { 0.0 };
    let base = if start > 0.0 { inner.transformed(rows(DMat4::from_translation(n * start)))? } else { inner };
    let mut prism = base.extrude((delta - n * start).to_array())?;
    let mut side = 2u16;
    let tags: Vec<i64> = (0..prism.face_count())
        .map(|i| {
            let fi = prism.face_info(i);
            let across = fi.map(|fi| fi.kind == OccSurf::Plane && DVec3::from_array(fi.normal).dot(n).abs() > 0.999).unwrap_or(false);
            let s = fi.map(|fi| (DVec3::from_array(fi.point) - o).dot(n)).unwrap_or(0.0);
            if across && (s - d).abs() < 1e-4 {
                r.tag
            } else if across {
                tag(0)
            } else {
                side = side.saturating_add(1);
                tag(side - 1)
            }
        })
        .collect();
    prism.set_tags(&tags);
    if d > 0.0 {
        acc.fuse(&prism)
    } else {
        acc.cut(&prism)
    }
}

/// A move and its mirror image (after a Mirror) made in one go, each
/// vertex by its own move (a face across the plane whose two corners move
/// bends once, not twice: a bent face cannot follow a second move).
fn tweak_pair(acc: &Shape, f: &Feature, t: &Tweak, u: &Tweak) -> Result<Option<Shape>, String> {
    if t.copy || u.copy {
        return Err("copies one at a time".into());
    }
    let pts = acc.vertex_points();
    let at = |v: i32| DVec3::from_array(pts[(v - 1) as usize]);
    let mut moves: Vec<(i32, [f64; 3])> = vec![];
    for tw in [t, u] {
        let mut vs: Vec<i32> = vec![];
        for c in tw.comps() {
            for v in comp_vertices(acc, c)? {
                if !vs.contains(&v) {
                    vs.push(v);
                }
            }
        }
        if vs.is_empty() {
            return Err("nothing to move".into());
        }
        let centre = tw.about.unwrap_or_else(|| vs.iter().map(|&v| at(v)).sum::<DVec3>() / vs.len() as f64);
        for &v in &vs {
            if !moves.iter().any(|(w, _)| *w == v) {
                moves.push((v, (centre + (at(v) - centre) * tw.scale + tw.delta).to_array()));
            }
        }
    }
    mark(&step("tweak", f));
    acc.move_vertices(&moves).map(Some)
}

/// The Knife: its points found on the edges (the fraction from the end
/// nearer where each was put), cut into them with its lines. None when it
/// has no point.
fn knife(acc: &Shape, f: &Feature, k: &Knife) -> Result<Option<Shape>, String> {
    if !k.remove_faces.is_empty() {
        let mut faces = vec![];
        for r in &k.remove_faces {
            faces.push(find_face(acc, r).ok_or("its faces are gone")?);
        }
        mark(&step("cut", f));
        return acc.remove_faces(&faces).map(Some);
    }
    if !k.remove_edges.is_empty() {
        let (found, _) = resolve(acc, &k.remove_edges);
        if found.is_empty() {
            return Err("its edges are gone".into());
        }
        mark(&step("cut", f));
        return acc.remove_edges(&found).map(Some);
    }
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

// ----------------------------------------------------------------- shell

/// The part hollowed (`doc::Shell`): its faces found again (by tag and
/// nearest point), taken away as openings, the walls made as thick as
/// asked, the faces grown sharp to meet (else filled round, which is what
/// works next to a fillet beside an opening). Too thick, the thickest that
/// works by halves (`limit`; at most 12 tries or 6 s). Also the count of
/// faces not found.
fn shell(acc: &Shape, f: &Feature, sh: &Shell, limit: &mut Option<f64>) -> (Result<Option<Shape>, String>, usize) {
    if sh.thickness < 1e-3 {
        return (Ok(None), 0);
    }
    let mut faces = vec![];
    let mut missing = 0;
    for r in &sh.faces {
        match find_face(acc, r) {
            Some(i) if !faces.contains(&i) => faces.push(i),
            Some(_) => {}
            None => missing += 1,
        }
    }
    if !sh.faces.is_empty() && faces.is_empty() {
        return (Err("its faces are gone".into()), missing);
    }
    let base = FaceTag { feature: f.id, sub: 0 }.pack();
    mark(&step("shell", f));
    let make = |t: f64| acc.shell(&faces, t, sh.outward, base, false).or_else(|e| acc.shell(&faces, t, sh.outward, base, true).map_err(|_| e));
    let e = match make(sh.thickness) {
        Ok(s) => return (Ok(Some(s)), missing),
        Err(e) => e,
    };
    // too thick? (a thin one must work first, else it is not the thickness)
    if make((sh.thickness * 0.05).min(0.1)).is_err() {
        return (Err(e), missing);
    }
    let (mut lo, mut hi) = ((sh.thickness * 0.05).min(0.1), sh.thickness);
    let t0 = Instant::now();
    for _ in 0..12 {
        if t0.elapsed().as_secs_f64() > SEARCH_STOP {
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
    match (size >= 0.01).then(|| make(size)) {
        Some(Ok(s)) => {
            *limit = Some(size);
            (Ok(Some(s)), missing)
        }
        _ => (Err(e), missing),
    }
}

// ---------------------------------------------------------------- mirror

/// The box round a shape (its faces' boxes).
fn shape_box(s: &Shape) -> Option<(DVec3, DVec3)> {
    let mut b: Option<(DVec3, DVec3)> = None;
    for i in 0..s.face_count() {
        let Some((lo, hi)) = s.face_box(i) else { continue };
        let (lo, hi) = (DVec3::from_array(lo), DVec3::from_array(hi));
        b = Some(match b {
            Some((a, c)) => (a.min(lo), c.max(hi)),
            None => (lo, hi),
        });
    }
    b
}

/// What is built so far made symmetric across the Mirror's plane: its
/// solids cut at the plane (the side kept), mirrored and joined to it (the
/// faces of one surface across the plane made one by the union); the
/// sheets beside them mirrored whole. Faces keep their tags on both sides,
/// so a click on either side names the same shape.
fn mirror_part(acc: &Shape, id: FeatureId, m: &Mirror, island: bool) -> Result<Option<Shape>, String> {
    let sheets = acc.lumps_of(false).ok();
    let solids = match &sheets {
        Some(_) => acc.lumps_of(true).ok(),
        None => None,
    };
    let solids: Option<&Shape> = if sheets.is_some() { solids.as_ref() } else { Some(acc) };
    let mut out: Option<Shape> = None;
    // its lumps' boxes first (quick, never smaller than the lumps): all of
    // it clearly on one side of the plane needs no exact box, which costs
    // tens of ms on a part with curved faces
    let (mut near, mut far) = (f64::MAX, f64::MIN);
    for (lo, hi) in solids.map(|s| s.lump_boxes()).unwrap_or_default() {
        let (k0, k1) = (m.side(DVec3::from_array(lo)), m.side(DVec3::from_array(hi)));
        near = near.min(k0.min(k1));
        far = far.max(k0.max(k1));
    }
    // (off the plane on the kept side: nothing to cut, and its image does
    // not touch it)
    let apart = near > 1e-3 && near.is_finite() && std::env::var_os("FORME_MIRROR_FUSE").is_none();
    // (an island all on the side dropped: its solids are gone, as the cut
    // at the plane takes them when the part has more on the side kept)
    let dropped = |sol: &Shape| {
        if apart || (far < -1e-3 && far.is_finite()) {
            return !apart;
        }
        let Some((lo, hi)) = shape_box(sol) else { return false };
        let (k0, k1) = (m.side(lo), m.side(hi));
        k0.max(k1) <= 1e-6 * (hi - lo).max_element().max(1.0)
    };
    if std::env::var_os("FORME_ISLANDS_DEBUG").is_some() {
        eprintln!("MIRROR {id}: acc {} faces, solids {:?} box {:?} dropped {:?}, sheets {:?}", acc.face_count(), solids.map(|s| s.face_count()), solids.and_then(shape_box), solids.map(|s| dropped(s)), sheets.as_ref().map(|s| s.face_count()));
    }
    let solids = solids.filter(|s| !(island && dropped(s)));
    if solids.is_none() && sheets.is_none() && island {
        return Ok(None);
    }
    // the part and its image side by side, no union: its faces of one
    // surface made one as the union does (a union of two solids that do not
    // touch took 250 ms for a pill with 15 operations, at each step of its
    // drag: Romain's scene of 21 nov.)
    if let Some(half) = solids.filter(|_| apart).and_then(|sol| sol.simplified().ok()) {
        out = Some(half.compound(&half.transformed(m.rows())?)?);
    }
    if let Some(sol) = solids.filter(|_| out.is_none()) {
        let (lo, hi) = shape_box(sol).ok_or("nothing to mirror")?;
        let n = m.normal();
        let a = m.axis.min(2) as usize;
        let (k0, k1) = (m.side(lo), m.side(hi));
        let (klo, khi) = (k0.min(k1), k0.max(k1));
        let size = (hi - lo).max_element().max(1.0);
        let tol = 1e-6 * size;
        if khi <= tol {
            return Err(format!("nothing on the kept side of the plane (flip it: keep the {}{} side)", if m.flip { "+" } else { "−" }, m.axis_name()));
        }
        let cut;
        let half: &Shape = if klo >= -tol {
            sol
        } else {
            // a box over the kept side, from the plane out past the part
            let margin = 1.0 + 0.1 * size;
            let len = khi + margin;
            let mut dims = (hi - lo) + DVec3::splat(2.0 * margin);
            dims[a] = len;
            let mut c = (lo + hi) * 0.5;
            c[a] = m.offset + if m.flip { -0.5 * len } else { 0.5 * len };
            let mut b = Shape::cuboid(dims.x, dims.y, dims.z)?;
            b.set_tags(&vec![FaceTag { feature: id, sub: 0 }.pack(); b.face_count()]);
            let b = b.transformed(rows(DMat4::from_translation(c)))?;
            let _ = n;
            let t = Instant::now();
            cut = sol.common(&b)?;
            if std::env::var_os("FORME_STAGE_MS").is_some() {
                eprintln!("  mirror {id}: half {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
            }
            &cut
        };
        let image = half.transformed(m.rows())?;
        // (its clean-up checked: a hole's wall across the plane, its two
        // halves one face again, made an invalid solid, 21 nov.)
        let t = Instant::now();
        out = Some(half.fuse_checked(&image)?);
        if std::env::var_os("FORME_STAGE_MS").is_some() {
            eprintln!("  mirror {id}: union {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
        }
    }
    if let Some(sh) = &sheets {
        let both = sh.compound(&sh.transformed(m.rows())?)?;
        out = Some(match out {
            Some(o) => o.compound(&both)?,
            None => both,
        });
    }
    out.map(Some).ok_or_else(|| "nothing to mirror".to_string())
}

/// A shape made after a Mirror: the tool and its image as one (an Add on
/// both sides, a Carve through both), the tool alone when it is its own
/// image (a box or a cylinder centred on the plane): None.
fn twin_tool(t: &Shape, m: &Mirror) -> Result<Option<Shape>, String> {
    let image = t.transformed(m.rows())?;
    if let (Some((lo, hi)), Some((lo2, hi2))) = (shape_box(t), shape_box(&image)) {
        let tol = 1e-4 * (hi - lo).length() + 1e-5;
        if lo.distance(lo2) < tol && hi.distance(hi2) < tol {
            let v = t.volume();
            if let Ok(c) = t.common(&image) {
                if v > 0.0 && (c.volume() - v).abs() < 1e-4 * v {
                    return Ok(None);
                }
            }
        }
    }
    t.fuse(&image).map(Some)
}

/// The document with every step after a Mirror made on both sides: a
/// fillet's or chamfer's edges with their images (one step: both sides
/// blend and stop alike); an inset, a move, the Knife followed by its image
/// (the same id: its faces carry the same tags; `twin_of`). None when there
/// is no Mirror. (The shapes after it are doubled in `run`, `twin_tool`.)
/// The document with the steps after a Mirror set to "this side only"
/// (`Mirror.single`), up to the next Mirror, marked so (`one_side`), as if
/// set one by one. (Not past another Mirror: a step made on the side that
/// Mirror drops would be lost.) None when no Mirror says so.
fn sided_doc(doc: &Doc) -> Option<Doc> {
    if !doc.features.iter().any(|f| f.visible && !f.suspended && f.mirror.is_some_and(|m| m.single)) {
        return None;
    }
    let mut out = doc.clone();
    let mut single = false;
    for f in out.features.iter_mut() {
        match f.mirror {
            Some(m) if f.visible && !f.suspended => single = m.single,
            Some(_) => {}
            None if single => f.one_side = true,
            None => {}
        }
    }
    Some(out)
}

fn mirrored_doc(doc: &Doc) -> Option<Doc> {
    if !doc.features.iter().any(|f| f.mirror.is_some() && f.visible && !f.suspended) {
        return None;
    }
    let mut out = doc.clone();
    out.features.clear();
    let mut sym: Option<Mirror> = None;
    for f in &doc.features {
        let on = f.visible && !f.suspended;
        if let (Some(m), true) = (&f.mirror, on) {
            sym = Some(*m);
            out.features.push(f.clone());
            continue;
        }
        let Some(m) = sym.filter(|_| on && !f.one_side) else {
            out.features.push(f.clone());
            continue;
        };
        if let Some(r) = &f.round {
            let mut g = f.clone();
            let images: Vec<EdgeRef> = r.edges.iter().filter(|e| m.reflect(e.at).distance(e.at) > 1e-6).map(|e| EdgeRef { at: m.reflect(e.at), ..e.clone() }).collect();
            if let Some(gr) = g.round.as_mut() {
                gr.edges.extend(images);
            }
            out.features.push(g);
        } else if let Some(sh) = &f.shell {
            // the openings' images open too (one step: a part hollowed once)
            let mut g = f.clone();
            let images: Vec<FaceRef> = sh.faces.iter().filter(|r| m.reflect(r.at).distance(r.at) > 1e-6).map(|r| FaceRef { at: m.reflect(r.at), ..*r }).collect();
            if let Some(gs) = g.shell.as_mut() {
                gs.faces.extend(images);
            }
            out.features.push(g);
        } else if f.push.is_some() || f.tweak.is_some() || f.cut.is_some() || f.bridge.is_some() {
            out.features.push(f.clone());
            let refl = |p: DVec3| m.reflect(p);
            let mut g = f.carried_by(&refl);
            if g != *f {
                g.name = format!("{}{MIRROR_SIDE}", f.name);
                out.features.push(g);
            }
        } else {
            out.features.push(f.clone());
        }
    }
    Some(out)
}

/// The image of the step before it (`mirrored_doc`): the same id (an
/// Array's copies have it too, but not the name).
fn twin_of<'a>(feats: &[&'a Feature], i: usize) -> Option<&'a Feature> {
    (i > 0 && feats[i - 1].id == feats[i].id && feats[i].name.ends_with(MIRROR_SIDE)).then(|| feats[i - 1])
}

const MIRROR_SIDE: &str = " (mirror side)";

/// The document with each Pattern / Array's shapes and operations made
/// again at each copy's place, just after it (`Doc::array_pattern`), copy
/// after copy: the same ids, so a copy's faces carry the original's tags
/// (a click on a copy names the original's faces, and a fillet's edges are
/// found again on each copy near its own points). None when there is no
/// Array. (Before `mirrored_doc`: the copies after a Mirror are made on
/// both sides.)
fn arrayed_doc(doc: &Doc) -> Option<Doc> {
    if !doc.features.iter().any(|f| f.array.is_some() && f.visible && !f.suspended) {
        return None;
    }
    let mut out = doc.clone();
    out.features.clear();
    for (i, f) in doc.features.iter().enumerate() {
        out.features.push(f.clone());
        let Some(a) = f.array.as_ref().filter(|_| f.visible && !f.suspended) else { continue };
        let pat = doc.array_pattern(i);
        for (n, (_, m)) in a.copies().into_iter().enumerate() {
            for p in &pat {
                let mut g = if p.is_solid() { p.moved_by(m) } else { p.carried_by(&|q| m.transform_point3(q)) };
                g.name = format!("{} (copy {})", p.name, n + 2);
                out.features.push(g);
            }
        }
    }
    Some(out)
}

/// An image step that names what its step named on the solid before them
/// (a face or a knife line across the plane): nothing more to make.
fn twin_same(acc0: Option<&Shape>, a: &Feature, b: &Feature) -> bool {
    if let (Some(p), Some(q)) = (&a.push, &b.push) {
        let Some(acc) = acc0 else { return true };
        return find_face(acc, &p.face) == find_face(acc, &q.face);
    }
    if let (Some(t), Some(u)) = (&a.tweak, &b.tweak) {
        let Some(acc) = acc0 else { return true };
        let verts = |t: &Tweak| {
            let mut v: Vec<i32> = t.comps().filter_map(|c| comp_vertices(acc, c).ok()).flatten().collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        // the same components (across the plane): a move of them is not
        // made twice, even one that leaves the part lopsided. (Components
        // found on neither side, a fillet made before having taken them,
        // are not "the same": the move made before that fillet takes its
        // image with it, else the image side stayed as it was, in silence.)
        let (x, y) = (verts(t), verts(u));
        return !x.is_empty() && x == y;
    }
    if let (Some(p), Some(q)) = (&a.bridge, &b.bridge) {
        // two faces each other's images (a leg and its mirror): one bridge
        let Some(acc) = acc0 else { return true };
        let mut x: Vec<Option<usize>> = p.faces.iter().map(|r| find_face(acc, r)).collect();
        let mut y: Vec<Option<usize>> = q.faces.iter().map(|r| find_face(acc, r)).collect();
        x.sort();
        y.sort();
        return x == y;
    }
    if let (Some(k), Some(l)) = (&a.cut, &b.cut) {
        let near = |p: DVec3, set: &[DVec3]| set.iter().any(|q| q.distance(p) < 1e-6);
        let (kp, kf, ke): (Vec<DVec3>, Vec<DVec3>, Vec<DVec3>) = (k.pts.iter().map(|q| q.at).collect(), k.remove_faces.iter().map(|q| q.at).collect(), k.remove_edges.iter().map(|q| q.at).collect());
        return l.pts.iter().all(|p| near(p.at, &kp)) && l.remove_faces.iter().all(|r| near(r.at, &kf)) && l.remove_edges.iter().all(|e| near(e.at, &ke));
    }
    false
}

fn hash2(a: u64, b: u64) -> u64 {
    crate::doc::fnv(&[a.to_le_bytes(), b.to_le_bytes()].concat())
}

/// Builds the document. Never fails: what cannot be made is listed in
/// `Model::errors` and left out.
/// Builds the features `feats` in this order: the stage after the last
/// (the stages on the way cached by the hash of what comes before).
fn run<'a>(doc: &'a Doc, feats: &[&'a Feature], skip: &[String], cache: &mut Cache, model: &mut Model, used_tools: &mut Vec<u64>, used_stages: &mut Vec<u64>) -> Stage {
    let mut stage = Stage::new();
    let island = in_island();
    let mut key = 0u64;
    // the stage before each feature (blended fillets start again from there)
    let mut befores: Vec<(&Feature, Stage)> = vec![];
    // the Mirror the steps from here on are made on both sides of
    let mut sym: Option<Mirror> = None;
    // (an island) the place in the whole history of the step before
    let mut last: Option<usize> = None;
    // a union or a cut made in another island since: this island's faces
    // of one surface made one too, as in one history (a stage of its own)
    // Its key says how many passes changed it, not how many were asked: one
    // that changes nothing leaves the key as it is, so the steps after it
    // are found again whatever is made, deleted or left out elsewhere.
    let join = |times: usize, key: &mut u64, stage: &mut Stage, cache: &mut Cache, used_stages: &mut Vec<u64>| {
        let pre = *key;
        let name = |n: usize| if n == 0 { pre } else { hash2(hash2(pre, 0x6a6f696e), n as u64) };
        if let Some(&full) = cache.passes.get(&pre) {
            let n = times.min(full);
            if n == 0 {
                stage.rough = false;
                return;
            }
            if let Some(s) = cache.stages.get(&name(n)) {
                *stage = s.clone();
                *key = name(n);
                used_stages.push(*key);
                return;
            }
        }
        let n = stage.join_faces(times);
        if !stage.rough {
            cache.passes.insert(pre, n);
        }
        if n > 0 {
            *key = name(n);
            used_stages.push(*key);
            keep(cache, *key, stage);
        }
    };
    for (i, &f) in feats.iter().enumerate() {
        let here = if island { place_of(f) } else { None };
        if let (Some(a), Some(b)) = (last, here) {
            let n = strangers_join(a, Some(b));
            if n > 0 {
                join(n, &mut key, &mut stage, cache, used_stages);
            }
        }
        last = here.or(last);
        befores.push((f, stage.clone()));
        let fh = hash_of(doc, f);
        key = hash2(key, fh);
        // (a move made in one go with its image, the step after it: not the
        // stage the move alone gives, which an island built up to it keeps)
        if f.tweak.is_some() && feats.get(i + 1).is_some_and(|g| g.id == f.id) {
            key = hash2(key, 0x70616972);
        }
        // (an island's Mirror may leave nothing of it: not the same step as
        // in one history, where that is an error)
        if island && f.mirror.is_some() {
            key = hash2(key, 0x69736c616e64);
        }
        let skipped = |what: &str| skip.iter().any(|s| *s == step(what, f));
        if skipped("tool") || skipped("boolean") || skipped("round") || skipped("push") || skipped("tweak") || skipped("cut") || skipped("mirror") || skipped("shell") {
            key = hash2(key, 1);
        }
        if let Some(m) = &f.mirror {
            sym = Some(*m);
        }
        used_stages.push(key);
        if let Some(s) = cache.stages.get(&key) {
            stage = s.clone();
            model.cached += 1;
            continue;
        }
        // the part made symmetric
        if let Some(m) = &f.mirror {
            if let Some(acc) = stage.shape.clone() {
                // the same plane as the Mirror before it and nothing gone
                // wrong since (every step after a Mirror is made on both
                // sides): the part is symmetric already (Romain's scene of
                // 21 nov.: three Mirrors across X, 0.9 s each)
                let plane = (m.axis, m.offset.to_bits());
                if stage.even.is_some_and(|e| (e.0, e.1) == plane && e.2 == stage.errors.len()) && !skipped("mirror") && std::env::var_os("FORME_MIRROR_AGAIN").is_none() {
                    stage.mirrors.push((f.id, true));
                    keep(cache, key, &mut stage);
                    continue;
                }
                let res = if skipped("mirror") {
                    Err("skipped, it crashed the kernel".to_string())
                } else {
                    mark(&step("mirror", f));
                    mirror_part(&acc, f.id, m, island)
                };
                match res {
                    // (nothing came out of the cut at the plane: gone too)
                    Ok(Some(s)) if island && s.face_count() == 0 => {
                        stage.shape = None;
                        stage.mirrors.push((f.id, false));
                        stage.even = None;
                    }
                    Ok(Some(s)) => {
                        model.booleans += 1;
                        stage.shape = Some(Rc::new(s));
                        stage.rough = true;
                        stage.mirrors.push((f.id, true));
                        stage.even = Some((plane.0, plane.1, stage.errors.len()));
                    }
                    // an island all on the side the Mirror drops
                    Ok(None) => {
                        stage.shape = None;
                        stage.mirrors.push((f.id, false));
                        stage.even = None;
                    }
                    Err(e) => {
                        stage.errors.push(format!("{}: {e}", f.name));
                        stage.even = None;
                    }
                }
            }
            keep(cache, key, &mut stage);
            continue;
        }
        // a step made on one side only: the part is no longer symmetric
        // (the same Mirror again makes it so)
        if f.one_side && sym.is_some() {
            stage.even = None;
        }
        // the image of the step before (after a Mirror) that names what it
        // named (a face across the plane): nothing more to make
        if let Some(a) = twin_of(feats, i) {
            // (a move that failed: its image would fail the same way, one
            // error says it)
            let failed = a.tweak.is_some() && befores[i].1.errors.len() > befores[i - 1].1.errors.len();
            if stage.paired || failed || twin_same(befores[i - 1].1.shape.as_deref(), a, f) {
                // a move of components across the plane, made once: it may
                // leave the part lopsided (the same Mirror again would even
                // it out)
                if !stage.paired && a.tweak.is_some() {
                    stage.even = None;
                }
                stage.paired = false;
                keep(cache, key, &mut stage);
                continue;
            }
        }
        stage.paired = false;
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
                    let (t, took) = tension(Rc::new(p), Some(acc.as_ref()), &all, &name_of, &mut errs, &mut stops);
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
                if r.blend && !r.chamfer && !r.only && !skipped("round") {
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
                    let (mut res, missing, mut limit) = if skipped("round") { (Some(Err("skipped, it crashed the kernel".to_string())), 0, None) } else { round(&acc, f, r, false, !skip.contains(&step("onebyone", f))) };
                    // (with a blended result at hand, a smaller size made on
                    // its own would not be taken: no search for it)
                    if matches!(&res, Some(Err(e)) if e != NOT_VALID) && partial.is_none() && !skipped("round") {
                        // next to fillets made just before it: made before
                        // them, they made again on what it gives
                        if let Some(s) = replay_round(doc, &feats[..i], f, r, &stage, skip, cache, model, used_tools, used_stages) {
                            stage = s;
                            keep(cache, key, &mut stage);
                            continue;
                        }
                        // too big? the largest size that works
                        let (edges, _) = resolve(&acc, &r.edges);
                        if let Some((s, size)) = round_search(&acc, &edges, r, FaceTag { feature: f.id, sub: 0 }.pack()) {
                            res = Some(Ok(s));
                            limit = Some(size);
                        }
                    }
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
                        (Some(Err(e)), None) if e.starts_with("its edges are smooth") => stage.errors.push(format!("{}: {e}", f.name)),
                        (Some(Err(e)), None) => {
                            // at a corner of four edges or more: say so, and
                            // what OpenCascade can do there
                            let (edges, _) = resolve(&acc, &r.edges);
                            let msg = match crowded(&acc, &edges) {
                                Some((all, k)) => format!("{}: OpenCascade cannot round {k} of the {n} edges meeting at one corner here: try one of them, or all {n}", f.name, n = all.len()),
                                None => format!("{}: OpenCascade cannot make it at {} mm ({e})", f.name, (r.size * 1000.0).round() / 1000.0),
                            };
                            stage.errors.push(msg)
                        }
                        (None, None) => {}
                    }
                }
                if let Some((s, full)) = made {
                    model.booleans += 1;
                    stage.shape = Some(s);
                    stage.rough = true;
                    stage.conic.retain(|id| *id != f.id && !full.contains(id));
                    stage.conic.extend(full);
                }
            }
            keep(cache, key, &mut stage);
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
                        stage.rough = true;
                    }
                    Ok(None) => {}
                    Err(e) => stage.errors.push(format!("{}: {e}", f.name)),
                }
            }
            keep(cache, key, &mut stage);
            continue;
        }
        // a face, edge or vertex moved: on the solid built so far
        if let Some(t) = &f.tweak {
            if let Some(acc) = stage.shape.clone() {
                // with its mirror image next (after a Mirror): both moved
                // in one go when they do not name the same components
                let image = feats.get(i + 1).copied().filter(|g| g.id == f.id && twin_of(feats, i).is_none() && !skipped("tweak") && g.tweak.is_some() && !twin_same(Some(&acc), f, g));
                if let Some((g, u)) = image.and_then(|g| Some((g, g.tweak.as_ref()?))) {
                    if let Ok(Some(s)) = tweak_pair(&acc, f, t, u) {
                        model.booleans += 1;
                        stage.shape = Some(Rc::new(s));
                        stage.rough = true;
                        stage.paired = true;
                        keep(cache, key, &mut stage);
                        continue;
                    }
                    // both where fillets made just before run along them:
                    // made before those, in one go there (the image alone,
                    // after its move, would find no fillet just before it)
                    if !t.copy {
                        if let Some(mut s) = replay(doc, &feats[..i], f, Some(g), t, &stage, skip, cache, model, used_tools, used_stages) {
                            s.paired = true;
                            stage = s;
                            keep(cache, key, &mut stage);
                            continue;
                        }
                    }
                }
                let res = if skipped("tweak") { Err("skipped, it crashed the kernel".to_string()) } else { tweak(&acc, f, t) };
                // (the image of a move that was made where it stood: both
                // made again before the fillets)
                let twin = twin_of(feats, i).filter(|a| a.tweak.is_some() && i > 0);
                match res {
                    Ok(Some(s)) => {
                        model.booleans += 1;
                        stage.shape = Some(Rc::new(s));
                        stage.rough = true;
                    }
                    Ok(None) => {}
                    Err(e) if t.copy => stage.errors.push(format!("{}: {e}", f.name)),
                    Err(e) => match match twin.and_then(|a| Some((a, a.tweak.as_ref()?))) {
                        Some((a, at)) => replay(doc, &feats[..i - 1], a, Some(f), at, &stage, skip, cache, model, used_tools, used_stages),
                        None => replay(doc, &feats[..i], f, None, t, &stage, skip, cache, model, used_tools, used_stages),
                    } {
                        Some(s) => stage = s,
                        None => match tweak_unrounded(doc, &acc, f, t).ok().or_else(|| tweak_swept(&acc, f, t)) {
                            Some(s) => {
                                model.booleans += 1;
                                stage.shape = Some(Rc::new(s));
                                stage.rough = true;
                            }
                            None => stage.errors.push(format!("{}: {e}", f.name)),
                        },
                    },
                }
            }
            keep(cache, key, &mut stage);
            continue;
        }
        // the part hollowed: on the solid built so far
        if let Some(sh) = &f.shell {
            if let Some(acc) = stage.shape.clone() {
                let mut limit = None;
                let (res, missing) = if skipped("shell") { (Err("skipped, it crashed the kernel".to_string()), 0) } else { shell(&acc, f, sh, &mut limit) };
                if let Some(l) = limit {
                    stage.limits.push((f.id, l));
                }
                if missing > 0 {
                    stage.errors.push(format!("{}: {missing} open face{} not found", f.name, if missing > 1 { "s" } else { "" }));
                }
                match res {
                    Ok(Some(s)) => {
                        model.booleans += 1;
                        stage.shape = Some(Rc::new(s));
                        stage.rough = true;
                    }
                    Ok(None) => {}
                    Err(e) => stage.errors.push(format!("{}: {e}", f.name)),
                }
            }
            keep(cache, key, &mut stage);
            continue;
        }
        // two faces bridged: on the solid built so far
        if let Some(b) = &f.bridge {
            if let Some(acc) = stage.shape.clone() {
                let res = if skipped("bridge") { Err("skipped, it crashed the kernel".to_string()) } else { bridge(&acc, f, b) };
                match res {
                    Ok(s) => {
                        model.booleans += 1;
                        stage.shape = Some(Rc::new(s));
                        stage.rough = true;
                    }
                    Err(e) => stage.errors.push(format!("{}: {e}", f.name)),
                }
            }
            keep(cache, key, &mut stage);
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
                        stage.rough = true;
                    }
                    Ok(None) => {}
                    Err(e) => stage.errors.push(format!("{}: {e}", f.name)),
                }
            }
            keep(cache, key, &mut stage);
            continue;
        }
        // the feature's solid
        let t = if skipped("tool") {
            Err("skipped, it crashed the kernel".to_string())
        } else if let Some(t) = cache.tools.get(&fh) {
            Ok(t.clone())
        } else {
            mark(&step("tool", f));
            if f.revolve.is_some() || f.loft.is_some() { sheet_tool(doc, f) } else if f.shape_bool.is_some() { bool_tool(doc, f) } else { tool(f) }.map(|t| {
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
                keep(cache, key, &mut stage);
                continue;
            }
        };
        // a face of it lying just off a face of the part (a thin wall, a
        // thin step, a gap): moved onto it, or for a Carve past it
        let (t, th) = match &stage.shape {
            Some(acc) if !skipped("boolean") => match settle(acc, f, &t).and_then(|g| tool(&g).ok().map(|s| (Rc::new(s), g.hash()))) {
                Some(x) => x,
                None => (t, fh),
            },
            _ => (t, fh),
        };
        // after a Mirror: the shape and its image
        let t = match sym {
            Some(m) if !skipped("tool") && !f.one_side => {
                let th = hash2(hash2(th, 0x6d6972726f72), hash2(m.axis as u64 + 3 * m.flip as u64, m.offset.to_bits()));
                match cache.tools.get(&th) {
                    Some(d) => {
                        used_tools.push(th);
                        d.clone()
                    }
                    None => match twin_tool(&t, &m) {
                        Ok(d) => {
                            let d = d.map(Rc::new).unwrap_or_else(|| t.clone());
                            cache.tools.insert(th, d.clone());
                            used_tools.push(th);
                            d
                        }
                        Err(e) => {
                            stage.errors.push(format!("{}: its mirror image failed ({e})", f.name));
                            t
                        }
                    },
                }
            }
            _ => t,
        };
        // the boolean
        let next = match (&stage.shape, f.op) {
            (None, Op::Add) => Some(Ok(t)),
            (None, Op::Carve) => None,
            (Some(_), _) if skipped("boolean") => Some(Err("skipped, it crashed the kernel".to_string())),
            (Some(acc), op) => {
                mark(&step("boolean", f));
                model.booleans += 1;
                // (where the tool reaches: a Carve's box is not in the result's)
                boxes_add(&mut stage.ext, &t.lump_boxes());
                Some(if op == Op::Add { acc.fuse(&t) } else { acc.cut(&t) }.map(Rc::new))
            }
        };
        let first = stage.shape.is_none();
        match next {
            Some(Ok(s)) => {
                stage.shape = Some(s);
                // an island's first shape: in one history it is joined to
                // what is built before it, its faces of one surface made one
                // (a union or a cut of its own unifies once: one more pass,
                // another island's step, may still join faces)
                stage.rough = true;
                if first && here.is_some_and(joins_here) {
                    stage.join_faces(1);
                }
            }
            Some(Err(e)) => stage.errors.push(format!("{}: {} failed ({e})", f.name, if f.op == Op::Add { "union" } else { "subtraction" })),
            None => {}
        }
        keep(cache, key, &mut stage);
    }
    if let Some(a) = last {
        let n = strangers_join(a, None);
        if n > 0 {
            join(n, &mut key, &mut stage, cache, used_stages);
        }
    }
    stage
}

/// A face or edge moved (`Tweak`) that cannot be made where fillets or
/// chamfers made just before it run along it: made before them instead,
/// they made again on what it gives (as if it had come first; their edges
/// are found again by their faces' tags). The stage, when that adds no
/// error. `image`: its mirror image (the step after it, after a Mirror),
/// made there with it.
#[allow(clippy::too_many_arguments)]
fn replay<'a>(doc: &'a Doc, before: &[&'a Feature], f: &'a Feature, image: Option<&'a Feature>, t: &Tweak, stage: &Stage, skip: &[String], cache: &mut Cache, model: &mut Model, used_tools: &mut Vec<u64>, used_stages: &mut Vec<u64>) -> Option<Stage> {
    let mut tags: Vec<i64> = vec![];
    for c in t.comps() {
        match c {
            Comp::Face(r) => tags.push(r.tag),
            Comp::Edge(e) => tags.extend(e.faces),
            Comp::Vertex(_) => return None,
        }
    }
    let mut j = before.len();
    while j > 0 && before[j - 1].round.is_some() && !gap(before[j - 1], before.get(j).copied().unwrap_or(f), false) {
        j -= 1;
    }
    let touches = before[j..].iter().any(|g| g.round.as_ref().is_some_and(|r| r.edges.iter().any(|e| e.faces.iter().any(|x| tags.contains(x)))));
    if !touches {
        return None;
    }
    let order: Vec<&Feature> = before[..j].iter().copied().chain(std::iter::once(f)).chain(image).chain(before[j..].iter().copied()).collect();
    let s = run(doc, &order, skip, cache, model, used_tools, used_stages);
    (s.shape.is_some() && s.errors.len() <= stage.errors.len()).then_some(s)
}

thread_local! {
    /// A round step being made again before the fillets it touches
    /// (`replay_round`): no other is moved meanwhile.
    static REPLAYING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A fillet or chamfer that cannot be made next to the fillets made just
/// before it (Romain 20 nov.: the four edges round a cap standing flush with
/// a pocket's end, after the cap's top edge was rounded): made before that
/// run of round steps, which are then made again on what it gives (as
/// `replay` does for a move; their edges found again by their faces' tags).
/// Not before a step whose faces it names. The stage, when it brings no
/// error and no step made smaller than before.
#[allow(clippy::too_many_arguments)]
fn replay_round<'a>(doc: &'a Doc, before: &[&'a Feature], f: &'a Feature, r: &Round, stage: &Stage, skip: &[String], cache: &mut Cache, model: &mut Model, used_tools: &mut Vec<u64>, used_stages: &mut Vec<u64>) -> Option<Stage> {
    if REPLAYING.with(|c| c.get()) {
        return None;
    }
    let mut j = before.len();
    while j > 0 && before[j - 1].round.is_some() && !gap(before[j - 1], before.get(j).copied().unwrap_or(f), false) {
        j -= 1;
    }
    let names = |g: &Feature| r.edges.iter().any(|e| e.faces.iter().any(|t| FaceTag::unpack(*t).is_some_and(|x| x.feature == g.id)));
    if let Some(k) = before[j..].iter().rposition(|g| names(g)) {
        j += k + 1;
    }
    let tags: Vec<i64> = r.edges.iter().flat_map(|e| e.faces).collect();
    let touches = before[j..].iter().any(|g| g.round.as_ref().is_some_and(|gr| gr.edges.iter().any(|e| e.faces.iter().any(|x| tags.contains(x)))));
    if !touches {
        return None;
    }
    let order: Vec<&Feature> = before[..j].iter().copied().chain(std::iter::once(f)).chain(before[j..].iter().copied()).collect();
    REPLAYING.with(|c| c.set(true));
    let s = run(doc, &order, skip, cache, model, used_tools, used_stages);
    REPLAYING.with(|c| c.set(false));
    let kept = s.shape.is_some() && s.errors.len() <= stage.errors.len() && s.limits.iter().all(|l| l.0 == f.id || stage.limits.contains(l));
    kept.then_some(s)
}

/// A corner where four edges or more meet, some of a fillet's edges among
/// them but not all (OpenCascade rounds one of them, or all together, but
/// most often not two or three in one go): the edges there, and how many
/// of the fillet's.
fn crowded(acc: &Shape, edges: &[i32]) -> Option<(Vec<i32>, usize)> {
    let adj = acc.edge_faces();
    let ends: Vec<(i32, [DVec3; 2])> = (1..=adj.len() as i32)
        .filter(|&i| {
            let f = adj[i as usize - 1];
            f[0] >= 0 && f[1] >= 0 && f[0] != f[1]
        })
        .filter_map(|i| {
            let e = acc.edge(i, 2)?;
            let (a, b) = (DVec3::from_array(*e.pts.first()?), DVec3::from_array(*e.pts.last()?));
            (a.distance(b) > 1e-6).then_some((i, [a, b]))
        })
        .collect();
    let mut best: Option<(Vec<i32>, usize)> = None;
    for (_, pe) in ends.iter().filter(|(i, _)| edges.contains(i)) {
        for p in pe {
            let at: Vec<i32> = ends.iter().filter(|(_, q)| q.iter().any(|q| q.distance(*p) < 1e-4)).map(|(i, _)| *i).collect();
            let k = at.iter().filter(|i| edges.contains(i)).count();
            // the corner with the most of its edges (a fillet's other end
            // may meet such a corner with one edge only, which works)
            if at.len() >= 4 && k >= 2 && k < at.len() && best.as_ref().map_or(true, |b| (k, at.len()) > (b.1, b.0.len())) {
                best = Some((at, k));
            }
        }
    }
    best
}

// --------------------------------------------------------------- islands
//
// Romain, 21 nov. 2026: "la scène ouverte commence à être lourde et quand on
// déplace un cube c'est en 1 fps". His scene: 90 steps in one history, eight
// groups of shapes that do not touch; moving one made every step after it
// again (5.5 s a step of the drag). The history is now built island by
// island: the shapes that touch, with the operations on their faces, each
// island a history of its own (its stages cached by what comes before in
// it), the results side by side in one compound. A change is made again in
// its island only, and the meshing keeps the other islands' triangles.
//
// What is in an island: the shapes whose boxes come within `APART`; an
// operation with the features whose faces it names; copies and mirror
// images (the same id); a move of vertices with the island that has the
// vertex. A Mirror is a step of every island. The islands are then checked
// by where their solids and tools really went (`Stage.ext`): two that come
// near each other are joined and built again (and remembered,
// `Cache.hints`), so the result is that of one history.

/// How near two islands may come before they are built as one (mm): more
/// than `THIN` + `PAST` (a shape's face put on a face of the part near it)
/// and than the 0.1 mm a carving starts outside its face.
const APART: f64 = 1.0;
/// How many documents at rest keep their stages (`Cache.rest`).
const REST: usize = 8;

/// The whole history, for the islands being built.
struct Whole {
    /// Each feature's place in the whole list, by its address.
    place: HashMap<usize, usize>,
    /// Per place: 1 a fillet that blends with the run before it (no
    /// chamfer, not Only selected), 2 another fillet or chamfer, 0 the rest.
    kinds: Vec<u8>,
    /// Per place of an operation: the places of the features it names
    /// (None: one that is not there).
    names: Vec<Vec<Option<usize>>>,
    /// Per place: a step that makes a union or a cut on the part (a shape
    /// after the first, an extrusion, a copy fused, a fillet on its edges
    /// only): in one history it joins every island's faces of one surface.
    joins: Vec<bool>,
}

/// What `run` needs to know of the whole history while it builds one island.
struct Island {
    whole: Rc<Whole>,
    /// Per place: one of this island's (the Mirrors are every island's).
    own: Vec<bool>,
    /// The places built (the whole list, or its start).
    upto: usize,
}

thread_local! {
    static ISLAND: std::cell::RefCell<Option<Island>> = const { std::cell::RefCell::new(None) };
    /// The features' hashes during one build, by address (an island's list
    /// is run several times).
    static HASHES: std::cell::RefCell<HashMap<usize, u64>> = std::cell::RefCell::new(HashMap::new());
    /// One history, as before the islands (the tests compare both).
    static ONE_CHAIN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn addr(f: &Feature) -> usize {
    f as *const Feature as usize
}

fn hash_of(doc: &Doc, f: &Feature) -> u64 {
    // (kept by address for the whole history's features only, which live
    // as long as the build: a replay's own features come and go, and the
    // next may take the address of one gone)
    if place_of(f).is_none() {
        return deep_hash(doc, f);
    }
    if let Some(h) = HASHES.with(|m| m.borrow().get(&addr(f)).copied()) {
        return h;
    }
    let h = deep_hash(doc, f);
    HASHES.with(|m| m.borrow_mut().insert(addr(f), h));
    h
}

fn in_island() -> bool {
    ISLAND.with(|c| c.borrow().is_some())
}

/// A feature's place in the whole history (an island being built).
fn place_of(f: &Feature) -> Option<usize> {
    ISLAND.with(|c| c.borrow().as_ref().and_then(|il| il.whole.place.get(&addr(f)).copied()))
}

/// Between two neighbours of an island's list, a step of another island
/// that ends a run of fillets in the whole history (`blend`: anything but
/// a fillet that blends; else anything but a fillet or chamfer).
fn gap(a: &Feature, b: &Feature, blend: bool) -> bool {
    ISLAND.with(|c| {
        let c = c.borrow();
        let Some(il) = c.as_ref() else { return false };
        let (Some(&pa), Some(&pb)) = (il.whole.place.get(&addr(a)), il.whole.place.get(&addr(b))) else { return false };
        (pa.min(pb) + 1..pa.max(pb)).any(|p| !il.own[p] && (il.whole.kinds[p] == 0 || (blend && il.whole.kinds[p] != 1)))
    })
}

/// Where the run of blending fillets that ends just before place `p`
/// starts in the whole history.
fn run_start(p: usize) -> usize {
    ISLAND.with(|c| {
        let c = c.borrow();
        let Some(il) = c.as_ref() else { return p };
        let mut w = p;
        while w > 0 && il.whole.kinds[w - 1] == 1 {
            w -= 1;
        }
        w
    })
}

/// A fillet of another island among the places `from..to` that names a
/// feature made in that span or one that is not there: it cannot be found
/// on the solid from before `from`.
fn strangers_lost(from: usize, to: usize) -> bool {
    ISLAND.with(|c| {
        let c = c.borrow();
        let Some(il) = c.as_ref() else { return false };
        (from..to).any(|p| !il.own[p] && il.whole.kinds[p] == 1 && il.whole.names[p].iter().any(|n| n.map_or(true, |q| q >= from)))
    })
}

/// How many unions and cuts are made in other islands between two places
/// of this one's list (`b` None: after `a`, up to the end of what is
/// built): each one unifies the whole part once more in one history, and a
/// second pass may join faces the first left.
fn strangers_join(a: usize, b: Option<usize>) -> usize {
    ISLAND.with(|c| {
        let c = c.borrow();
        let Some(il) = c.as_ref() else { return 0 };
        let b = b.unwrap_or(il.upto);
        (a.min(b) + 1..a.max(b)).filter(|&p| !il.own[p] && il.whole.joins[p]).count()
    })
}

/// This place's step makes a union or a cut in one history (a shape with
/// one added before it).
fn joins_here(p: usize) -> bool {
    ISLAND.with(|c| c.borrow().as_ref().is_some_and(|il| il.whole.joins[p]))
}

/// A box added to a list of boxes (those inside another are left out).
fn boxes_add(ext: &mut Vec<[f64; 6]>, new: &[([f64; 3], [f64; 3])]) {
    let inside = |a: &[f64; 6], b: &[f64; 6]| (0..3).all(|i| a[i] >= b[i] && a[i + 3] <= b[i + 3]);
    for (lo, hi) in new {
        let b = [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]];
        if b.iter().any(|x| !x.is_finite()) || ext.iter().any(|e| inside(&b, e)) {
            continue;
        }
        ext.retain(|e| !inside(e, &b));
        ext.push(b);
    }
}

/// Two lists of boxes that come within `d` of each other.
fn boxes_near(a: &[[f64; 6]], b: &[[f64; 6]], d: f64) -> bool {
    a.iter().any(|p| b.iter().any(|q| (0..3).all(|i| p[i] - d <= q[i + 3] && q[i] - d <= p[i + 3])))
}

/// Disjoint sets of places.
struct Sets(Vec<usize>);

impl Sets {
    fn find(&mut self, mut a: usize) -> usize {
        while self.0[a] != a {
            self.0[a] = self.0[self.0[a]];
            a = self.0[a];
        }
        a
    }
    /// True when they were apart.
    fn join(&mut self, a: usize, b: usize) -> bool {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b)] = a.min(b);
        }
        a != b
    }
}

/// The islands of the first `upto` features, each built (`run`): its
/// places and its stage. Islands that turn out to come near each other are
/// joined and built again, until none does. None when a Mirror keeps
/// nothing of any island (one history then: its error says to flip it).
#[allow(clippy::too_many_arguments)]
fn islands_upto<'a>(doc: &'a Doc, feats: &[&'a Feature], upto: usize, global: &[bool], sets: &mut Sets, whole: &Rc<Whole>, skip: &[String], cache: &mut Cache, model: &mut Model, used_tools: &mut Vec<u64>, used_stages: &mut Vec<u64>) -> Option<Vec<(Vec<usize>, Stage)>> {
    loop {
        let mut groups: Vec<Vec<usize>> = vec![];
        let mut at: HashMap<usize, usize> = HashMap::new();
        for i in (0..upto).filter(|&i| !global[i]) {
            let r = sets.find(i);
            let g = *at.entry(r).or_insert_with(|| {
                groups.push(vec![]);
                groups.len() - 1
            });
            groups[g].push(i);
        }
        let mut out: Vec<(Vec<usize>, Stage)> = vec![];
        for g in groups {
            let mut list = g.clone();
            list.extend((0..upto).filter(|&i| global[i]));
            list.sort_unstable();
            let mut own = vec![false; feats.len()];
            for &i in &list {
                own[i] = true;
            }
            let fs: Vec<&Feature> = list.iter().map(|&i| feats[i]).collect();
            ISLAND.with(|c| *c.borrow_mut() = Some(Island { whole: whole.clone(), own, upto }));
            let st = run(doc, &fs, skip, cache, model, used_tools, used_stages);
            ISLAND.with(|c| *c.borrow_mut() = None);
            out.push((g, st));
        }
        if upto == feats.len() && std::env::var_os("FORME_ISLANDS_DEBUG").is_some() {
            for (g, st) in &out {
                eprintln!("ISLAND {:?}: {} faces, boxes {}, errors {:?}", g.iter().map(|&i| format!("{} {}", feats[i].id, feats[i].name)).collect::<Vec<_>>(), st.shape.as_ref().map_or(0, |s| s.face_count()), st.ext.len(), st.errors);
            }
        }
        for i in (0..upto).filter(|&i| global[i]) {
            let id = feats[i].id;
            let seen = out.iter().any(|(_, s)| s.mirrors.iter().any(|m| m.0 == id));
            let kept = out.iter().any(|(_, s)| s.mirrors.iter().any(|m| m.0 == id && m.1));
            if seen && !kept {
                return None;
            }
        }
        let mut hit = false;
        for a in 0..out.len() {
            for b in a + 1..out.len() {
                if boxes_near(&out[a].1.ext, &out[b].1.ext, APART) && sets.join(out[a].0[0], out[b].0[0]) {
                    hit = true;
                    let pair = (feats[out[a].0[0]].id, feats[out[b].0[0]].id);
                    if !cache.hints.contains(&pair) {
                        cache.hints.push(pair);
                    }
                }
            }
        }
        if !hit {
            return Some(out);
        }
    }
}

/// The features built island by island (see above): the stage of the whole
/// part, as `run` gives it for one history.
fn build_islands<'a>(doc: &'a Doc, feats: &[&'a Feature], skip: &[String], cache: &mut Cache, model: &mut Model, used_tools: &mut Vec<u64>, used_stages: &mut Vec<u64>) -> Stage {
    let n = feats.len();
    // FORME_ONE_CHAIN=1: one history, as before (to compare)
    if n < 2 || ONE_CHAIN.with(|c| c.get()) || std::env::var_os("FORME_ONE_CHAIN").is_some() {
        return run(doc, feats, skip, cache, model, used_tools, used_stages);
    }
    let global: Vec<bool> = feats.iter().map(|f| f.mirror.is_some()).collect();
    let mut sets = Sets((0..n).collect());
    // copies and mirror images carry their step's id
    let mut first_of: HashMap<FeatureId, usize> = HashMap::new();
    for (i, f) in feats.iter().enumerate() {
        match first_of.get(&f.id) {
            Some(&j) if !global[i] => {
                sets.join(i, j);
            }
            Some(_) => {}
            None => {
                first_of.insert(f.id, i);
            }
        }
    }
    let names: Vec<Vec<Option<usize>>> = feats.iter().enumerate().map(|(i, f)| if global[i] || f.is_solid() { vec![] } else { crate::doc::named_features(f).iter().map(|id| first_of.get(id).copied()).collect() }).collect();
    let kinds: Vec<u8> = feats.iter().map(|f| f.round.as_ref().map_or(0, |r| if !r.chamfer && !r.only { 1 } else { 2 })).collect();
    // an operation nothing of which is found: with the shape added last
    // before it (in one history it fails on the part built so far; with
    // nothing built yet it is passed over)
    let lone = |sets: &mut Sets, i: usize| {
        if let Some(j) = (0..i).rev().find(|&j| !global[j] && feats[j].is_solid() && feats[j].op == Op::Add) {
            sets.join(i, j);
        }
    };
    let mut all = false;
    let mut floating: Vec<usize> = vec![];
    for i in 0..n {
        let f = feats[i];
        if global[i] || f.is_solid() {
            continue;
        }
        // a Shell with no opening hollows every solid
        all |= f.shell.as_ref().is_some_and(|s| s.faces.is_empty());
        let mut found = false;
        for &p in names[i].iter().flatten() {
            if global[p] {
                // (a face a Mirror's cut left: any island's)
                all = true;
            } else {
                sets.join(i, p);
                found = true;
            }
        }
        if f.tweak.as_ref().is_some_and(|t| t.comps().any(|c| matches!(c, Comp::Vertex(_)))) {
            floating.push(i);
        } else if !found {
            lone(&mut sets, i);
        }
    }
    // the shapes whose boxes come near each other, their mirror images too
    let mut planes: Vec<Mirror> = vec![];
    for f in feats.iter().filter_map(|f| f.mirror.as_ref()) {
        if !planes.iter().any(|m| m.axis == f.axis && m.offset == f.offset) {
            planes.push(*f);
        }
    }
    let boxes: Vec<Vec<[f64; 6]>> = feats
        .iter()
        .enumerate()
        .map(|(i, f)| {
            if global[i] || !f.is_solid() {
                return vec![];
            }
            let (lo, hi) = f.bounds();
            let mut list = vec![[lo.x, lo.y, lo.z, hi.x, hi.y, hi.z]];
            for m in planes.iter().take(3) {
                let a = m.axis.min(2) as usize;
                let more: Vec<[f64; 6]> = list
                    .iter()
                    .map(|b| {
                        let mut r = *b;
                        r[a] = 2.0 * m.offset - b[a + 3];
                        r[a + 3] = 2.0 * m.offset - b[a];
                        r
                    })
                    .collect();
                list.extend(more);
            }
            list
        })
        .collect();
    for i in 0..n {
        for j in i + 1..n {
            if !boxes[i].is_empty() && !boxes[j].is_empty() && boxes_near(&boxes[i], &boxes[j], APART) {
                sets.join(i, j);
            }
        }
    }
    // those found to touch in the builds before
    cache.hints.retain(|(a, b)| a != b);
    if cache.hints.len() > 512 {
        let cut = cache.hints.len() - 512;
        cache.hints.drain(..cut);
    }
    for (a, b) in cache.hints.clone() {
        if let (Some(&i), Some(&j)) = (first_of.get(&a), first_of.get(&b)) {
            if !global[i] && !global[j] {
                sets.join(i, j);
            }
        }
    }
    if all {
        if let Some(k) = (0..n).find(|&i| !global[i]) {
            for i in (0..n).filter(|&i| !global[i]) {
                sets.join(i, k);
            }
        }
    }
    let place: HashMap<usize, usize> = feats.iter().enumerate().map(|(i, f)| (addr(f), i)).collect();
    let mut added = false;
    let joins: Vec<bool> = feats
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let found = names[i].iter().any(|n| n.is_some());
            if global[i] {
                false
            } else if f.is_solid() {
                let j = added;
                added |= f.op == Op::Add;
                j
            } else if let Some(p) = &f.push {
                found && p.depth.abs() > 1e-6
            } else if let Some(t) = &f.tweak {
                found && t.copy && (t.delta.length() > 1e-6 || (t.scale - DVec3::ONE).abs().max_element() > 1e-9)
            } else {
                found && f.round.as_ref().is_some_and(|r| r.only)
            }
        })
        .collect();
    let shared = Rc::new(Whole { place, kinds, names, joins });
    // a move of vertices: with the island that has the vertex nearest each
    // point (`comp_vertices`), on the islands as they are just before it
    for &i in &floating {
        // (a copy's or the mirror side's: with its step, joined above)
        if first_of.get(&feats[i].id) != Some(&i) {
            continue;
        }
        let Some(gs) = islands_upto(doc, feats, i, &global, &mut sets, &shared, skip, cache, model, used_tools, used_stages) else {
            return run(doc, feats, skip, cache, model, used_tools, used_stages);
        };
        let mut found = shared.names[i].iter().flatten().any(|&p| !global[p]);
        let comps: Vec<DVec3> = feats[i].tweak.iter().flat_map(|t| t.comps()).filter_map(|c| if let Comp::Vertex(p) = c { Some(*p) } else { None }).collect();
        for p in comps {
            let mut best: Option<(f64, usize)> = None;
            for (g, s) in &gs {
                let Some(sh) = &s.shape else { continue };
                for q in sh.vertex_points() {
                    let d = DVec3::from_array(q).distance(p);
                    if best.map_or(true, |b| d < b.0) {
                        best = Some((d, g[0]));
                    }
                }
            }
            if let Some((d, j)) = best {
                if d < 1.0 {
                    sets.join(i, j);
                    found = true;
                }
            }
        }
        if !found {
            lone(&mut sets, i);
        }
    }
    let Some(gs) = islands_upto(doc, feats, n, &global, &mut sets, &shared, skip, cache, model, used_tools, used_stages) else {
        return run(doc, feats, skip, cache, model, used_tools, used_stages);
    };
    // side by side
    let mut out = Stage::new();
    let shared_names: Vec<String> = (0..n).filter(|&i| global[i]).map(|i| format!("{}: ", feats[i].name)).collect();
    for (_, s) in gs {
        out.shape = match (out.shape.take(), s.shape) {
            (Some(a), Some(b)) => match a.compound(&b) {
                Ok(c) => Some(Rc::new(c)),
                Err(e) => {
                    out.errors.push(format!("a part of the scene is left out ({e})"));
                    Some(a)
                }
            },
            (a, b) => a.or(b),
        };
        for e in s.errors {
            // (a Mirror's error once, not once per island)
            if !(shared_names.iter().any(|h| e.starts_with(h)) && out.errors.contains(&e)) {
                out.errors.push(e);
            }
        }
        out.limits.extend(s.limits);
        out.tension_limits.extend(s.tension_limits);
        boxes_add(&mut out.ext, &s.ext.iter().map(|b| ([b[0], b[1], b[2]], [b[3], b[4], b[5]])).collect::<Vec<_>>());
    }
    out
}

/// While a shape is dragged (a coarse build: a slider moved live too), the
/// features that changed since the last request, with every later step
/// that touches them, are built after the others; the others' stages are
/// then found in the cache at each step of the drag instead of being made
/// again (21 nov.: the drone's camera, 16th of 28 steps, 1.5 s a step, all
/// of it after the camera made again, its Mirror 0.6 s). The fine build
/// keeps the document's order.
///
/// A later step joins them (in their order) when it names one of their
/// faces, when its box meets one of theirs (a union commutes with a union,
/// a cut or a fillet far from them with anything), when it is an Array
/// repeating one of them, a Shell, or a Mirror that would treat one of them
/// otherwise once it comes after it (made on both sides then, not cut at
/// the plane): one off the side kept, unless a box or a cylinder centred
/// on the plane and square to it, a fillet whose edges there are the
/// images of its edges on the side kept.
fn drag_order(doc: &Doc, last: &[(FeatureId, u64)]) -> Option<Doc> {
    let n = doc.features.len();
    if n < 3 || last.len() != n || doc.features.iter().zip(last).any(|(f, l)| f.id != l.0) {
        return None;
    }
    let mut inc: Vec<bool> = doc.features.iter().zip(last).map(|(f, l)| f.hash() != l.1).collect();
    let k = inc.iter().position(|c| *c)?;
    let built = |f: &Feature| f.visible && !f.suspended;
    let mut regs: Vec<Option<Option<(DVec3, DVec3)>>> = vec![None; n];
    let mut reg = |i: usize| -> Option<(DVec3, DVec3)> {
        if regs[i].is_none() {
            regs[i] = Some(reach(doc, &doc.features[i], 0));
        }
        regs[i].unwrap()
    };
    let meet = |a: Option<(DVec3, DVec3)>, b: Option<(DVec3, DVec3)>| match (a, b) {
        (Some((l0, h0)), Some((l1, h1))) => l0.cmple(h1).all() && l1.cmple(h0).all(),
        _ => true,
    };
    for j in k + 1..n {
        if inc[j] || !built(&doc.features[j]) {
            continue;
        }
        let g = &doc.features[j];
        let members: Vec<usize> = (k..j).filter(|&i| inc[i] && built(&doc.features[i])).collect();
        if members.is_empty() {
            continue;
        }
        let ids: Vec<FeatureId> = members.iter().map(|&i| doc.features[i].id).collect();
        let join = if g.shell.is_some() {
            true
        } else if let Some(m) = &g.mirror {
            members.iter().any(|&i| !mirror_safe(doc, &doc.features[i], m, 0))
        } else if g.array.is_some() {
            let pat: Vec<FeatureId> = doc.array_pattern(j).iter().map(|f| f.id).collect();
            let rg = reg(j);
            ids.iter().any(|id| pat.contains(id)) || members.iter().any(|&i| meet(reg(i), rg))
        } else if g.round.is_some() || g.push.is_some() || g.tweak.is_some() || g.cut.is_some() {
            let named = crate::doc::named_features(g);
            let rg = reg(j);
            named.iter().any(|id| ids.contains(id)) || members.iter().any(|&i| meet(reg(i), rg))
        } else if doc.makes(g) == Makes::Nothing {
            false
        } else {
            let rg = reg(j);
            members.iter().any(|&i| meet(reg(i), rg))
        };
        if join {
            inc[j] = true;
        }
    }
    // nothing left before them that was after them: no gain
    if !(k..n).any(|i| !inc[i] && built(&doc.features[i])) {
        return None;
    }
    let mut out = doc.clone();
    out.features = (0..n).filter(|&i| !inc[i]).chain((0..n).filter(|&i| inc[i])).map(|i| doc.features[i].clone()).collect();
    if std::env::var_os("FORME_ISLANDS_DEBUG").is_some() {
        eprintln!("DRAG ORDER: last {:?}", (0..n).filter(|&i| inc[i]).map(|i| doc.features[i].id).collect::<Vec<_>>());
    }
    Some(out)
}

/// Where a feature can change the part, in world space (`drag_order`):
/// None, anywhere (a Mirror, a Shell, what cannot be told).
fn reach(doc: &Doc, f: &Feature, depth: u32) -> Option<(DVec3, DVec3)> {
    if depth > 6 || f.mirror.is_some() || f.shell.is_some() {
        return None;
    }
    let grow = |(lo, hi): (DVec3, DVec3), d: f64| (lo - DVec3::splat(d), hi + DVec3::splat(d));
    let add = |a: Option<(DVec3, DVec3)>, b: (DVec3, DVec3)| Some(match a {
        Some((l, h)) => (l.min(b.0), h.max(b.1)),
        None => b,
    });
    let named = |f: &Feature| -> Option<Option<(DVec3, DVec3)>> {
        let mut b = None;
        for id in crate::doc::named_features(f) {
            let g = doc.features.iter().find(|g| g.id == id && g.id != f.id)?;
            b = add(b, reach(doc, g, depth + 1)?);
        }
        Some(b)
    };
    if let Some(a) = &f.array {
        let at = doc.features.iter().position(|g| g.id == f.id)?;
        let copies = a.copies();
        let mut b = None;
        for p in doc.array_pattern(at) {
            let r = reach(doc, &p, depth + 1)?;
            b = add(b, r);
            for (_, m) in &copies {
                let c: Vec<DVec3> = (0..8).map(|i| m.transform_point3(DVec3::new(if i & 1 == 0 { r.0.x } else { r.1.x }, if i & 2 == 0 { r.0.y } else { r.1.y }, if i & 4 == 0 { r.0.z } else { r.1.z }))).collect();
                let lo = c.iter().fold(DVec3::splat(f64::MAX), |a, q| a.min(*q));
                let hi = c.iter().fold(DVec3::splat(f64::MIN), |a, q| a.max(*q));
                b = add(b, (lo, hi));
            }
        }
        return b;
    }
    if let Some(r) = &f.round {
        return named(f)?.map(|b| grow(b, r.size.abs() * 2.0 + 0.5));
    }
    if let Some(p) = &f.push {
        return named(f)?.map(|b| grow(b, p.depth.abs() + p.inset.abs() + 0.5));
    }
    if let Some(t) = &f.tweak {
        if (t.scale - DVec3::ONE).abs().max_element() > 1e-9 || t.copy {
            return None;
        }
        return named(f)?.map(|b| grow(b, t.delta.length() + 0.5));
    }
    if f.cut.is_some() || f.bridge.is_some() {
        return named(f)?.map(|b| grow(b, 0.5));
    }
    Some(grow(f.bounds(), 0.5))
}

/// Whether a feature made after this Mirror instead of before it (on both
/// sides then) gives the same part (`drag_order`).
fn mirror_safe(doc: &Doc, f: &Feature, m: &Mirror, depth: u32) -> bool {
    let tol = 1e-6;
    let kept = |r: Option<(DVec3, DVec3)>| r.is_some_and(|(lo, hi)| m.side(lo).min(m.side(hi)) >= -tol);
    if f.mirror.is_some() || f.shell.is_some() || f.one_side || f.bridge.is_some() || depth > 6 {
        return false;
    }
    // a face's image across the plane, for a face of a plain shape centred
    // on it (a box's ±side across the plane swapped, the rest its own)
    let sigma = |tag: i64| -> Option<i64> {
        let g = doc.features.iter().find(|g| g.id == (tag >> 16) as FeatureId)?;
        let a = sym_axis(g, m)? as i64;
        let sub = tag & 0xffff;
        let s2 = match g.prim {
            Prim::Box { .. } if sub <= 5 => {
                if sub / 2 == a {
                    sub ^ 1
                } else {
                    sub
                }
            }
            Prim::Cylinder { .. } if sub <= 2 => {
                if a == 1 && sub > 0 {
                    3 - sub
                } else {
                    sub
                }
            }
            _ => return None,
        };
        Some((tag & !0xffff) | s2)
    };
    if let Some(r) = &f.round {
        // each edge on the side kept, the image of another edge, or its own
        // image (between two faces of shapes centred on the plane that are
        // their own images or each other's)
        return r.edges.iter().all(|e| {
            m.side(e.at) >= -tol
                || r.edges.iter().any(|o| o.at.distance(m.reflect(e.at)) < 1e-3)
                || match (sigma(e.faces[0]), sigma(e.faces[1])) {
                    (Some(x), Some(y)) => (x, y) == (e.faces[0], e.faces[1]) || (x, y) == (e.faces[1], e.faces[0]),
                    _ => false,
                }
        });
    }
    if let Some(p) = &f.push {
        return m.side(p.face.at) >= -tol || sigma(p.face.tag) == Some(p.face.tag);
    }
    if f.tweak.is_some() || f.cut.is_some() || f.array.is_some() {
        return kept(reach(doc, f, 0));
    }
    kept(Some(f.bounds())) || sym_axis(f, m).is_some()
}

/// A plain box or cylinder centred on a Mirror's plane, one of its own axes
/// square to it: that axis (0 X, 1 Y, 2 Z), it is its own image.
fn sym_axis(f: &Feature, m: &Mirror) -> Option<usize> {
    let plain = matches!(f.prim, Prim::Box { .. } | Prim::Cylinder { .. }) && f.is_solid() && f.profile.is_none() && f.form.is_none() && f.curve.is_none() && f.sheet.is_none() && f.revolve.is_none() && f.loft.is_none() && f.shape_bool.is_none();
    if !plain {
        return None;
    }
    let (lo, hi) = f.bounds();
    let size = (hi - lo).max_element().max(1.0);
    if m.side((lo + hi) * 0.5).abs() > 1e-6 * size {
        return None;
    }
    let n = m.normal();
    [DVec3::X, DVec3::Y, DVec3::Z].iter().position(|a| (f.rot * *a).dot(n).abs() > 1.0 - 1e-9)
}

/// The edges a Mirror's union keeps between a curved face and its own image
/// (`simplify_keeping`: never joined, OpenCascade could loop for ever there)
/// are drawn and picked as a seam: both faces of one tag, curved, the edge
/// on a Mirror's plane.
fn mirror_seams(doc: &Doc, model: &mut Model) {
    let planes: Vec<(DVec3, f64)> = doc.features.iter().filter(|f| f.visible && !f.suspended).filter_map(|f| f.mirror.as_ref()).map(|m| (m.normal(), m.offset)).collect();
    if planes.is_empty() {
        return;
    }
    for e in model.edges.iter_mut() {
        if e.seam || e.faces[0] < 0 || e.faces[1] < 0 || e.faces[0] == e.faces[1] || e.pts.is_empty() {
            continue;
        }
        let (fa, fb) = (&model.faces[e.faces[0] as usize], &model.faces[e.faces[1] as usize]);
        if fa.tag.is_none() || fa.tag != fb.tag || fa.kind == SurfKind::Plane || fb.kind == SurfKind::Plane {
            continue;
        }
        let on = |(n, o): &(DVec3, f64)| e.pts.iter().all(|p| (DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64).dot(*n) - o).abs() < 0.01);
        if planes.iter().any(on) {
            e.seam = true;
        }
    }
}

pub fn build(doc: &Doc, fine: bool, quads: bool, skip: &[String], cache: &mut Cache) -> Model {
    let t0 = Instant::now();
    let mut model = Model { fine, ..Default::default() };
    let mut used_tools = vec![];
    let mut used_stages = vec![];
    // during a drag, what it changes built last
    // a Mirror whose later steps are made on one side only
    let sided = sided_doc(doc);
    let doc = sided.as_ref().unwrap_or(doc);
    let ordered = if fine || quads || std::env::var_os("FORME_NO_DRAG_ORDER").is_some() { None } else { drag_order(doc, &cache.last) };
    cache.last = doc.features.iter().map(|f| (f.id, f.hash())).collect();
    let doc = ordered.as_ref().unwrap_or(doc);
    // each Array's copies, then after a Mirror each step on both sides
    let arrayed = arrayed_doc(doc);
    let doc = arrayed.as_ref().unwrap_or(doc);
    let mirrored = mirrored_doc(doc);
    let doc = mirrored.as_ref().unwrap_or(doc);
    let feats: Vec<&Feature> = doc.features.iter().filter(|f| f.visible && !f.suspended && doc.makes(f) == Makes::Solid).collect();
    // an inset left as a line: its inside lies on the surface of the face
    // round it, and the next union or cut made anywhere (or a Mirror) made
    // them one again; its face keeps its outline
    let lines: Vec<i64> = feats.iter().filter(|f| f.push.as_ref().is_some_and(|p| p.inset > 0.0 && p.depth.abs() < 1e-6)).map(|f| FaceTag { feature: f.id, sub: 1 }.pack()).collect();
    forme_occt::keep_lines(&lines);
    HASHES.with(|m| m.borrow_mut().clear());
    let mut stage = build_islands(doc, &feats, skip, cache, &mut model, &mut used_tools, &mut used_stages);
    HASHES.with(|m| m.borrow_mut().clear());
    // the surfaces, beside the part (no boolean)
    // a curve filled, turned open, a loft of open curves (cached as tools)
    for f in doc.features.iter().filter(|f| f.visible && !f.suspended && f.sheet.is_none() && doc.makes(f) == Makes::Sheet) {
        let fh = deep_hash(doc, f);
        let made = match cache.tools.get(&fh) {
            Some(t) => Ok(t.clone()),
            None => {
                mark(&format!("sheet:{}:0", f.id));
                sheet_tool(doc, f).map(Rc::new)
            }
        };
        used_tools.push(fh);
        let made = made.and_then(|t| {
            cache.tools.insert(fh, t.clone());
            match &stage.shape {
                Some(acc) => acc.compound(&t).map(Rc::new),
                None => Ok(t),
            }
        });
        match made {
            Ok(s) => stage.shape = Some(s),
            Err(e) => stage.errors.push(format!("{}: {e}", f.name)),
        }
    }
    for f in doc.features.iter().filter(|f| f.visible && !f.suspended && f.is_sheet()) {
        let sh = f.sheet.as_ref().unwrap();
        let m = f.matrix();
        let pts: Vec<[f64; 3]> = sh.pts.iter().map(|q| m.transform_point3(*q).to_array()).collect();
        let inner: Vec<[f64; 3]> = sh.inner.iter().map(|q| m.transform_point3(*q).to_array()).collect();
        mark(&format!("sheet:{}:0", f.id));
        let filled = match sh.grid {
            // on the scan: a surface through the grid's heights, trimmed
            Some([nx, nz]) if sh.inner.len() == nx * nz && nx >= 2 && nz >= 2 => {
                let (a, y, z, o) = (m.x_axis.truncate(), m.y_axis.truncate(), m.z_axis.truncate(), m.w_axis.truncate());
                let fr = [o.x, o.y, o.z, a.x, a.y, a.z, y.x, y.y, y.z, z.x, z.y, z.z];
                let h: Vec<f64> = sh.inner.iter().map(|q| q.y).collect();
                let (lo, hi) = (sh.inner[0], sh.inner[nx * nz - 1]);
                let poly: Vec<[f64; 2]> = sh.pts.iter().map(|q| [q.x, q.z]).collect();
                // should it fail, the outline filled as before
                Shape::sheet_grid(&fr, nx, nz, [lo.x, lo.z], [hi.x, hi.z], &h, &poly).or_else(|e| Shape::fill(&pts, &inner).map_err(|_| e))
            }
            _ => Shape::fill(&pts, &inner),
        };
        let made = filled.and_then(|mut s| {
            s.set_tags(&vec![FaceTag { feature: f.id, sub: 0 }.pack(); s.face_count().max(1)]);
            match &stage.shape {
                Some(acc) => acc.compound(&s),
                None => Ok(s),
            }
        });
        match made {
            Ok(s) => stage.shape = Some(Rc::new(s)),
            Err(e) => stage.errors.push(format!("{}: {e}", f.name)),
        }
    }
    // keep what this document uses (the next request is most often the same
    // document with one feature changed)
    let prev: std::collections::HashSet<u64> = std::mem::take(&mut cache.prev).into_iter().collect();
    let used: std::collections::HashSet<u64> = used_stages.iter().copied().collect();
    // (a fine build: the document at rest, one of the undo steps)
    if fine {
        let mut now: Vec<u64> = used.iter().copied().collect();
        now.sort_unstable();
        if cache.rest.back() != Some(&now) {
            cache.rest.push_back(now);
        }
        while cache.rest.len() > REST {
            cache.rest.pop_front();
        }
    }
    let rest: std::collections::HashSet<u64> = cache.rest.iter().flatten().copied().collect();
    cache.stages.retain(|k, _| used.contains(k) || prev.contains(k) || rest.contains(k));
    let stages = &cache.stages;
    cache.passes.retain(|k, _| stages.contains_key(k));
    cache.prev = used.into_iter().collect();
    if cache.tools.len() > 256 {
        cache.tools.retain(|k, _| used_tools.contains(k));
    }
    model.errors = stage.errors.clone();
    // an Array's shapes it cannot repeat
    for f in doc.features.iter().filter(|f| f.visible && !f.suspended) {
        let Some(a) = &f.array else { continue };
        for g in a.sources.iter().filter_map(|id| doc.feature(*id)) {
            if g.shape_bool.is_some() || g.loft.is_some() {
                model.errors.push(format!("{}: {} is not repeated (its curves are elsewhere)", f.name, g.name));
            }
        }
    }
    // the Curves set's 2D booleans: made exactly, their edges sent back
    let mut regions = HashMap::new();
    for f in doc.features.iter().filter(|f| f.visible && !f.suspended && f.shape_bool.is_some()) {
        let h = deep_hash(doc, f);
        let made = match cache.regions.remove(&h) {
            Some(r) => r,
            None => {
                mark(&format!("region:{}:0", f.id));
                region(doc, f, 0).and_then(|r| r.edge_lines(0.01, 0.05)).map(|lines| lines.iter().map(|l| l.iter().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect()).collect())
            }
        };
        match &made {
            Ok(lines) if lines.is_empty() => model.errors.push(format!("{}: nothing left (the curves share nothing)", f.name)),
            Ok(lines) => model.curves.push((f.id, lines.clone())),
            Err(e) => model.errors.push(format!("{}: {e}", f.name)),
        }
        regions.insert(h, made);
    }
    cache.regions = regions;
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
                mirror_seams(doc, &mut model);
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

    /// The mesh's volume (closed: the divergence theorem).
    fn mesh_volume(m: &Model) -> f64 {
        m.triangles
            .iter()
            .map(|t| {
                let p = |i: u32| DVec3::from_array(m.positions[i as usize].map(|x| x as f64));
                p(t[0]).dot(p(t[1]).cross(p(t[2]))) / 6.0
            })
            .sum()
    }

    #[test]
    fn shell_live() {
        use crate::doc::Shell;
        // a 40 × 20 × 30 block (20 high), its top open, walls of 2
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let top = FaceRef { tag: tag(b, 2), at: DVec3::new(5.0, 20.0, 3.0) };
        let s = d.add_shell(Shell { faces: vec![top], thickness: 2.0, outward: false }, top.at);
        assert_eq!(d.feature(s).unwrap().kind_label(), "Shell");
        assert!(!d.feature(s).unwrap().is_solid());
        let mut cache = Cache::default();
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!((mesh_volume(&m) - (24000.0 - 36.0 * 18.0 * 26.0)).abs() < 2.0, "{}", mesh_volume(&m));
        // the block's five faces kept; its inner walls and the rim the step's
        let by = |id: u32| m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(id)).count();
        assert_eq!(by(b), 5);
        assert_eq!(by(s), 6);
        // a fillet on an inner edge (the floor along the +Z wall), picked
        // by its faces' tags as any edge
        let e = edge_near(&m, DVec3::new(0.0, 2.0, 13.0));
        assert!(e.faces.iter().all(|t| (*t >> 16) as u32 == s), "{e:?}");
        let r = d.add_round(1.0, false, vec![e]);
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).count(), 1);
        d.remove(&[r]);
        // closed: a cavity inside
        d.feature_mut(s).unwrap().shell.as_mut().unwrap().faces.clear();
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!((mesh_volume(&m) - (24000.0 - 36.0 * 16.0 * 26.0)).abs() < 2.0, "{}", mesh_volume(&m));
        // outwards, the top open: the walls round it
        {
            let sh = d.feature_mut(s).unwrap().shell.as_mut().unwrap();
            sh.faces = vec![top];
            sh.outward = true;
        }
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!((mesh_volume(&m) - (44.0 * 22.0 * 34.0 - 24000.0)).abs() < 2.0, "{}", mesh_volume(&m));
        let (lo, hi) = m.bounds().unwrap();
        assert!((lo[1] + 2.0).abs() < 1e-3 && (hi[1] - 20.0).abs() < 1e-3, "{lo:?} {hi:?}");
        // too thick (the block is 30 deep): the thickest that works
        {
            let sh = d.feature_mut(s).unwrap().shell.as_mut().unwrap();
            sh.outward = false;
            sh.thickness = 16.0;
        }
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let l = m.limits.iter().find(|(id, _)| *id == s).map(|x| x.1).unwrap_or(0.0);
        assert!(l > 14.0 && l < 15.0, "{:?}", m.limits);
        // its vertical edges rounded before it (5 mm): the walls follow
        let mut d2 = Doc::default();
        let b2 = d2.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let mut c2 = Cache::default();
        let m = build(&d2, true, false, &[], &mut c2);
        let es: Vec<EdgeRef> = [(20.0, 15.0), (-20.0, 15.0), (20.0, -15.0), (-20.0, -15.0)].iter().map(|(x, z)| edge_near(&m, DVec3::new(*x, 10.0, *z))).collect();
        d2.add_round(5.0, false, es);
        let top2 = FaceRef { tag: tag(b2, 2), at: DVec3::new(0.0, 20.0, 0.0) };
        let s2 = d2.add_shell(Shell { faces: vec![top2], thickness: 2.0, outward: false }, top2.at);
        let m = build(&d2, true, false, &[], &mut c2);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let v_round = 24000.0 - 4.0 * (25.0 - std::f64::consts::PI * 25.0 / 4.0) * 20.0;
        let v_in = 36.0 * 26.0 * 18.0 - 4.0 * (9.0 - std::f64::consts::PI * 9.0 / 4.0) * 18.0;
        assert!((mesh_volume(&m) - (v_round - v_in)).abs() < 3.0, "{} {}", mesh_volume(&m), v_round - v_in);
        assert!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(s2)).count() >= 9);
        // after a Mirror: a block from x = −10 to 30, its +X end opened, the
        // −X end (its image) opens too
        let mut d3 = Doc::default();
        let b3 = d3.add(Op::Add, Prim::Box { x: 40.0, y: 10.0, z: 20.0 }, DVec3::new(10.0, 0.0, 0.0), DQuat::IDENTITY);
        d3.add_mirror(Mirror { axis: 0, offset: 0.0, flip: false, single: false });
        let end = FaceRef { tag: tag(b3, 0), at: DVec3::new(30.0, 5.0, 0.0) };
        d3.add_shell(Shell { faces: vec![end], thickness: 1.0, outward: false }, end.at);
        let mut c3 = Cache::default();
        let m = build(&d3, true, false, &[], &mut c3);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!((mesh_volume(&m) - (60.0 * 10.0 * 20.0 - 60.0 * 8.0 * 18.0)).abs() < 2.0, "{}", mesh_volume(&m));
    }

    #[test]
    #[ignore]
    fn shell_probe() {
        use crate::doc::Shell;
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Box { x: 80.0, y: 30.0, z: 50.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let corners: Vec<EdgeRef> = [(0u16, 4u16, 40.0, 25.0), (1, 4, -40.0, 25.0), (0, 5, 40.0, -25.0), (1, 5, -40.0, -25.0)].iter().map(|(a, b, x, z)| EdgeRef { faces: [tag(c, *a), tag(c, *b)], at: DVec3::new(*x, 15.0, *z) }).collect();
        d.add_round(8.0, false, corners);
        if std::env::var("FORME_HOLE").map(|v| v == "1").unwrap_or(true) {
            d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 40.0 }, DVec3::new(24.0, -5.0, 0.0), DQuat::IDENTITY);
        }
        let top = FaceRef { tag: tag(c, 2), at: DVec3::new(-12.0, 30.0, 6.0) };
        let front = FaceRef { tag: tag(c, 4), at: DVec3::new(-14.0, 14.0, 25.0) };
        for faces in [vec![top], vec![front], vec![top, front]] {
            for t in [1.0, 2.0] {
                let mut e = d.clone();
                e.add_shell(Shell { faces: faces.clone(), thickness: t, outward: false }, top.at);
                let m = build(&e, true, false, &[], &mut Cache::default());
                eprintln!("{} faces t {t}: {} faces, volume {:.0}, errors {:?}", faces.len(), m.faces.len(), mesh_volume(&m), m.errors);
            }
        }
    }

    #[test]
    fn mirror_live() {
        use crate::doc::Mirror;
        // a block from x = −10 to 30 across the plane x = 0, mirrored: −30
        // to 30, its top one face (no seam)
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 10.0, z: 20.0 }, DVec3::new(10.0, 0.0, 0.0), DQuat::IDENTITY);
        let mi = d.add_mirror(Mirror { axis: 0, offset: 0.0, flip: false, single: false });
        assert_eq!(d.feature(mi).unwrap().kind_label(), "Mirror");
        assert!(!d.feature(mi).unwrap().is_solid());
        let mut cache = Cache::default();
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, hi) = m.bounds().unwrap();
        assert!((lo[0] + 30.0).abs() < 1e-3 && (hi[0] - 30.0).abs() < 1e-3, "{lo:?} {hi:?}");
        assert_eq!(m.faces.len(), 6, "{:?}", m.faces.iter().map(|f| f.tag).collect::<Vec<_>>());
        assert!((mesh_volume(&m) - 60.0 * 10.0 * 20.0).abs() < 1.0, "{}", mesh_volume(&m));
        // both ends are the block's +X face (tag sub 0)
        assert_eq!(m.faces.iter().filter(|f| f.tag == Some(FaceTag { feature: b, sub: 0 })).count(), 2);
        // flipped: the −X side kept (x from −10 to 0), mirrored: −10 to 10
        d.feature_mut(mi).unwrap().mirror.as_mut().unwrap().flip = true;
        let m = build(&d, true, false, &[], &mut cache);
        let (lo, hi) = m.bounds().unwrap();
        assert!((lo[0] + 10.0).abs() < 1e-3 && (hi[0] - 10.0).abs() < 1e-3, "{lo:?} {hi:?}");
        d.feature_mut(mi).unwrap().mirror.as_mut().unwrap().flip = false;
        // a hole made after it at x = 15: one at −15 too
        let h = d.add(Op::Carve, Prim::Cylinder { radius: 3.0, height: 20.0 }, DVec3::new(15.0, -5.0, 0.0), DQuat::IDENTITY);
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(h)).count(), 2);
        let v1 = 60.0 * 10.0 * 20.0 - 2.0 * std::f64::consts::PI * 9.0 * 10.0;
        assert!((mesh_volume(&m) - v1).abs() < 2.0, "{} {v1}", mesh_volume(&m));
        // the Mirror's later steps on one side only: the hole at 15 alone,
        // a push of the front face on the mirror side alone
        d.feature_mut(mi).unwrap().mirror.as_mut().unwrap().single = true;
        let front = FaceRef { tag: tag(b, 4), at: DVec3::new(-20.0, 5.0, 10.0) };
        let p = d.add_push(Push { face: front, inset: 4.0, depth: 2.0 });
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(h)).count(), 1);
        let v_one = 60.0 * 10.0 * 20.0 - std::f64::consts::PI * 9.0 * 10.0 + 52.0 * 2.0 * 2.0;
        assert!((mesh_volume(&m) - v_one).abs() < 2.0, "{} {v_one}", mesh_volume(&m));
        d.remove(&[p]);
        d.feature_mut(mi).unwrap().mirror.as_mut().unwrap().single = false;
        let m = build(&d, true, false, &[], &mut cache);
        assert_eq!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(h)).count(), 2);
        // a box centred on the plane after it: made once (its own image)
        let c = d.add(Op::Add, Prim::Box { x: 10.0, y: 5.0, z: 10.0 }, DVec3::new(0.0, 10.0, 0.0), DQuat::IDENTITY);
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!((mesh_volume(&m) - (v1 + 500.0)).abs() < 2.0, "{}", mesh_volume(&m));
        d.remove(&[c]);
        // the front face (across the plane) pushed out by 3: once, not twice
        let front = FaceRef { tag: tag(b, 4), at: DVec3::new(-20.0, 5.0, 10.0) };
        let p = d.add_push(Push { face: front, inset: 0.0, depth: 3.0 });
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, hi) = m.bounds().unwrap();
        assert!((hi[2] - 13.0).abs() < 1e-3 && (lo[2] + 10.0).abs() < 1e-3, "{lo:?} {hi:?}");
        d.remove(&[p]);
        // a fillet on the +X end's top edge: the −X end's too
        let m = build(&d, true, false, &[], &mut cache);
        let e = edge_near(&m, DVec3::new(30.0, 10.0, 0.0));
        let r = d.add_round(2.0, false, vec![e]);
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).count(), 2);
        let cut = (4.0 - std::f64::consts::PI) * 20.0;
        assert!((mesh_volume(&m) - (v1 - 2.0 * cut)).abs() < 2.0, "{} {}", mesh_volume(&m), v1 - 2.0 * cut);
        // the −X end's bottom edge picked on the mirror side: the +X end's too
        let e = edge_near(&m, DVec3::new(-30.0, 0.0, 0.0));
        let r2 = d.add_round(1.0, false, vec![e]);
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r2)).count(), 2);
        // a vertex moved on the +X side: its image on the −X side
        let t = d.add_tweak(Tweak { comp: Comp::Vertex(DVec3::new(30.0, 0.0, -10.0)), delta: DVec3::new(0.0, 0.0, -4.0), more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
        let _ = t;
        d.remove(&[r, r2]);
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, _) = m.bounds().unwrap();
        assert!((lo[2] + 14.0).abs() < 1e-3, "{lo:?}");
        // the vertex at the −X end moved down too: the part stays symmetric
        let low: Vec<[f32; 3]> = m.positions.iter().filter(|q| q[2] < -13.9).copied().collect();
        assert!(low.iter().any(|q| q[0] > 29.0) && low.iter().any(|q| q[0] < -29.0), "{low:?}");
    }

    /// The edge whose polyline passes nearest `p` (its middle as `at`).
    fn edge_through(m: &Model, p: DVec3) -> EdgeRef {
        let d = |e: &crate::model::Edge| e.pts.iter().map(|q| DVec3::new(q[0] as f64, q[1] as f64, q[2] as f64).distance(p)).fold(f64::MAX, f64::min);
        let e = m.edges.iter().filter(|e| !e.seam && e.faces[0] >= 0 && e.faces[1] >= 0).min_by(|a, b| d(a).total_cmp(&d(b))).unwrap();
        let t = |i: i32| m.faces[i as usize].tag.unwrap().pack();
        let q = e.pts[e.pts.len() / 2];
        EdgeRef { faces: [t(e.faces[0]), t(e.faces[1])], at: DVec3::new(q[0] as f64, q[1] as f64, q[2] as f64) }
    }

    #[test]
    fn array_live() {
        use crate::doc::{Array, ArrayKind};
        // a plate, a hole with a rounded rim: repeated 4 times along X
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 80.0, y: 10.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let h = d.add(Op::Carve, Prim::Cylinder { radius: 3.0, height: 20.0 }, DVec3::new(-30.0, -5.0, -7.0), DQuat::IDENTITY);
        let mut cache = Cache::default();
        let m = build(&d, true, false, &[], &mut cache);
        let r = d.add_round(1.0, false, vec![edge_through(&m, DVec3::new(-27.0, 10.0, -7.0))]);
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let one = 80.0 * 10.0 * 30.0 - mesh_volume(&m);
        let a = d.add_array(Array { sources: vec![h], kind: ArrayKind::Line, count: 4, step: DVec3::new(20.0, 0.0, 0.0), count2: 1, step2: DVec3::ZERO, axis: 1, centre: DVec3::ZERO, angle: 360.0 });
        let fa = d.feature(a).unwrap();
        assert_eq!(fa.kind_label(), "Array");
        assert!(!fa.is_solid());
        assert_eq!(d.array_pattern(3).iter().map(|f| f.id).collect::<Vec<_>>(), vec![h, r]);
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let holes = |m: &Model| {
            let mut v = m.faces.iter().filter(|f| f.tag == Some(FaceTag { feature: h, sub: 0 })).map(|f| (f.axis_origin[0] * 10.0).round() / 10.0).collect::<Vec<_>>();
            v.sort_by(f64::total_cmp);
            v
        };
        assert_eq!(holes(&m), vec![-30.0, -10.0, 10.0, 30.0], "{:?}", holes(&m));
        assert_eq!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r)).count(), 4);
        let v4 = 80.0 * 10.0 * 30.0 - 4.0 * one;
        assert!((mesh_volume(&m) - v4).abs() < 2.0, "{} {v4}", mesh_volume(&m));
        // in a grid of 4 × 2: 8
        let set = |d: &mut Doc, f: &dyn Fn(&mut Array)| f(d.feature_mut(a).unwrap().array.as_mut().unwrap());
        set(&mut d, &|x| {
            x.kind = ArrayKind::Grid;
            x.count2 = 2;
            x.step2 = DVec3::new(0.0, 0.0, 14.0);
        });
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(holes(&m).len(), 8);
        let v8 = 80.0 * 10.0 * 30.0 - 8.0 * one;
        assert!((mesh_volume(&m) - v8).abs() < 3.0, "{} {v8}", mesh_volume(&m));
        // the original moved: the copies follow
        set(&mut d, &|x| x.kind = ArrayKind::Line);
        d.feature_mut(h).unwrap().origin.x = -32.0;
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(holes(&m), vec![-32.0, -12.0, 8.0, 28.0], "{:?}", holes(&m));
        d.feature_mut(h).unwrap().origin.x = -30.0;
        // a fillet made after the Array on the third copy's bottom rim:
        // moved before it, brought back to the original, made on all four
        let m = build(&d, true, false, &[], &mut cache);
        let r2 = d.add_round(0.5, false, vec![edge_through(&m, DVec3::new(13.0, 0.0, -7.0))]);
        assert!(d.keep_arrays());
        let order: Vec<u32> = d.features.iter().map(|f| f.id).collect();
        assert_eq!(order, vec![1, h, r, r2, a]);
        let at = d.feature(r2).unwrap().round.as_ref().unwrap().edges[0].at;
        assert!((at.x + 30.0).abs() < 3.5 && at.y.abs() < 1e-3, "{at:?}");
        assert!(!d.keep_arrays());
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(r2)).count(), 4);
        // round a disc: a boss and its fillet 6 times round Y
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Cylinder { radius: 30.0, height: 8.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let b = d.add(Op::Add, Prim::Box { x: 6.0, y: 4.0, z: 6.0 }, DVec3::new(20.0, 8.0, 0.0), DQuat::IDENTITY);
        let m = build(&d, true, false, &[], &mut cache);
        d.add_round(1.0, false, vec![edge_through(&m, DVec3::new(23.0, 12.0, 0.0))]);
        let a = d.add_array(Array { sources: vec![b], kind: ArrayKind::Circle, count: 6, step: DVec3::ZERO, count2: 1, step2: DVec3::ZERO, axis: 1, centre: DVec3::ZERO, angle: 360.0 });
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let tops: Vec<[f64; 3]> = m.faces.iter().filter(|f| f.tag == Some(FaceTag { feature: b, sub: 2 })).map(|f| f.point).collect();
        assert_eq!(tops.len(), 6);
        // the copies' tops all 12 high, all 20 from the axis or so
        assert!(tops.iter().all(|p| (p[1] - 12.0).abs() < 1e-3), "{tops:?}");
        // half round: 4 over 180°, the last at −X
        let fa = d.feature_mut(a).unwrap().array.as_mut().unwrap();
        fa.count = 4;
        fa.angle = 180.0;
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, _) = m.bounds().unwrap();
        assert!((lo[1] - 0.0).abs() < 1e-3);
        let tops: Vec<[f64; 3]> = m.faces.iter().filter(|f| f.tag == Some(FaceTag { feature: b, sub: 2 })).map(|f| f.point).collect();
        assert_eq!(tops.len(), 4);
        // deleted, the original stays alone; the Array goes with its shape
        d.remove(&[b]);
        assert!(d.feature(a).is_none());
    }

    #[test]
    #[ignore]
    fn mirror_probe() {
        // the push across the plane after fillets at both ends: with the
        // Mirror, and the same block made whole without it
        for mirrored in [false, true] {
            let mut d = Doc::default();
            let b = if mirrored { d.add(Op::Add, Prim::Box { x: 40.0, y: 10.0, z: 20.0 }, DVec3::new(10.0, 0.0, 0.0), DQuat::IDENTITY) } else { d.add(Op::Add, Prim::Box { x: 60.0, y: 10.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY) };
            if mirrored {
                d.add_mirror(crate::doc::Mirror { axis: 0, offset: 0.0, flip: false, single: false });
            }
            let mut cache = Cache::default();
            let m = build(&d, true, false, &[], &mut cache);
            d.add(Op::Carve, Prim::Cylinder { radius: 3.0, height: 20.0 }, DVec3::new(15.0, -5.0, 0.0), DQuat::IDENTITY);
            if !mirrored {
                d.add(Op::Carve, Prim::Cylinder { radius: 3.0, height: 20.0 }, DVec3::new(-15.0, -5.0, 0.0), DQuat::IDENTITY);
            }
            let m = build(&d, true, false, &[], &mut cache);
            let e = edge_near(&m, DVec3::new(30.0, 10.0, 0.0));
            let e2 = edge_near(&m, DVec3::new(-30.0, 10.0, 0.0));
            d.add_round(2.0, false, if mirrored { vec![e] } else { vec![e, e2] });
            let m = build(&d, true, false, &[], &mut cache);
            let e = edge_near(&m, DVec3::new(-30.0, 0.0, 0.0));
            let e2 = edge_near(&m, DVec3::new(30.0, 0.0, 0.0));
            d.add_round(1.0, false, if mirrored { vec![e] } else { vec![e, e2] });
            let m = build(&d, true, false, &[], &mut cache);
            eprintln!("mirrored {mirrored}: {} faces {:?}", m.faces.len(), m.errors);
            let front = FaceRef { tag: tag(b, 4), at: DVec3::new(-20.0, 5.0, 10.0) };
            d.add_push(Push { face: front, inset: 0.0, depth: 3.0 });
            let t0 = Instant::now();
            let m = build(&d, true, false, &[], &mut cache);
            eprintln!("  push: {} faces {:?} {:.0} ms", m.faces.len(), m.errors, t0.elapsed().as_secs_f64() * 1e3);
        }
    }

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
        let pr = crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, -0.2], [-0.2, -0.2], [-0.2, 0.5], [-0.5, 0.5]], smooth: vec![false, false, false, true, false, false], corner: vec![], tension: vec![], open: false, spline: false };
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
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.0, 0.5]], smooth: vec![false, false, true], corner: vec![], tension: vec![], open: false, spline: false });
        let m3 = build(&d, true, false, &[], &mut cache);
        assert!(m3.errors.is_empty(), "{:?}", m3.errors);
        assert!(m3.faces.iter().any(|f| f.tag.map(|t| t.feature) == Some(c)), "the pocket's faces carry the sketch's tags");
        // a bow tie crosses itself: an error, the block stays
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, 0.5], [0.5, -0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![], tension: vec![], open: false, spline: false });
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
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![4.0, -3.0, 50.0, 0.0], tension: vec![], open: false, spline: false });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let cyl: Vec<f64> = m.faces.iter().filter(|f| f.kind == SurfKind::Cylinder).map(|f| f.radius).collect();
        assert_eq!(cyl.len(), 2, "two rounded corners: {cyl:?}");
        assert!(cyl.iter().any(|r| (r - 4.0).abs() < 1e-6), "{cyl:?}");
        assert!(cyl.iter().any(|r| (r - 10.0).abs() < 1e-6), "the big one stops at half a side: {cyl:?}");
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
    fn slot_outline() {
        // a slot 20 × 6 mm, 4 high: a rectangle whose four corners are cut
        // at half its width, so each end is a half circle (two arcs, no
        // straight piece between them)
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Profile { x: 20.0, y: 4.0, z: 6.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![3.0; 4], tension: vec![], open: false, spline: false });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let cyl: Vec<f64> = m.faces.iter().filter(|f| f.kind == SurfKind::Cylinder).map(|f| f.radius).collect();
        assert_eq!(cyl.len(), 4, "four quarter arcs: {cyl:?}");
        assert!(cyl.iter().all(|r| (r - 3.0).abs() < 1e-6), "{cyl:?}");
        // top, bottom, the two long sides, the four arcs
        assert_eq!(m.faces.len(), 2 + 2 + 4, "{}", m.faces.len());
        // a hole of that shape cut in a block
        let b = d.add(Op::Add, Prim::Box { x: 30.0, y: 10.0, z: 20.0 }, DVec3::new(0.0, -8.0, 0.0), DQuat::IDENTITY);
        let _ = b;
        d.features.swap(0, 1);
        d.feature_mut(c).unwrap().op = Op::Carve;
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
    }

    #[test]
    fn curves_made_solid() {
        // the Curves set's outlines as sketches: a circle (four exact arcs),
        // an ellipse (four conic quarters), a pentagon with round corners,
        // a star with round inner corners
        use crate::doc::{CurveKind, Prim};
        for (kind, x, z) in [(CurveKind::Circle, 20.0, 20.0), (CurveKind::Ellipse, 30.0, 16.0), (CurveKind::Polygon, 22.83, 21.71), (CurveKind::Star, 22.83, 21.71)] {
            let mut c = kind.new_curve();
            c.corner = if kind == CurveKind::Polygon { 2.0 } else { 0.0 };
            c.corner2 = if kind == CurveKind::Star { 1.0 } else { 0.0 };
            let mut d = Doc::default();
            let id = d.add(Op::Add, Prim::Profile { x, y: 5.0, z }, DVec3::ZERO, DQuat::IDENTITY);
            d.feature_mut(id).unwrap().profile = Some(c.profile(x, z));
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "{kind:?}: {:?}", m.errors);
            let (lo, hi) = m.positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1]), lo[2].min(p[2])], [hi[0].max(p[0]), hi[1].max(p[1]), hi[2].max(p[2])]));
            // (a polygon's round corners pull its box in)
            assert!(kind == CurveKind::Polygon || (((hi[0] - lo[0]) as f64 - x).abs() < 0.05 && ((hi[2] - lo[2]) as f64 - z).abs() < 0.05), "{kind:?}: {lo:?} {hi:?}");
            if kind == CurveKind::Circle {
                let cyl: Vec<f64> = m.faces.iter().filter(|f| f.kind == SurfKind::Cylinder).map(|f| f.radius).collect();
                assert!(cyl.len() == 4 && cyl.iter().all(|r| (r - 10.0).abs() < 1e-6), "{cyl:?}");
            }
            if kind == CurveKind::Ellipse {
                // a point of the ellipse at 45° in its own terms
                let (a, b) = (15.0f32, 8.0f32);
                let want = [a * std::f32::consts::FRAC_1_SQRT_2, b * std::f32::consts::FRAC_1_SQRT_2];
                let near = m.positions.iter().map(|p| ((p[0] - want[0]).powi(2) + (p[2] - want[1]).powi(2)).sqrt()).fold(f32::MAX, f32::min);
                assert!(near < 0.05, "no point near the ellipse at 45°: {near}");
            }
        }
    }

    /// A path of these points (in mm, on the ground round the origin).
    fn path_of(d: &mut Doc, pts: &[[f64; 2]], open: bool, spline: bool, at: DVec3) -> FeatureId {
        use crate::doc::Prim;
        let lo = pts.iter().fold([f64::MAX; 2], |a, q| [a[0].min(q[0]), a[1].min(q[1])]);
        let hi = pts.iter().fold([f64::MIN; 2], |a, q| [a[0].max(q[0]), a[1].max(q[1])]);
        let (w, dp) = ((hi[0] - lo[0]).max(0.01), (hi[1] - lo[1]).max(0.01));
        let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
        let id = d.add(Op::Add, Prim::Path { x: w, z: dp }, at + DVec3::new(c[0], 0.0, c[1]), DQuat::IDENTITY);
        let n = pts.len();
        d.feature_mut(id).unwrap().profile = Some(crate::doc::Profile { points: pts.iter().map(|q| [(q[0] - c[0]) / w, (q[1] - c[1]) / dp]).collect(), smooth: vec![true; n], corner: vec![], tension: vec![], open, spline });
        id
    }

    #[test]
    fn spline_curves() {
        // a closed curve through 5 points (one sharp) pulled into a solid,
        // and filled: its sides are the spline's exact spans
        let mut d = Doc::default();
        let pts = [[-10.0, -8.0], [12.0, -10.0], [14.0, 6.0], [0.0, 12.0], [-12.0, 4.0]];
        let id = path_of(&mut d, &pts, false, true, DVec3::ZERO);
        d.feature_mut(id).unwrap().profile.as_mut().unwrap().smooth[1] = false;
        d.feature_mut(id).unwrap().fill = true;
        assert_eq!(d.makes(d.feature(id).unwrap()), Makes::Sheet);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.len(), 1, "one flat face");
        assert_eq!(m.edges.len(), 5, "five spans");
        // the curve passes through its points
        for q in pts {
            let near = m.positions.iter().map(|p| ((p[0] as f64 - q[0]).powi(2) + (p[2] as f64 - q[1]).powi(2)).sqrt()).fold(f64::MAX, f64::min);
            assert!(near < 0.02, "the curve misses {q:?} by {near}");
        }
        // pulled up: a solid of 5 sides, a top and a bottom
        let mut e = d.clone();
        let f = e.feature_mut(id).unwrap();
        f.fill = false;
        f.prim = match f.prim { crate::doc::Prim::Path { x, z } => crate::doc::Prim::Profile { x, y: 6.0, z }, p => p };
        let m = build(&e, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.len(), 7);
        // an open curve cannot be filled
        let mut e = d.clone();
        e.feature_mut(id).unwrap().profile.as_mut().unwrap().open = true;
        let m = build(&e, true, false, &[], &mut Cache::default());
        assert_eq!(m.errors.len(), 1, "{:?}", m.errors);
    }

    #[test]
    fn revolve_curves() {
        use crate::doc::Revolve;
        // a vase's half profile, open, both ends on the axis (its left
        // side): a solid; turned half way: a solid with two flat ends
        let mut d = Doc::default();
        let id = path_of(&mut d, &[[0.0, 0.0], [8.0, 0.0], [6.0, -10.0], [9.0, -20.0], [0.0, -20.0]], true, true, DVec3::ZERO);
        d.feature_mut(id).unwrap().revolve = Some(Revolve::default());
        assert!(d.feature(id).unwrap().revolve_closes());
        assert_eq!(d.makes(d.feature(id).unwrap()), Makes::Solid);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, hi) = m.positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1]), lo[2].min(p[2])], [hi[0].max(p[0]), hi[1].max(p[1]), hi[2].max(p[2])]));
        assert!((hi[1] - lo[1] - 2.0 * (hi[0] - 0.0)).abs() < 1.0, "round about the axis: {lo:?} {hi:?}");
        assert!(lo[0] < -8.0 && hi[0] > 8.0, "{lo:?} {hi:?}");
        d.feature_mut(id).unwrap().revolve.as_mut().unwrap().angle = 180.0;
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!(m.faces.iter().filter(|f| f.kind == SurfKind::Plane).count() >= 2, "two flat ends");
        // a closed square beside its axis: a ring (a torus-like solid)
        let mut d = Doc::default();
        let r = path_of(&mut d, &[[10.0, -3.0], [16.0, -3.0], [16.0, 3.0], [10.0, 3.0]], false, false, DVec3::ZERO);
        let f = d.feature_mut(r).unwrap();
        f.profile.as_mut().unwrap().smooth = vec![false; 4];
        // the axis through the world origin along Z: the box's left side
        // is at x = 10, so put it at its middle and move the curve
        f.revolve = Some(Revolve { angle: 360.0, along_x: false, align: 0 });
        f.origin = DVec3::new(0.0, 0.0, 0.0);
        f.prim = crate::doc::Prim::Path { x: 32.0, z: 6.0 };
        f.profile.as_mut().unwrap().points = vec![[10.0 / 32.0, -0.5], [0.5, -0.5], [0.5, 0.5], [10.0 / 32.0, 0.5]];
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(d.makes(d.feature(r).unwrap()), Makes::Solid);
        // an open line beside the axis: a sheet (a lampshade)
        let mut d = Doc::default();
        let l = path_of(&mut d, &[[6.0, 0.0], [10.0, -12.0]], true, false, DVec3::ZERO);
        let f = d.feature_mut(l).unwrap();
        f.revolve = Some(Revolve { angle: 360.0, along_x: false, align: 0 });
        f.origin = DVec3::new(0.0, 0.0, -6.0);
        f.prim = crate::doc::Prim::Path { x: 20.0, z: 12.0 };
        f.profile.as_mut().unwrap().points = vec![[0.3, 0.5], [0.5, -0.5]];
        assert_eq!(d.makes(d.feature(l).unwrap()), Makes::Sheet);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!(!m.faces.is_empty());
        // in the chain: a revolved ring carved out of a block
        let mut d = Doc::default();
        d.add(Op::Add, crate::doc::Prim::Box { x: 40.0, y: 10.0, z: 40.0 }, DVec3::new(0.0, -5.0, 0.0), DQuat::IDENTITY);
        let v = path_of(&mut d, &[[0.0, 0.0], [8.0, 0.0], [8.0, -6.0], [0.0, -6.0]], true, false, DVec3::new(0.0, 5.0, 3.0));
        let f = d.feature_mut(v).unwrap();
        f.profile.as_mut().unwrap().smooth = vec![false; 4];
        f.revolve = Some(Revolve::default());
        f.op = Op::Carve;
        f.rot = DQuat::from_rotation_x(std::f64::consts::FRAC_PI_2);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!(m.faces.iter().any(|f| f.tag.map(|t| t.feature) == Some(v)), "the hole's faces carry the revolve's tag");
    }

    #[test]
    fn loft_curves() {
        use crate::doc::{CurveKind, Loft, Prim};
        // a square at the bottom, a circle 20 up, a smaller square 35 up:
        // a solid through them; ruled; three open lines: a sheet
        let mut d = Doc::default();
        let a = d.add(Op::Add, Prim::Path { x: 20.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(a).unwrap().curve = Some(CurveKind::Rectangle.new_curve());
        let b = d.add(Op::Add, Prim::Path { x: 14.0, z: 14.0 }, DVec3::new(0.0, 20.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(b).unwrap().curve = Some(CurveKind::Circle.new_curve());
        let c = d.add(Op::Add, Prim::Path { x: 8.0, z: 8.0 }, DVec3::new(2.0, 35.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(c).unwrap().curve = Some(CurveKind::Polygon.new_curve());
        d.regen_curves();
        let l = d.add(Op::Add, Prim::Path { x: 1.0, z: 1.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(l).unwrap().loft = Some(Loft { sections: vec![a, b, c], ruled: false });
        d.regen_curves();
        let f = d.feature(l).unwrap();
        assert!(!f.is_path());
        assert!((f.origin.y - 17.5).abs() < 1e-6, "its middle half-way up: {:?}", f.origin);
        assert_eq!(d.makes(f), Makes::Solid);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, hi) = m.positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1]), lo[2].min(p[2])], [hi[0].max(p[0]), hi[1].max(p[1]), hi[2].max(p[2])]));
        assert!(lo[1].abs() < 0.01 && (hi[1] - 35.0).abs() < 0.01, "{lo:?} {hi:?}");
        d.feature_mut(l).unwrap().loft.as_mut().unwrap().ruled = true;
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "ruled: {:?}", m.errors);
        // its curves moved with it
        assert_eq!(d.with_operands(&[l]).len(), 4);
        // a curve taken out: the loft goes on with two; then with one, gone
        let mut e = d.clone();
        e.remove(&[c]);
        assert_eq!(e.feature(l).map(|f| f.loft.as_ref().unwrap().sections.len()), Some(2));
        e.remove(&[b]);
        assert!(e.feature(l).is_none(), "a loft of one curve goes");
        // deleting the loft leaves its curves
        let mut e = d.clone();
        e.remove(&[l]);
        assert!(e.feature(a).is_some() && e.feature(b).is_some());
        // open lines: a sheet
        let mut d = Doc::default();
        let s1 = path_of(&mut d, &[[-10.0, 0.0], [0.0, -4.0], [10.0, 0.0]], true, true, DVec3::ZERO);
        let s2 = path_of(&mut d, &[[-10.0, 0.0], [0.0, 4.0], [10.0, 0.0]], true, true, DVec3::new(0.0, 15.0, 0.0));
        let l = d.add(Op::Add, Prim::Path { x: 1.0, z: 1.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(l).unwrap().loft = Some(Loft { sections: vec![s1, s2], ruled: false });
        d.regen_curves();
        assert_eq!(d.makes(d.feature(l).unwrap()), Makes::Sheet);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.len(), 2, "one face per span");
    }

    #[test]
    fn forms() {
        use crate::doc::{Form, Prim};
        // a sphere, a cylinder and a soft box made from their poles; a
        // pole ring pulled out; a form added to a block and carved
        for (name, fm) in [("sphere", Form::sphere(24, 13)), ("cylinder", Form::cylinder(24, false)), ("box", Form::cylinder(24, true))] {
            let mut d = Doc::default();
            let id = d.add(Op::Add, Prim::Box { x: 30.0, y: 30.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            d.feature_mut(id).unwrap().form = Some(fm.clone());
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "{name}: {:?}", m.errors);
            let (lo, hi) = m.positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1]), lo[2].min(p[2])], [hi[0].max(p[0]), hi[1].max(p[1]), hi[2].max(p[2])]));
            eprintln!("{name}: {lo:?} {hi:?} faces {}", m.faces.len());
            assert!((hi[1] - lo[1] - 30.0).abs() < 0.6 && (hi[0] - lo[0] - 30.0).abs() < 1.5, "{name}: {lo:?} {hi:?}");
            // pulled: the poles of a ring half-way up moved out on one side
            let f = d.feature(id).unwrap().clone();
            let mut w = f.form_poles();
            let j = fm.nv / 2;
            for i in 0..3 {
                w[j * fm.nu + i] += DVec3::new(8.0, 0.0, 0.0);
            }
            let mut g = f.clone();
            g.set_form_poles(&w);
            *d.feature_mut(id).unwrap() = g;
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "{name} pulled: {:?}", m.errors);
            let hx = m.positions.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
            assert!(hx > 17.0, "{name}: pulled out to {hx}");
            // round the block
            let b = d.add(Op::Add, Prim::Box { x: 20.0, y: 10.0, z: 20.0 }, DVec3::new(20.0, 5.0, 0.0), DQuat::IDENTITY);
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "{name} + block: {:?}", m.errors);
            d.feature_mut(b).unwrap().op = Op::Carve;
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "{name} − block: {:?}", m.errors);
        }
    }

    #[test]
    fn form_basis_matches_the_kernel() {
        // the app's evaluation of a form (its preview while a brush pulls
        // it) is the kernel's surface, refined too
        use crate::doc::Form;
        let mut fm = Form::sphere(12, 9);
        fm.poles[4 * 12 + 2][0] += 0.4;
        fm.poles[3 * 12 + 7][1] += 0.2;
        for f in [fm.clone(), fm.refined()] {
            let s = Shape::form(&f.poles, f.nu, f.nv).unwrap();
            let (ru, rv) = f.range();
            let uv: Vec<[f64; 2]> = (0..40).map(|k| [ru * ((k * 7) % 40) as f64 / 40.0 + 0.13, rv * (k as f64 + 0.5) / 40.0]).collect();
            let occ = s.face_eval(0, &uv).unwrap();
            let unit: Vec<DVec3> = f.poles.iter().map(|p| DVec3::from(*p)).collect();
            let mut worst = 0.0f64;
            for (q, (p, _)) in uv.iter().zip(&occ) {
                let a = Form::point(&unit, f.nu, f.nv, q[0], q[1]);
                worst = worst.max(a.distance(DVec3::from(*p)));
            }
            assert!(worst < 1e-6, "{} × {}: off by {worst}", f.nu, f.nv);
        }
        // refined: the same surface
        let r = fm.refined();
        let a: Vec<DVec3> = fm.poles.iter().map(|p| DVec3::from(*p)).collect();
        let b: Vec<DVec3> = r.poles.iter().map(|p| DVec3::from(*p)).collect();
        for k in 0..30 {
            let (u, v) = (12.0 * k as f64 / 30.0 + 0.07, 6.0 * (k as f64 + 0.3) / 30.0);
            let d = Form::point(&a, fm.nu, fm.nv, u, v).distance(Form::point(&b, r.nu, r.nv, u * 2.0, v * 2.0));
            assert!(d < 1e-9, "refined off by {d} at {u} {v}");
        }
    }

    #[test]
    fn curves_extruded() {
        // step 3: a 30 × 20 rectangle less a circle of radius 6 on its right
        // side, pulled 10 up; the same pulled 4 down into a block (Carve);
        // a star pulled on its own
        use crate::doc::{CurveKind, CurveOp, Prim, ShapeBool};
        let mut d = Doc::default();
        let curve = |d: &mut Doc, kind: CurveKind, x: f64, z: f64, at: DVec3| {
            let id = d.add(Op::Add, Prim::Path { x, z }, at, DQuat::IDENTITY);
            d.feature_mut(id).unwrap().curve = Some(kind.new_curve());
            id
        };
        let r = curve(&mut d, CurveKind::Rectangle, 30.0, 20.0, DVec3::ZERO);
        let c = curve(&mut d, CurveKind::Circle, 12.0, 12.0, DVec3::new(15.0, 0.0, 0.0));
        let b = d.add(Op::Add, Prim::Profile { x: 1.0, y: 10.0, z: 1.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(b).unwrap().shape_bool = Some(ShapeBool { op: CurveOp::Subtract, operands: vec![r, c], corners: vec![] });
        d.regen_curves();
        let f = d.feature(b).unwrap();
        assert!(f.is_extruded_curve() && !f.is_path());
        assert_eq!(f.prim, Prim::Profile { x: 36.0, y: 10.0, z: 20.0 }, "its box: its curves'");
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (lo, hi) = m.bounds().unwrap();
        assert!((lo[0] + 15.0).abs() < 0.01 && (hi[0] - 15.0).abs() < 0.01 && lo[1].abs() < 0.01 && (hi[1] - 10.0).abs() < 0.01, "{lo:?} {hi:?}");
        // its notch's wall is tagged by the circle (the second leaf)
        assert!(m.faces.iter().any(|x| x.tag.is_some_and(|t| t.feature == b && t.sub >= 1003)), "{:?}", m.faces.iter().map(|x| x.tag).collect::<Vec<_>>());
        assert!(m.faces.iter().any(|x| x.tag.is_some_and(|t| t.feature == b && t.sub == 1)) && m.faces.iter().any(|x| x.tag.is_some_and(|t| t.feature == b && t.sub == 2)));
        // the circle grown: the solid follows
        let mut e = d.clone();
        if let Some(Prim::Path { x, z }) = e.feature_mut(c).map(|g| &mut g.prim) {
            *x = 20.0;
            *z = 20.0;
        }
        e.regen_curves();
        let m2 = build(&e, true, false, &[], &mut Cache::default());
        assert!(m2.errors.is_empty(), "{:?}", m2.errors);
        assert!(m2.positions.len() != m.positions.len() || m2.faces.len() != m.faces.len() || m2.bounds() != m.bounds() || m2.edges.len() != m.edges.len());
        // down into a block, as a Carve: turned over round its Z
        let mut e = d.clone();
        let blk = e.add(Op::Add, Prim::Box { x: 40.0, y: 10.0, z: 30.0 }, DVec3::new(0.0, -10.0, 0.0), DQuat::IDENTITY);
        let pos = e.features.iter().position(|g| g.id == blk).unwrap();
        let g = e.features.remove(pos);
        e.features.insert(0, g);
        {
            let f = e.feature_mut(b).unwrap();
            f.op = Op::Carve;
            f.rot = DQuat::from_rotation_z(std::f64::consts::PI);
            f.prim = Prim::Profile { x: 1.0, y: 4.0, z: 1.0 };
        }
        e.regen_curves();
        assert!(e.feature(b).unwrap().axis().y < -0.99, "still down after regen");
        let m3 = build(&e, true, false, &[], &mut Cache::default());
        assert!(m3.errors.is_empty(), "{:?}", m3.errors);
        // the pocket's floor at y = −4
        assert!(m3.faces.iter().any(|x| x.tag.is_some_and(|t| t.feature == b) && (x.point[1] + 4.0).abs() < 0.01), "no floor at -4");
        // a star curve pulled on its own: a Profile with its curve
        let mut e = Doc::default();
        let st = curve(&mut e, CurveKind::Star, 20.0, 20.0, DVec3::ZERO);
        e.feature_mut(st).unwrap().prim = Prim::Profile { x: 20.0, y: 5.0, z: 20.0 };
        e.regen_curves();
        let m4 = build(&e, true, false, &[], &mut Cache::default());
        assert!(m4.errors.is_empty() && m4.faces.len() >= 12, "{:?} {}", m4.errors, m4.faces.len());
    }

    #[test]
    fn curves_combined() {
        // a 30 × 20 rectangle less a circle of radius 6 on its right side
        // (a notch), with a circle inside (a hole), then their union and
        // their intersection
        use crate::doc::{CurveKind, CurveOp, Prim, ShapeBool};
        let mut d = Doc::default();
        let curve = |d: &mut Doc, kind: CurveKind, x: f64, z: f64, at: DVec3| {
            let id = d.add(Op::Add, Prim::Path { x, z }, at, DQuat::IDENTITY);
            d.feature_mut(id).unwrap().curve = Some(kind.new_curve());
            id
        };
        let r = curve(&mut d, CurveKind::Rectangle, 30.0, 20.0, DVec3::ZERO);
        let c = curve(&mut d, CurveKind::Circle, 12.0, 12.0, DVec3::new(15.0, 0.0, 0.0));
        let h = curve(&mut d, CurveKind::Circle, 6.0, 6.0, DVec3::new(-6.0, 0.0, 0.0));
        d.regen_curves();
        for (op, operands, edges) in [(CurveOp::Subtract, vec![r, c, h], 7), (CurveOp::Union, vec![r, c], 5), (CurveOp::Intersect, vec![r, c], 2)] {
            let mut e = d.clone();
            let b = e.add(Op::Add, Prim::Path { x: 30.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
            e.feature_mut(b).unwrap().shape_bool = Some(ShapeBool { op, operands, corners: vec![] });
            let m = build(&e, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "{op:?}: {:?}", m.errors);
            let lines = &m.curves.iter().find(|c| c.0 == b).expect("its edges").1;
            assert!(lines.len() >= edges, "{op:?}: {} edges", lines.len());
            assert!(lines.iter().flatten().all(|p| p[1].abs() < 1e-4), "{op:?}: off the ground");
            let xs = lines.iter().flatten().map(|p| p[0]);
            let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
            let want = match op { CurveOp::Subtract => (-15.0, 15.0), CurveOp::Union => (-15.0, 21.0), CurveOp::Intersect => (9.0, 15.0) };
            assert!((lo - want.0).abs() < 0.01 && (hi - want.1).abs() < 0.01, "{op:?}: x from {lo} to {hi}");
        }
        // not on one plane: said
        let mut e = d.clone();
        e.feature_mut(c).unwrap().origin.y = 5.0;
        let b = e.add(Op::Add, Prim::Path { x: 30.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        e.feature_mut(b).unwrap().shape_bool = Some(ShapeBool { op: CurveOp::Union, operands: vec![r, c], corners: vec![] });
        let m = build(&e, true, false, &[], &mut Cache::default());
        assert!(m.errors.iter().any(|x| x.contains("plane")), "{:?}", m.errors);
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
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![5.0; 4], tension: vec![], open: false, spline: false });
        tension_case("rounded square", d, DVec3::new(0.0, 10.0, 10.0), 2.0);
    }

    #[test]
    fn tension_oblique_ends() {
        // a trapezoid prism: the fillet on its long top edge ends on the
        // slanted sides
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Profile { x: 30.0, y: 10.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.25, 0.5], [-0.25, 0.5]], smooth: vec![false; 4], corner: vec![], tension: vec![], open: false, spline: false });
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
    fn tension_point_corner() {
        // the top front edge filleted (2 mm), then the top -X edge at 3 mm
        // with tension: OpenCascade carries the second round the first one's
        // end and down the front vertical edge, its corner there a
        // three-sided face closing on a point of the -X side (the end
        // curved tighter than the fillet). That corner is made conic too,
        // its ends the two cylinders' new sections (a little off a plane:
        // taken on the plane through their ends and middle)
        let round = |m: &Model, id: u32| m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(id) && matches!(f.kind, SurfKind::Cylinder | SurfKind::Torus)).count();
        for (r1, t1, r2, t2) in [(2.0, 0.0, 3.0, 0.7), (2.0, 0.0, 3.0, 0.3), (2.0, 0.5, 3.0, 0.7), (1.0, 0.0, 4.0, 0.3), (1.5, 0.4, 3.0, 0.6)] {
            let mut d = Doc::default();
            let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            let a = d.add_round(r1, false, vec![EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(0.0, 20.0, 15.0) }]);
            d.feature_mut(a).unwrap().round.as_mut().unwrap().tension = t1;
            let c = d.add_round(r2, false, vec![EdgeRef { faces: [tag(b, 2), tag(b, 1)], at: DVec3::new(-20.0, 20.0, 0.0) }]);
            d.feature_mut(c).unwrap().round.as_mut().unwrap().tension = t2;
            let m = build(&d, true, false, &[], &mut Cache::default());
            let what = format!("{r1} mm ({t1}) then {r2} mm ({t2})");
            assert!(m.errors.is_empty(), "{what}: {:?}", m.errors);
            assert!(m.limits.is_empty() && m.tension_limits.is_empty(), "{what}: {:?} {:?}", m.limits, m.tension_limits);
            // (not blended with the first: the second fillet's three faces, all conic)
            assert_eq!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(c)).count(), 3, "{what}");
            assert_eq!(round(&m, c), 0, "{what}");
            if t1 > 0.0 {
                assert_eq!(round(&m, a), 0, "{what}");
            }
        }
    }

    #[test]
    fn corner_tension() {
        // a 20 mm square sketch, one corner a 6 mm fillet: circular, then
        // with tension 0.7 (fuller) and 0.3 (flatter)
        let mut d = Doc::default();
        let c = d.add(Op::Add, Prim::Profile { x: 20.0, y: 6.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let mut pr = crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![0.0, 6.0, 0.0, 0.0], tension: vec![], open: false, spline: false };
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
    fn surfaces_beside_the_part() {
        // a block, a flat surface above it and a bent one (its outline off
        // a plane, a point inside pulling it up): both kept beside the part
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        let flat = d.add(Op::Add, Prim::Path { x: 20.0, z: 20.0 }, DVec3::new(0.0, 30.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(flat).unwrap().sheet = Some(crate::doc::Sheet { pts: vec![DVec3::new(-10.0, 0.0, -10.0), DVec3::new(10.0, 0.0, -10.0), DVec3::new(10.0, 0.0, 10.0), DVec3::new(-5.0, 0.0, 12.0), DVec3::new(-10.0, 0.0, 0.0)], inner: vec![], grid: None });
        let bent = d.add(Op::Add, Prim::Path { x: 20.0, z: 20.0 }, DVec3::new(40.0, 30.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(bent).unwrap().sheet = Some(crate::doc::Sheet {
            pts: vec![DVec3::new(-10.0, 0.0, -10.0), DVec3::new(10.0, 2.0, -10.0), DVec3::new(10.0, 0.0, 10.0), DVec3::new(-10.0, 2.0, 10.0)],
            inner: vec![DVec3::new(0.0, 5.0, 0.0)],
            grid: None,
        });
        let t = std::time::Instant::now();
        let m = build(&d, true, false, &[], &mut Cache::default());
        eprintln!("{:.0} ms", t.elapsed().as_secs_f64() * 1e3);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.len(), m0.faces.len() + 2);
        for id in [flat, bent] {
            assert!(m.faces.iter().any(|f| f.tag.map(|t| t.feature) == Some(id)), "{id}");
        }
        // the bent one passes near its inner point
        let top = DVec3::new(40.0, 35.0, 0.0);
        let near = m.positions.iter().map(|p| DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64).distance(top)).fold(f64::MAX, f64::min);
        assert!(near < 1.0, "{near}");
    }

    #[test]
    #[ignore]
    fn sheet_probe() {
        // FORME_SHEET_DOC = a document saved by the harness (surface-scan)
        let Ok(p) = std::env::var("FORME_SHEET_DOC") else { return };
        let d: Doc = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        let t = std::time::Instant::now();
        let m = build(&d, true, false, &[], &mut Cache::default());
        eprintln!("{:.0} ms, {} faces, {} triangles, errors {:?}", t.elapsed().as_secs_f64() * 1e3, m.faces.len(), m.triangles.len(), m.errors);
    }

    #[test]
    fn surface_l_outlines() {
        // an L on its plane (a flat 4 × 4 grid), then the same L's 60
        // points of a curve, then on a bump: each one face
        let mut d = Doc::default();
        let l = [[-20.0, -20.0], [20.0, -20.0], [20.0, -10.0], [-10.0, -10.0], [-10.0, 20.0], [-20.0, 20.0]];
        let flat: Vec<DVec3> = (0..16).map(|k| DVec3::new(-21.0 + 42.0 * (k / 4) as f64 / 3.0, if std::env::var("FORME_BUMP").is_ok() { 0.01 * (k % 3) as f64 } else { 0.0 }, -21.0 + 42.0 * (k % 4) as f64 / 3.0)).collect();
        let a = d.add(Op::Add, Prim::Path { x: 40.0, z: 40.0 }, DVec3::new(0.0, 5.0, 0.0), DQuat::from_rotation_z(0.3));
        d.feature_mut(a).unwrap().sheet = Some(crate::doc::Sheet { pts: l.iter().map(|p| DVec3::new(p[0], 0.0, p[1])).collect(), inner: flat.clone(), grid: Some([4, 4]) });
        // many points along the L's sides, some repeated, clockwise
        let mut many = vec![];
        for i in (0..6).rev() {
            let (p, q) = (l[i], l[(i + 5) % 6]);
            for k in 0..10 {
                let t = k as f64 / 10.0;
                many.push(DVec3::new(p[0] + (q[0] - p[0]) * t, 0.0, p[1] + (q[1] - p[1]) * t));
            }
            many.push(DVec3::new(q[0], 0.0, q[1]));
        }
        let b = d.add(Op::Add, Prim::Path { x: 40.0, z: 40.0 }, DVec3::new(60.0, 5.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(b).unwrap().sheet = Some(crate::doc::Sheet { pts: many, inner: flat, grid: Some([4, 4]) });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        for id in [a, b] {
            assert_eq!(m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(id)).count(), 1, "{id}");
        }
        for id in [a, b] {
            let n = m.tri_face.iter().filter(|f| m.faces[**f as usize].tag.map(|t| t.feature) == Some(id)).count();
            eprintln!("feature {id}: {n} triangles");
        }
        // its area: an L of 40 × 40 less 30 × 30
        let area: f64 = m.triangles.iter().zip(&m.tri_face).filter(|(_, f)| m.faces[**f as usize].tag.map(|t| t.feature) == Some(a)).map(|(t, _)| {
            let p = |i: u32| DVec3::new(m.positions[i as usize][0] as f64, m.positions[i as usize][1] as f64, m.positions[i as usize][2] as f64);
            0.5 * (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).length()
        }).sum();
        assert!((area - 700.0).abs() < 5.0, "{area}");
    }

    #[test]
    fn surface_on_a_height_field() {
        // a surface laid on a bump (heights on a 20 × 20 grid), its outline
        // a pentagon, its frame turned: one face, its points on the bump
        let mut d = Doc::default();
        let rot = DQuat::from_rotation_x(0.4);
        let id = d.add(Op::Add, Prim::Path { x: 40.0, z: 40.0 }, DVec3::new(5.0, 10.0, 0.0), rot);
        let hf = |x: f64, z: f64| 6.0 * (-(x * x + z * z) / 300.0).exp();
        let (nx, nz) = (20, 20);
        let mut inner = vec![];
        for i in 0..nx {
            for k in 0..nz {
                let (x, z) = (-22.0 + 44.0 * i as f64 / 19.0, -22.0 + 44.0 * k as f64 / 19.0);
                inner.push(DVec3::new(x, hf(x, z), z));
            }
        }
        let pts: Vec<DVec3> = (0..5).map(|k| {
            let a = k as f64 * std::f64::consts::TAU / 5.0;
            let (x, z) = (18.0 * a.cos(), 18.0 * a.sin());
            DVec3::new(x, hf(x, z), z)
        }).collect();
        d.feature_mut(id).unwrap().sheet = Some(crate::doc::Sheet { pts, inner, grid: Some([nx, nz]) });
        let t = std::time::Instant::now();
        let m = build(&d, true, false, &[], &mut Cache::default());
        eprintln!("{:.0} ms, {} triangles", t.elapsed().as_secs_f64() * 1e3, m.triangles.len());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.len(), 1);
        assert!(m.triangles.len() > 20);
        let f = d.feature(id).unwrap();
        let inv = f.matrix().inverse();
        let worst = m.positions.iter().map(|p| {
            let l = inv.transform_point3(DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64));
            (l.y - hf(l.x, l.z)).abs()
        }).fold(0.0, f64::max);
        assert!(worst < 0.1, "{worst}");
    }

    #[test]
    fn paths_have_no_solid() {
        // a block and an open path (an L of 3 points, its middle corner
        // rounded at 5 mm) over it: the path changes nothing in the part
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        let p = d.add(Op::Add, Prim::Path { x: 20.0, z: 20.0 }, DVec3::new(0.0, 20.0, 0.0), DQuat::IDENTITY);
        let pr = crate::doc::Profile { points: vec![[-0.5, 0.5], [-0.5, -0.5], [0.5, -0.5]], smooth: vec![false; 3], corner: vec![0.0, 5.0, 0.0], tension: vec![], open: true, spline: false };
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
        let k = d.add_knife(Knife { pts: vec![front.clone()], segs: vec![], remove_faces: vec![], remove_edges: vec![] }, "Points");
        let m1 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m1.errors.is_empty(), "{:?}", m1.errors);
        assert_eq!(m1.faces.len(), m0.faces.len());
        assert_eq!(m1.edges.len(), m0.edges.len() + 1, "the edge cut in two");
        d.feature_mut(k).unwrap().cut = Some(Knife { pts: vec![front, back], segs: vec![[0, 1]], remove_faces: vec![], remove_edges: vec![] });
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
            d.add_knife(Knife { pts: vec![a, c], segs: vec![[0, 1]], remove_faces: vec![], remove_edges: vec![] }, "Knife");
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
                d.add_knife(Knife { pts, segs: vec![[0, 1], [2, 3]], remove_faces: vec![], remove_edges: vec![] }, "Knife");
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
            Knife { pts: vec![kp([tag(b, 2), tag(r, 0)], DVec3::new(-5.0, 20.0, 11.0), 0.375), kp([tag(b, 4), tag(r, 0)], DVec3::new(5.0, 16.0, 15.0), 0.625)], segs: vec![[0, 1]], remove_faces: vec![], remove_edges: vec![] },
            "Knife",
        );
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.len(), m0.faces.len() + 1);
    }

    #[test]
    #[ignore]
    fn doc_probe() {
        // FORME_DOC=file.fnurbs: its errors, faces and the edges between two
        // pieces of one face (a document from the user's machine)
        let Ok(p) = std::env::var("FORME_DOC") else { return };
        let d = Doc::from_json(&std::fs::read_to_string(p).unwrap()).unwrap();
        let n = std::env::var("FORME_DOC_UPTO").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(d.features.len());
        let mut d2 = d.clone();
        d2.features.truncate(n);
        let m = build(&d2, true, false, &[], &mut Cache::default());
        eprintln!("errors {:?}, {} faces, {} edges", m.errors, m.faces.len(), m.edges.len());
        for (i, f) in m.faces.iter().enumerate() {
            eprintln!("face {i}: {:?} {:?}", f.tag.map(|t| (t.feature, t.sub)), f.kind);
        }
        for (i, e) in m.edges.iter().enumerate() {
            let a = e.pts.first().unwrap();
            let b = e.pts.last().unwrap();
            eprintln!("edge {i}: faces {:?} seam {} {:?} -> {:?}", e.faces, e.seam, a, b);
        }
    }

    /// One model's errors (sorted), faces, edges, volume and bounds.
    fn digest(m: &Model) -> (Vec<String>, usize, usize, f64, Option<([f32; 3], [f32; 3])>) {
        let mut e = m.errors.clone();
        e.sort();
        (e, m.faces.len(), m.edges.len(), mesh_volume(m), m.bounds())
    }

    /// The document built island by island and as one history: the same
    /// part (errors, faces, edges, volume, bounds). The differences, if any.
    fn islands_against_one(d: &Doc) -> Vec<String> {
        let a = build(d, true, false, &[], &mut Cache::default());
        ONE_CHAIN.with(|c| c.set(true));
        let b = build(d, true, false, &[], &mut Cache::default());
        ONE_CHAIN.with(|c| c.set(false));
        let (da, db) = (digest(&a), digest(&b));
        let mut out = vec![];
        if da.0 != db.0 {
            out.push(format!("errors: islands {:?}, one history {:?}", da.0.iter().filter(|e| !db.0.contains(e)).collect::<Vec<_>>(), db.0.iter().filter(|e| !da.0.contains(e)).collect::<Vec<_>>()));
        }
        if (da.1, da.2) != (db.1, db.2) {
            out.push(format!("faces {} / {}, edges {} / {}", da.1, db.1, da.2, db.2));
        }
        if (da.3 - db.3).abs() > 1e-3 * db.3.abs().max(1.0) {
            out.push(format!("volume {:.3} / {:.3}", da.3, db.3));
        }
        if let (Some(x), Some(y)) = (da.4, db.4) {
            if (0..3).any(|i| (x.0[i] - y.0[i]).abs() > 0.01 || (x.1[i] - y.1[i]).abs() > 0.01) {
                out.push(format!("bounds {x:?} / {y:?}"));
            }
        } else if da.4.is_some() != db.4.is_some() {
            out.push("one of them is empty".into());
        }
        out
    }

    /// Found by the islands battery with a Mirror (21 nov.). An inset left
    /// as a line was taken away by the next shape added anywhere and by a
    /// Mirror (its inside and the face round it lie on one surface). A flat
    /// face moved along its normal past a hole failed as soon as the part
    /// held another object beside it (the union had to give one solid).
    #[test]
    fn inset_line_kept_and_face_moved_beside_another_object() {
        // a block, its front inset 2 mm as a line
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::new(100.0, 0.0, 0.0), DQuat::IDENTITY);
        let i = d.add_push(Push { face: FaceRef { tag: tag(b, 4), at: DVec3::new(100.0, 10.0, 15.0) }, inset: 2.0, depth: 0.0 });
        // its inside: 36 × 16, the ring round it still the block's front
        let line = |m: &Model| {
            let inner: Vec<usize> = (0..m.faces.len()).filter(|k| m.faces[*k].tag == Some(FaceTag { feature: i, sub: 1 })).collect();
            let ring = m.faces.iter().filter(|f| f.tag == Some(FaceTag { feature: b, sub: 4 })).count();
            let wide: Vec<f32> = inner
                .iter()
                .map(|k| {
                    let xs: Vec<f32> = m.triangles.iter().enumerate().filter(|(t, _)| m.tri_face[*t] as usize == *k).flat_map(|(_, tri)| tri.iter().map(|v| m.positions[*v as usize][0])).collect();
                    xs.iter().cloned().fold(f32::MIN, f32::max) - xs.iter().cloned().fold(f32::MAX, f32::min)
                })
                .collect();
            (inner.len(), ring, wide)
        };
        for one in [false, true] {
            ONE_CHAIN.with(|c| c.set(one));
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert_eq!(line(&m), (1, 1, vec![36.0]), "{:?}", m.errors);
            // a block added apart: the line stays
            let mut d2 = d.clone();
            d2.add(Op::Add, Prim::Box { x: 10.0, y: 10.0, z: 10.0 }, DVec3::ZERO, DQuat::IDENTITY);
            let m = build(&d2, true, false, &[], &mut Cache::default());
            assert_eq!(line(&m), (1, 1, vec![36.0]), "one history {one}: {:?}", m.errors);
            // a Mirror: the line on both sides
            d2.add_mirror(crate::doc::Mirror { axis: 0, offset: 40.0, flip: false, single: false });
            let m = build(&d2, true, false, &[], &mut Cache::default());
            assert_eq!(line(&m), (2, 2, vec![36.0, 36.0]), "one history {one}: {:?}", m.errors);
            // sunk 1 mm after all: a pocket, as before
            let mut d3 = d2.clone();
            d3.feature_mut(i).unwrap().push.as_mut().unwrap().depth = -1.0;
            let m = build(&d3, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "{:?}", m.errors);
            assert_eq!(m.faces.iter().filter(|f| f.tag.is_some_and(|t| t.feature == i)).count(), 10);
        }
        // the test part (its top left of the slot has the hole in it) and a
        // block beside it: the top moved up 3 mm, in one history (the part
        // is two lumps) and by islands
        let (mut d, b, _) = test_part();
        let v0 = {
            let mut alone = d.clone();
            alone.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(b, 2), at: DVec3::new(-12.0, 20.0, 8.0) }), delta: DVec3::new(0.0, 3.0, 0.0), more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
            let m = build(&alone, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "{:?}", m.errors);
            mesh_volume(&m)
        };
        d.add(Op::Add, Prim::Box { x: 10.0, y: 10.0, z: 10.0 }, DVec3::new(100.0, 0.0, 0.0), DQuat::IDENTITY);
        d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(b, 2), at: DVec3::new(-12.0, 20.0, 8.0) }), delta: DVec3::new(0.0, 3.0, 0.0), more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
        for one in [false, true] {
            ONE_CHAIN.with(|c| c.set(one));
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "one history {one}: {:?}", m.errors);
            assert!((mesh_volume(&m) - v0 - 1000.0).abs() < 1.0, "{} / {}", mesh_volume(&m), v0 + 1000.0);
        }
        ONE_CHAIN.with(|c| c.set(false));
    }

    #[test]
    fn islands_live() {
        use crate::doc::Mirror;
        // (the suite run with one history: nothing of this applies)
        if std::env::var_os("FORME_ONE_CHAIN").is_some() {
            return;
        }
        // two blocks 40 mm apart, each with a hole and a fillet on a top
        // edge, the steps of one between the other's; a third block apart
        let mut d = Doc::default();
        let a = d.add(Op::Add, Prim::Box { x: 20.0, y: 10.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let b = d.add(Op::Add, Prim::Box { x: 20.0, y: 10.0, z: 20.0 }, DVec3::new(60.0, 0.0, 0.0), DQuat::IDENTITY);
        let ha = d.add(Op::Carve, Prim::Cylinder { radius: 3.0, height: 20.0 }, DVec3::new(0.0, -5.0, 0.0), DQuat::IDENTITY);
        let hb = d.add(Op::Carve, Prim::Cylinder { radius: 3.0, height: 20.0 }, DVec3::new(60.0, -5.0, 0.0), DQuat::IDENTITY);
        d.add(Op::Add, Prim::Box { x: 20.0, y: 10.0, z: 20.0 }, DVec3::new(0.0, 0.0, 60.0), DQuat::IDENTITY);
        let m = build(&d, true, false, &[], &mut Cache::default());
        let fa = d.add_round(2.0, false, vec![edge_near(&m, DVec3::new(0.0, 10.0, 10.0))]);
        let fb = d.add_round(2.0, false, vec![edge_near(&m, DVec3::new(60.0, 10.0, 10.0))]);
        let same = |d: &Doc| assert_eq!(islands_against_one(d), Vec::<String>::new());
        same(&d);
        // one block's hole moved: the other blocks' steps are kept (in one
        // history the second hole is cut again)
        let made = |d: &Doc, one: bool, id: FeatureId| {
            ONE_CHAIN.with(|c| c.set(one));
            let mut cache = Cache::default();
            let m = build(d, true, false, &[], &mut cache);
            assert!(m.errors.is_empty(), "{:?}", m.errors);
            let mut e = d.clone();
            e.feature_mut(id).unwrap().origin.x += 2.0;
            let m = build(&e, true, false, &[], &mut cache);
            ONE_CHAIN.with(|c| c.set(false));
            assert!(m.errors.is_empty(), "{:?}", m.errors);
            (m.booleans, m.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(fb)).count())
        };
        let (isl, one) = (made(&d, false, ha), made(&d, true, ha));
        // (its cut and its fillet)
        assert_eq!((isl.0, isl.1), (2, 1), "{isl:?}");
        assert!(one.0 > isl.0, "{one:?} {isl:?}");
        let _ = (hb, fa);
        // a corner of the third block moved up: found on its island
        d.add_tweak(Tweak { comp: Comp::Vertex(DVec3::new(10.0, 10.0, 70.0)), delta: DVec3::new(0.0, 4.0, 0.0), more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!((m.bounds().unwrap().1[1] - 14.0).abs() < 1e-3);
        same(&d);
        // the first block's +X face pulled out into the second: they are one
        // lump, as in one history (found by their boxes once built)
        let mut e = d.clone();
        e.add_push(Push { face: FaceRef { tag: tag(a, 0), at: DVec3::new(10.0, 4.0, 0.0) }, inset: 0.0, depth: 45.0 });
        let mut cache = Cache::default();
        let m = build(&e, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!(!cache.hints.is_empty());
        same(&e);
        // a Mirror across X: the first and third blocks lie across it, the
        // second gets its image
        let mi = d.add_mirror(Mirror { axis: 0, offset: 0.0, flip: false, single: false });
        let m1 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m1.errors.is_empty(), "{:?}", m1.errors);
        assert!((m1.bounds().unwrap().0[0] + 70.0).abs() < 1e-3);
        same(&d);
        // a hole made after it in the second block: in its image too
        let h2 = d.add(Op::Carve, Prim::Cylinder { radius: 1.5, height: 20.0 }, DVec3::new(66.0, -5.0, 6.0), DQuat::IDENTITY);
        let m2 = build(&d, true, false, &[], &mut Cache::default());
        assert_eq!(m2.faces.iter().filter(|f| f.tag.map(|t| t.feature) == Some(h2)).count(), 2);
        same(&d);
        // the same Mirror again: nothing to make, the same part
        let mut cache = Cache::default();
        build(&d, true, false, &[], &mut cache);
        let mut e = d.clone();
        e.add_mirror(Mirror { axis: 0, offset: 0.0, flip: false, single: false });
        let m3 = build(&e, true, false, &[], &mut cache);
        assert_eq!(m3.booleans, 0);
        assert_eq!(digest(&m3), digest(&m2));
        same(&e);
        // flipped: the second block is all on the side dropped, gone (the
        // hole made in it after the Mirror is passed over)
        d.remove(&[h2]);
        d.feature_mut(mi).unwrap().mirror.as_mut().unwrap().flip = true;
        let m4 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m4.errors.is_empty(), "{:?}", m4.errors);
        assert!(!m4.faces.iter().any(|f| f.tag.map(|t| t.feature) == Some(b)));
        assert!((m4.bounds().unwrap().1[0] - 10.0).abs() < 1e-3);
        same(&d);
    }

    #[test]
    #[ignore]
    fn islands_probe() {
        // FORME_DOC=file.fnurbs (several: a comma between them): built island
        // by island and as one history, compared; FORME_UPTO=n: its first n
        // features, then n + 1… (where the two part ways)
        let Ok(list) = std::env::var("FORME_DOC") else { return };
        for p in list.split(',') {
            let d = Doc::from_json(&std::fs::read_to_string(p).unwrap()).unwrap();
            let t = std::time::Instant::now();
            let diff = islands_against_one(&d);
            eprintln!("{p}: {} features, {:.1} s: {}", d.features.len(), t.elapsed().as_secs_f64(), if diff.is_empty() { "the same".to_string() } else { diff.join(" · ") });
            if std::env::var("FORME_TAGS").is_ok() {
                // faces per feature, both ways
                let count = |m: &Model| {
                    let mut c: std::collections::BTreeMap<u32, usize> = Default::default();
                    for f in &m.faces {
                        *c.entry(f.tag.map(|t| t.feature).unwrap_or(0)).or_default() += 1;
                    }
                    c
                };
                let ma = build(&d, true, false, &[], &mut Cache::default());
                let a = count(&ma);
                ONE_CHAIN.with(|c| c.set(true));
                let mb = build(&d, true, false, &[], &mut Cache::default());
                ONE_CHAIN.with(|c| c.set(false));
                let b = count(&mb);
                // FORME_SUBS=id: that feature's faces one by one (its face
                // and where its triangles lie), both ways
                if let Some(id) = std::env::var("FORME_SUBS").ok().and_then(|v| v.parse::<u32>().ok()) {
                    for (name, m) in [("islands", &ma), ("one history", &mb)] {
                        for (i, f) in m.faces.iter().enumerate().filter(|(_, f)| id == 0 || f.tag.is_some_and(|t| t.feature == id)) {
                            let (mut lo, mut hi, mut n) = ([f32::MAX; 3], [f32::MIN; 3], 0);
                            for (t, tri) in m.triangles.iter().enumerate().filter(|(t, _)| m.tri_face[*t] as usize == i) {
                                let _ = t;
                                for v in tri {
                                    let p = m.positions[*v as usize];
                                    for k in 0..3 {
                                        lo[k] = lo[k].min(p[k]);
                                        hi[k] = hi[k].max(p[k]);
                                    }
                                }
                                n += 1;
                            }
                            eprintln!("  {name}: feature {} sub {} · {n} triangles · {:.1?} to {:.1?}", f.tag.map_or(0, |t| t.feature), f.tag.map_or(0, |t| t.sub), lo, hi);
                        }
                    }
                }
                eprintln!("one history: {} faces, volume {:.1}", mb.faces.len(), mesh_volume(&mb));
                for id in a.keys().chain(b.keys()).collect::<std::collections::BTreeSet<_>>() {
                    let (x, y) = (a.get(id).copied().unwrap_or(0), b.get(id).copied().unwrap_or(0));
                    if x != y || std::env::var("FORME_IDS").is_ok_and(|v| v.split(',').any(|w| w.parse::<u32>() == Ok(*id))) {
                        eprintln!("  feature {id} ({}): {x} faces in islands, {y} in one history", d.feature(*id).map(|f| f.name.as_str()).unwrap_or("?"));
                    }
                }
            }
            if let Some(from) = std::env::var("FORME_UPTO").ok().and_then(|v| v.parse::<usize>().ok()) {
                for n in from..=d.features.len() {
                    let mut d2 = d.clone();
                    d2.features.truncate(n);
                    let diff = islands_against_one(&d2);
                    if !diff.is_empty() {
                        eprintln!("  first {n} ({}): {}", d2.features.last().map(|f| f.name.as_str()).unwrap_or(""), diff.join(" · "));
                        break;
                    }
                }
            }
        }
    }

    /// FORME_DOC=file.fnurbs with FORME_DELETE=id (that shape and its
    /// operations taken out) or FORME_KEEP=id (they alone kept, as Isolate
    /// does; FORME_MIRRORS=1: the Mirrors too): the build after it, timed,
    /// against the whole document built before; then the whole one again.
    #[test]
    #[ignore]
    fn edit_probe() {
        let Ok(p) = std::env::var("FORME_DOC") else { return };
        let d = Doc::from_json(&std::fs::read_to_string(p).unwrap()).unwrap();
        let mut cache = Cache::default();
        let t = std::time::Instant::now();
        let m = build(&d, true, false, &[], &mut cache);
        eprintln!("whole: {:.0} ms, {} faces, {} triangles, errors {}", t.elapsed().as_secs_f64() * 1e3, m.faces.len(), m.triangles.len(), m.errors.len());
        for id in std::env::var("FORME_KEEP").ok().and_then(|v| v.parse::<u32>().ok()) {
            let fs: Vec<usize> = (0..m.faces.len()).filter(|k| m.faces[*k].tag.is_some_and(|t| t.feature == id)).collect();
            eprintln!("  feature {id}: {} faces, {} triangles in the whole part", fs.len(), m.tri_face.iter().filter(|f| fs.contains(&(**f as usize))).count());
        }
        let with_ops = |id: FeatureId| {
            let mut ids = vec![id];
            for f in &d.features {
                if !f.is_solid() && f.mirror.is_none() && crate::doc::named_features(f).iter().any(|n| ids.contains(n)) {
                    ids.push(f.id);
                }
            }
            ids
        };
        let mut e = d.clone();
        if let Some(id) = std::env::var("FORME_DELETE").ok().and_then(|v| v.parse().ok()) {
            e.remove(&with_ops(id));
        } else if let Some(id) = std::env::var("FORME_KEEP").ok().and_then(|v| v.parse().ok()) {
            let ids = with_ops(id);
            let mirrors = std::env::var("FORME_MIRRORS").is_ok();
            e.features.retain(|f| ids.contains(&f.id) || (mirrors && f.mirror.is_some()));
        }
        for k in 0..2 {
            let t = std::time::Instant::now();
            let m = build(&e, true, false, &[], &mut cache);
            eprintln!("edited ({} features){}: {:.0} ms, {} faces, {} triangles, cached {}, errors {}", e.features.len(), if k == 1 { " again" } else { "" }, t.elapsed().as_secs_f64() * 1e3, m.faces.len(), m.triangles.len(), m.cached, m.errors.len());
        }
        let t = std::time::Instant::now();
        let m = build(&d, true, false, &[], &mut cache);
        eprintln!("whole again: {:.0} ms, {} faces, cached {}", t.elapsed().as_secs_f64() * 1e3, m.faces.len(), m.cached);
    }

    #[test]
    #[ignore]
    fn valid_probe() {
        // FORME_DOC=file.fnurbs: the document's first n features built
        // for each n (a cache of its own), whether the part is a valid
        // solid for BRepCheck (a fillet on an invalid part fails with
        // "invalid solid" whatever its edges), and the errors
        let Ok(p) = std::env::var("FORME_DOC") else { return };
        let d0 = Doc::from_json(&std::fs::read_to_string(p).unwrap()).unwrap();
        let from: usize = std::env::var("FORME_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        for n in from..=d0.features.len() {
            let mut d = d0.clone();
            d.features.truncate(n);
            let d = sided_doc(&d).unwrap_or(d);
            let arrayed = arrayed_doc(&d);
            let doc = arrayed.as_ref().unwrap_or(&d);
            let mirrored = mirrored_doc(doc);
            let doc = mirrored.as_ref().unwrap_or(doc);
            let feats: Vec<&Feature> = doc.features.iter().filter(|f| f.visible && !f.suspended && doc.makes(f) == Makes::Solid).collect();
            let mut model = Model::default();
            let (mut ut, mut us) = (vec![], vec![]);
            let st = build_islands(doc, &feats, &[], &mut Cache::default(), &mut model, &mut ut, &mut us);
            let s = st.shape.as_ref();
            eprintln!("{n:>3} {:<14} valid {:?} faces {:?} lumps {:?} errors {:?}", d.features[n - 1].name, s.map(|s| s.is_valid()), s.map(|s| s.face_count()), s.map(|s| s.lump_boxes().len()), st.errors);
        }
    }

    #[test]
    #[ignore]
    fn drag_probe() {
        // FORME_DOC=file.fnurbs FORME_DRAG=id: the coarse builds a drag of
        // that shape makes (8 steps of 1 mm along X), timed
        let Ok(p) = std::env::var("FORME_DOC") else { return };
        if std::env::var_os("FORME_SIGNALS").is_some() {
            forme_occt::catch_signals();
        }
        let id: FeatureId = std::env::var("FORME_DRAG").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        let mut d = Doc::from_json(&std::fs::read_to_string(p).unwrap()).unwrap();
        let mut cache = Cache::default();
        let t = std::time::Instant::now();
        let m = build(&d, false, false, &[], &mut cache);
        eprintln!("first coarse build {:.0} ms, {} faces, cached {}", t.elapsed().as_secs_f64() * 1e3, m.faces.len(), m.cached);
        if std::env::var("FORME_ERRORS").is_ok() {
            for e in &m.errors {
                eprintln!("  ERROR {e}");
            }
        }
        let t = std::time::Instant::now();
        let m = build(&d, true, false, &[], &mut cache);
        eprintln!("fine build {:.0} ms, {} faces, cached {}", t.elapsed().as_secs_f64() * 1e3, m.faces.len(), m.cached);
        // the shape's operations go with it, as the app carries them
        let mut ids = vec![id];
        for f in &d.features {
            if !f.is_solid() && crate::doc::named_features(f).iter().any(|n| ids.contains(n)) {
                ids.push(f.id);
            }
        }
        let mut worst = 0.0f64;
        // FORME_STEP: mm a step along X (1), FORME_AXIS: x, y or z
        let step: f64 = std::env::var("FORME_STEP").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
        let dir = match std::env::var("FORME_AXIS").as_deref() {
            Ok("y") => DVec3::Y,
            Ok("z") => DVec3::Z,
            _ => DVec3::X,
        } * step;
        let steps: usize = std::env::var("FORME_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
        for i in 0..steps {
            for f in d.features.iter_mut().filter(|f| ids.contains(&f.id)) {
                if f.id == id {
                    f.origin += dir;
                } else {
                    *f = f.carried_by(&|p| p + dir);
                }
            }
            let t = std::time::Instant::now();
            let m = build(&d, false, false, &[], &mut cache);
            worst = worst.max(t.elapsed().as_secs_f64() * 1e3);
            eprintln!("drag {i}: {:.0} ms, cached {} of {}, errors {}", t.elapsed().as_secs_f64() * 1e3, m.cached, d.features.len(), m.errors.len());
            if std::env::var("FORME_ERRORS").is_ok() {
                for e in &m.errors {
                    eprintln!("  ERROR {e}");
                }
            }
        }
        let t = std::time::Instant::now();
        let m = build(&d, true, false, &[], &mut cache);
        eprintln!("fine build after the drag {:.0} ms, {} faces, errors {} (worst drag step {worst:.0} ms)", t.elapsed().as_secs_f64() * 1e3, m.faces.len(), m.errors.len());
        if std::env::var("FORME_PROFILE").is_ok() {
            let _ = build(&d, false, false, &[], &mut Cache::default());
        }
    }

    #[test]
    fn moves_on_a_part_in_two_lumps() {
        // a cube added apart from the block (20 nov.): its top +X edge
        // pushed out, its top moved up
        for comp in [
            Comp::Edge(EdgeRef { faces: [tag(2, 0), tag(2, 2)], at: DVec3::new(18.0, 26.0, -75.0) }),
            Comp::Face(FaceRef { tag: tag(2, 2), at: DVec3::new(0.0, 26.0, -75.0) }),
        ] {
            let mut d = Doc::default();
            d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            d.add(Op::Add, Prim::Box { x: 36.0, y: 26.0, z: 36.0 }, DVec3::new(0.0, 0.0, -75.0), DQuat::IDENTITY);
            d.add_tweak(Tweak { comp, delta: DVec3::new(5.0, 2.0, 0.0), more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "{:?}", m.errors);
            assert_eq!(m.faces.len(), 12);
            assert!(m.faces.iter().all(|f| f.tag.is_some()));
        }
    }

    #[test]
    fn thin_walls_avoided() {
        // a Carve 0.2 mm short of the block's front face: opened through
        // it, no 0.2 mm wall left; an Add floating 0.2 mm above the top:
        // put on it (one solid); an Add whose side is 0.2 mm off the
        // block's side: flush, no thin step (Romain, 20 nov. 5 h 57)
        let base = || {
            let mut d = Doc::default();
            d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            d
        };
        let mut d = base();
        d.add(Op::Carve, Prim::Box { x: 10.0, y: 10.0, z: 10.0 }, DVec3::new(0.0, 15.0, 9.8), DQuat::IDENTITY);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.len(), 10, "the pocket opens on the front");
        let mut d = base();
        d.add(Op::Add, Prim::Box { x: 10.0, y: 5.0, z: 10.0 }, DVec3::new(0.0, 20.2, 0.0), DQuat::IDENTITY);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.len(), 11, "the block on the part, one solid");
        let (_, hi) = m.bounds().unwrap();
        assert!((hi[1] - 25.2).abs() < 1e-3, "its top stays: {hi:?}");
        let mut d = base();
        d.add(Op::Add, Prim::Box { x: 10.0, y: 30.0, z: 10.0 }, DVec3::new(0.0, 0.0, 9.8), DQuat::IDENTITY);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let (_, hi) = m.bounds().unwrap();
        assert!((hi[2] - 15.0).abs() < 1e-3, "flush with the side: {hi:?}");
        // a real wall (1 mm) is kept
        let mut d = base();
        d.add(Op::Carve, Prim::Box { x: 10.0, y: 10.0, z: 10.0 }, DVec3::new(0.0, 15.0, 9.0), DQuat::IDENTITY);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert_eq!(m.faces.len(), 11, "a 1 mm wall stays");
    }

    fn b0(d: &Doc) -> FeatureId {
        d.features[0].id
    }

    #[test]
    fn copy_parts_moved() {
        // Romain, 20 nov. 6 h 57: a cube, three faces copied and moved
        // apart, then one face of the copy moved, then one of its edges
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let fr = |sub: u16, at: DVec3| Comp::Face(FaceRef { tag: tag(b, sub), at });
        let mut t = Tweak::new(vec![fr(2, DVec3::new(0.0, 20.0, 0.0)), fr(0, DVec3::new(20.0, 10.0, 0.0)), fr(4, DVec3::new(0.0, 10.0, 15.0))]).unwrap();
        t.copy = true;
        t.delta = DVec3::new(50.0, 0.0, 0.0);
        d.add_tweak(t);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let cid = d.features.last().unwrap().id;
        let copy: Vec<(FaceTag, DVec3)> = m.faces.iter().enumerate().filter(|(_, f)| f.tag.map(|t| t.feature) == Some(cid)).map(|(i, f)| {
            let t = m.tri_face.iter().position(|&x| x as usize == i).unwrap();
            let c = m.triangles[t].iter().map(|&v| DVec3::from_array(m.positions[v as usize].map(|x| x as f64))).sum::<DVec3>() / 3.0;
            (f.tag.unwrap(), c)
        }).collect();
        eprintln!("copy: {} faces, part {} faces", copy.len(), m.faces.len());
        assert!(m.faces.iter().all(|f| f.kind == crate::model::SurfKind::Plane), "a copy moved keeps its planes");
        // its top (the face highest up) moved up 5
        let (tt, at) = copy.iter().copied().max_by(|a, b| a.1.y.total_cmp(&b.1.y)).unwrap();
        let mut f1 = d.clone();
        let mut t = Tweak::new(vec![Comp::Face(FaceRef { tag: tt.pack(), at })]).unwrap();
        t.delta = DVec3::new(0.0, 5.0, 0.0);
        f1.add_tweak(t);
        let m1 = build(&f1, true, false, &[], &mut Cache::default());
        eprintln!("face moved: errors {:?}, bounds {:?}", m1.errors, m1.bounds());
        // one of its edges: the top's edge along +X... between the top and
        // the front of the copy
        let ed = m.edges.iter().find(|e| {
            let tags: Vec<Option<FaceTag>> = e.faces.iter().map(|&f| if f >= 0 { m.faces[f as usize].tag } else { None }).collect();
            tags.iter().all(|t| t.map(|t| t.feature == cid).unwrap_or(false)) && e.faces[0] != e.faces[1]
        });
        let ed = ed.expect("an edge between two faces of the copy");
        let at = {
            let p = &ed.pts[ed.pts.len() / 2];
            DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64)
        };
        let er = EdgeRef { faces: [m.faces[ed.faces[0] as usize].tag.unwrap().pack(), m.faces[ed.faces[1] as usize].tag.unwrap().pack()], at };
        let mut f2 = d.clone();
        let mut t = Tweak::new(vec![Comp::Edge(er)]).unwrap();
        t.delta = DVec3::new(0.0, 3.0, 3.0);
        f2.add_tweak(t);
        let m2 = build(&f2, true, false, &[], &mut Cache::default());
        eprintln!("edge moved: errors {:?}, bounds {:?}", m2.errors, m2.bounds());
        assert!(m1.errors.is_empty(), "{:?}", m1.errors);
        assert!(m2.errors.is_empty(), "{:?}", m2.errors);
        // the cube's own top moved up with the copy beside it
        let mut f3 = d.clone();
        let mut t = Tweak::new(vec![fr(2, DVec3::new(0.0, 20.0, 0.0))]).unwrap();
        t.delta = DVec3::new(0.0, 4.0, 0.0);
        f3.add_tweak(t);
        let m3 = build(&f3, true, false, &[], &mut Cache::default());
        assert!(m3.errors.is_empty(), "{:?}", m3.errors);
        assert_eq!(m3.faces.len(), 9);
        // the copy's three faces then moved again together, then one alone
        let mut f4 = f1.clone();
        let comps: Vec<Comp> = copy.iter().map(|(t, at)| Comp::Face(FaceRef { tag: t.pack(), at: *at + if t.pack() == tt.pack() { DVec3::new(0.0, 5.0, 0.0) } else { DVec3::ZERO } })).collect();
        let mut t = Tweak::new(comps).unwrap();
        t.delta = DVec3::new(0.0, 0.0, -10.0);
        f4.add_tweak(t);
        let m4 = build(&f4, true, false, &[], &mut Cache::default());
        eprintln!("all moved after one: errors {:?}, bounds {:?}", m4.errors, m4.bounds());
        assert!(m4.errors.is_empty(), "{:?}", m4.errors);
        // a closed copy (the five faces but the bottom: closed by a face,
        // a solid) moved apart, then its top alone moved up
        let mut d5 = Doc::default();
        let b5 = d5.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let fr5 = |sub: u16, at: DVec3| Comp::Face(FaceRef { tag: tag(b5, sub), at });
        let mut t = Tweak::new(vec![fr5(2, DVec3::new(0.0, 20.0, 0.0)), fr5(0, DVec3::new(20.0, 10.0, 0.0)), fr5(1, DVec3::new(-20.0, 10.0, 0.0)), fr5(4, DVec3::new(0.0, 10.0, 15.0)), fr5(5, DVec3::new(0.0, 10.0, -15.0))]).unwrap();
        t.copy = true;
        t.delta = DVec3::new(50.0, 0.0, 0.0);
        d5.add_tweak(t);
        let m5 = build(&d5, true, false, &[], &mut Cache::default());
        let c5 = d5.features.last().unwrap().id;
        let top = m5.faces.iter().enumerate().filter(|(_, f)| f.tag.map(|t| t.feature) == Some(c5)).map(|(i, f)| {
            let t = m5.tri_face.iter().position(|&x| x as usize == i).unwrap();
            (f.tag.unwrap(), m5.triangles[t].iter().map(|&v| DVec3::from_array(m5.positions[v as usize].map(|x| x as f64))).sum::<DVec3>() / 3.0)
        }).max_by(|a, b| a.1.y.total_cmp(&b.1.y)).unwrap();
        let mut t = Tweak::new(vec![Comp::Face(FaceRef { tag: top.0.pack(), at: top.1 })]).unwrap();
        t.delta = DVec3::new(0.0, 6.0, 0.0);
        d5.add_tweak(t);
        let m6 = build(&d5, true, false, &[], &mut Cache::default());
        eprintln!("closed copy's top moved: errors {:?}, bounds {:?}", m6.errors, m6.bounds());
        assert!(m6.errors.is_empty(), "{:?}", m6.errors);
        assert!((m6.bounds().unwrap().1[1] - 26.0).abs() < 1e-3);
    }

    #[test]
    fn faces_duplicated() {
        // Ctrl + D on picked faces (20 nov.): a block's top and its four
        // sides copied, closed by a bottom, moved out 30 along X (a second
        // block); the top alone (a sheet beside the part); the leg of an
        // Extrude copied 10 forward, still touching the part (fused)
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let fr = |sub: u16, at: DVec3| Comp::Face(FaceRef { tag: tag(b, sub), at });
        let mut box5 = d.clone();
        let mut t = Tweak::new(vec![fr(2, DVec3::new(0.0, 20.0, 0.0)), fr(0, DVec3::new(20.0, 10.0, 0.0)), fr(1, DVec3::new(-20.0, 10.0, 0.0)), fr(4, DVec3::new(0.0, 10.0, 15.0)), fr(5, DVec3::new(0.0, 10.0, -15.0))]).unwrap();
        t.copy = true;
        t.delta = DVec3::new(50.0, 0.0, 0.0);
        box5.add_tweak(t);
        let m = build(&box5, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.len(), 12, "two blocks");
        let (_, hi) = m.bounds().unwrap();
        assert!((hi[0] - 70.0).abs() < 1e-3, "{hi:?}");
        // every face of the copy moved: it goes as a piece (20 nov.: its
        // faces moved one by one only moved the pivot)
        let cid = box5.features.last().unwrap().id;
        let comps: Vec<Comp> = m.faces.iter().enumerate().filter(|(_, f)| f.tag.map(|t| t.feature) == Some(cid)).map(|(i, f)| {
            let t = m.tri_face.iter().position(|&x| x as usize == i).unwrap();
            let c = m.triangles[t].iter().map(|&v| DVec3::from_array(m.positions[v as usize].map(|x| x as f64))).sum::<DVec3>() / 3.0;
            Comp::Face(FaceRef { tag: f.tag.unwrap().pack(), at: c })
        }).collect();
        assert_eq!(comps.len(), 6, "the copy's faces");
        let subs: std::collections::HashSet<u64> = m.faces.iter().filter_map(|f| f.tag).filter(|t| t.feature == cid).map(|t| t.pack() as u64).collect();
        assert_eq!(subs.len(), 6, "each copied face its own tag");
        let mut mv = box5.clone();
        let mut t = Tweak::new(comps).unwrap();
        t.delta = DVec3::new(10.0, 0.0, 5.0);
        mv.add_tweak(t);
        let m2 = build(&mv, true, false, &[], &mut Cache::default());
        assert!(m2.errors.is_empty(), "{:?}", m2.errors);
        let (_, hi) = m2.bounds().unwrap();
        assert!((hi[0] - 80.0).abs() < 1e-3 && (hi[2] - 20.0).abs() < 1e-3, "{hi:?}");
        // a block with a filleted top edge: its top, fillet and front copied
        // (an open sheet), then all moved
        let mut fd = d.clone();
        let e = |a: u16, b: u16, at: DVec3| vec![EdgeRef { faces: [tag(b0(&fd), a), tag(b0(&fd), b)], at }];
        let r = e(2, 4, DVec3::new(0.0, 20.0, 15.0));
        fd.add_round(3.0, false, r);
        let fm = build(&fd, true, false, &[], &mut Cache::default());
        assert!(fm.errors.is_empty(), "{:?}", fm.errors);
        let pick = |m: &Model, keep: &dyn Fn(FaceTag) -> bool| -> Vec<Comp> {
            m.faces.iter().enumerate().filter(|(_, f)| f.tag.map(|t| keep(t)).unwrap_or(false)).map(|(i, f)| {
                let t = m.tri_face.iter().position(|&x| x as usize == i).unwrap();
                let c = m.triangles[t].iter().map(|&v| DVec3::from_array(m.positions[v as usize].map(|x| x as f64))).sum::<DVec3>() / 3.0;
                Comp::Face(FaceRef { tag: f.tag.unwrap().pack(), at: c })
            }).collect()
        };
        let rid = fd.features.last().unwrap().id;
        let comps = pick(&fm, &|t| (t.feature == b && (t.sub == 2 || t.sub == 4)) || t.feature == rid);
        assert_eq!(comps.len(), 3);
        let mut t = Tweak::new(comps).unwrap();
        t.copy = true;
        t.delta = DVec3::new(45.0, 0.0, 0.0);
        fd.add_tweak(t);
        let cm = build(&fd, true, false, &[], &mut Cache::default());
        assert!(cm.errors.is_empty(), "{:?}", cm.errors);
        let cid = fd.features.last().unwrap().id;
        let comps = pick(&cm, &|t| t.feature == cid);
        let mut t = Tweak::new(comps).unwrap();
        t.delta = DVec3::new(0.0, 0.0, 7.0);
        fd.add_tweak(t);
        let mm = build(&fd, true, false, &[], &mut Cache::default());
        eprintln!("sheet copy moved: errors {:?}, bounds {:?}", mm.errors, mm.bounds());
        assert!(mm.errors.is_empty(), "{:?}", mm.errors);
        let (_, hi) = mm.bounds().unwrap();
        assert!((hi[2] - 22.0).abs() < 1e-3, "{hi:?}");
        let mut top = d.clone();
        let mut t = Tweak::new(vec![fr(2, DVec3::new(0.0, 20.0, 0.0))]).unwrap();
        t.copy = true;
        t.delta = DVec3::new(0.0, 10.0, 0.0);
        top.add_tweak(t);
        let m = build(&top, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.faces.len(), 7, "the block and a sheet");
        // a leg pushed down from the bottom, its faces copied forward
        let mut leg = d.clone();
        leg.add_push(Push { face: FaceRef { tag: tag(b, 3), at: DVec3::new(0.0, 0.0, 0.0) }, inset: 0.0, depth: 10.0 });
        let mid = build(&leg, true, false, &[], &mut Cache::default());
        assert!(mid.errors.is_empty(), "{:?}", mid.errors);
        let pid = leg.features.last().unwrap().id;
        let comps: Vec<Comp> = mid.faces.iter().enumerate().filter(|(_, f)| f.tag.map(|t| t.feature) == Some(pid)).map(|(i, f)| {
            // a point of the face: its first triangle's centre
            let t = mid.tri_face.iter().position(|&x| x as usize == i).unwrap();
            let c = mid.triangles[t].iter().map(|&v| DVec3::from_array(mid.positions[v as usize].map(|x| x as f64))).sum::<DVec3>() / 3.0;
            Comp::Face(FaceRef { tag: f.tag.unwrap().pack(), at: c })
        }).collect();
        let mut t = Tweak::new(comps).unwrap();
        t.copy = true;
        t.delta = DVec3::new(0.0, 0.0, 5.0);
        leg.add_tweak(t);
        let m = build(&leg, true, false, &[], &mut Cache::default());
        eprintln!("leg copied: errors {:?}, {} faces, bounds {:?}", m.errors, m.faces.len(), m.bounds());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
    }

    #[test]
    fn knife_kept_by_later_booleans() {
        // a Knife's line on a side, then a cube added apart and a boss on
        // the cut side: the side stays in two pieces (20 nov.: the
        // boolean's simplification made them one again)
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let kp = |faces: [i64; 2], at: DVec3| KnifePoint { edge: EdgeRef { faces, at }, f: (20.0 - at.x.abs()) / 40.0, at };
        d.add_knife(Knife { pts: vec![kp([tag(b, 4), tag(b, 2)], DVec3::new(5.0, 20.0, 15.0)), kp([tag(b, 4), tag(b, 3)], DVec3::new(5.0, 0.0, 15.0))], segs: vec![[0, 1]], remove_faces: vec![], remove_edges: vec![] }, "Knife");
        d.add(Op::Add, Prim::Box { x: 10.0, y: 10.0, z: 10.0 }, DVec3::new(0.0, 0.0, -80.0), DQuat::IDENTITY);
        d.add(Op::Add, Prim::Cylinder { radius: 3.0, height: 4.0 }, DVec3::new(-8.0, 10.0, 15.0), DQuat::from_rotation_x(std::f64::consts::FRAC_PI_2));
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let n = m.faces.iter().filter(|f| f.tag.map(|t| t.pack()) == Some(tag(b, 4))).count();
        assert_eq!(n, 2, "the cut side's pieces");
    }

    #[test]
    fn moves_by_fillets_and_cuts() {
        // Romain's flank (20 nov.): a block whose top long edges are
        // filleted, then cut by the Knife on its +Z side; the cut's edge
        // moved along X (its top end slides along the fillet's edge), a
        // diagonal cut from a corner taken out, the +X end moved out (its
        // fillets' ends follow along their axes)
        let block = |d: &mut Doc| {
            let b = d.add(Op::Add, Prim::Box { x: 60.0, y: 40.0, z: 27.4 }, DVec3::ZERO, DQuat::IDENTITY);
            let r = d.add_round(
                6.6,
                false,
                vec![
                    EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(0.0, 40.0, 13.7) },
                    EdgeRef { faces: [tag(b, 2), tag(b, 5)], at: DVec3::new(0.0, 40.0, -13.7) },
                ],
            );
            (b, r)
        };
        // a point on a straight edge running along X from -30 to 30
        let kx = |faces: [i64; 2], at: DVec3| KnifePoint { edge: EdgeRef { faces, at }, f: (30.0 - at.x.abs()) / 60.0, at };
        let mut cases: Vec<(&str, Doc)> = vec![];
        for (name, dx) in [("cut edge moved +X", 1.631), ("cut edge moved -X", -4.0)] {
            let mut d = Doc::default();
            let (b, r) = block(&mut d);
            let side = tag(b, 4);
            d.add_knife(Knife { pts: vec![kx([side, tag(r, 0)], DVec3::new(11.0, 33.4, 13.7)), kx([side, tag(b, 3)], DVec3::new(11.0, 0.0, 13.7))], segs: vec![[0, 1]], remove_faces: vec![], remove_edges: vec![] }, "Knife");
            d.add_tweak(Tweak { comp: Comp::Edge(EdgeRef { faces: [side, side], at: DVec3::new(11.0, 16.7, 13.7) }), delta: DVec3::new(dx, 0.0, 0.0), more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
            cases.push((name, d));
        }
        {
            let mut d = Doc::default();
            let (b, r) = block(&mut d);
            let side = tag(b, 4);
            d.add_knife(Knife { pts: vec![kx([side, tag(r, 0)], DVec3::new(11.0, 33.4, 13.7)), kx([side, tag(b, 3)], DVec3::new(11.0, 0.0, 13.7))], segs: vec![[0, 1]], remove_faces: vec![], remove_edges: vec![] }, "Knife");
            d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(b, 0), at: DVec3::new(30.0, 15.0, 0.0) }), delta: DVec3::new(3.0, 0.0, 0.0), more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
            cases.push(("+X end moved out after a cut", d));
        }
        // across the fillet: a half of the cut flank moved out, the top
        // moved up (the fillet made again after the move)
        for (name, face, at, delta) in [
            ("cut flank's half moved out", 4u16, DVec3::new(20.0, 15.0, 13.7), DVec3::new(0.0, 0.0, 2.0)),
            ("top moved up after a cut", 2u16, DVec3::new(0.0, 40.0, 0.0), DVec3::new(0.0, 3.0, 0.0)),
            ("cut flank's half moved in", 4u16, DVec3::new(20.0, 15.0, 13.7), DVec3::new(0.0, 0.0, -2.0)),
        ] {
            let mut d = Doc::default();
            let (b, r) = block(&mut d);
            let side = tag(b, 4);
            d.add_knife(Knife { pts: vec![kx([side, tag(r, 0)], DVec3::new(11.0, 33.4, 13.7)), kx([side, tag(b, 3)], DVec3::new(11.0, 0.0, 13.7))], segs: vec![[0, 1]], remove_faces: vec![], remove_edges: vec![] }, "Knife");
            d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(b, face), at }), delta, more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
            cases.push((name, d));
        }
        {
            let mut d = Doc::default();
            let (b, r) = block(&mut d);
            let side = tag(b, 4);
            d.add_knife(Knife { pts: vec![kx([side, tag(r, 0)], DVec3::new(-30.0, 33.4, 13.7)), kx([side, tag(b, 3)], DVec3::new(5.0, 0.0, 13.7))], segs: vec![[0, 1]], remove_faces: vec![], remove_edges: vec![] }, "Knife");
            let m = build(&d, true, false, &[], &mut Cache::default());
            eprintln!("diagonal cut: errors {:?}, faces {}", m.errors, m.faces.len());
            let e = m.edges.iter().filter(|e| e.faces[0] >= 0 && e.faces[1] >= 0 && m.faces[e.faces[0] as usize].tag.map(|t| t.pack()) == Some(side) && m.faces[e.faces[1] as usize].tag.map(|t| t.pack()) == Some(side)).count();
            let one_sided = m.edges.iter().filter(|e| e.faces[1] < 0).count();
            eprintln!("diagonal cut: {e} edges between its halves, {one_sided} one-sided edges");
            let mut d2 = d.clone();
            d2.add_knife(Knife { pts: vec![], segs: vec![], remove_faces: vec![], remove_edges: vec![EdgeRef { faces: [side, side], at: DVec3::new(-12.5, 16.7, 13.7) }] }, "Delete");
            cases.push(("diagonal cut from a corner deleted", d2));
            d.add_tweak(Tweak { comp: Comp::Edge(EdgeRef { faces: [side, side], at: DVec3::new(-12.5, 16.7, 13.7) }), delta: DVec3::new(2.0, 0.0, 0.0), more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
            let m = build(&d, true, false, &[], &mut Cache::default());
            eprintln!("diagonal cut from a corner moved: {:?} (its corner cannot leave the fillet's end)", m.errors);
        }
        let mut bad = vec![];
        for (name, d) in &cases {
            let m = build(d, true, false, &[], &mut Cache::default());
            eprintln!("{name:<40} errors {:?}, {} faces, bounds {:?}", m.errors, m.faces.len(), m.bounds());
            if !m.errors.is_empty() {
                bad.push(format!("{name}: {:?}", m.errors));
            }
        }
        assert!(bad.is_empty(), "{} failed:\n{}", bad.len(), bad.join("\n"));
    }

    #[test]
    fn edges_on_an_open_part() {
        // 20 nov., 11 h 48 (his Knife 2: "ne peut ni se supprimer ni se
        // déplacer"): a face of the block deleted, the part open; the line
        // a Knife cut across its front then deleted, or moved along it
        let base = |d: &mut Doc| {
            let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
            let front = tag(b, 4);
            let kp = |faces: [i64; 2], at: DVec3| KnifePoint { edge: EdgeRef { faces, at }, f: 0.5, at };
            d.add_knife(Knife { pts: vec![kp([front, tag(b, 2)], DVec3::new(0.0, 20.0, 15.0)), kp([front, tag(b, 3)], DVec3::new(0.0, 0.0, 15.0))], segs: vec![[0, 1]], remove_faces: vec![], remove_edges: vec![] }, "Knife");
            d.add_knife(Knife { pts: vec![], segs: vec![], remove_faces: vec![FaceRef { tag: tag(b, 1), at: DVec3::new(-20.0, 10.0, 0.0) }], remove_edges: vec![] }, "Delete faces");
            front
        };
        let line = |front: i64| EdgeRef { faces: [front, front], at: DVec3::new(0.0, 10.0, 15.0) };
        let mut d = Doc::default();
        let front = base(&mut d);
        let m0 = build(&d, true, false, &[], &mut Cache::default());
        assert!(m0.errors.is_empty(), "{:?}", m0.errors);
        let fronts = |m: &Model| m.faces.iter().filter(|f| f.tag.map(|t| t.pack()) == Some(front)).count();
        assert_eq!(fronts(&m0), 2, "the front in two");
        let mut d1 = d.clone();
        d1.add_knife(Knife { pts: vec![], segs: vec![], remove_faces: vec![], remove_edges: vec![line(front)] }, "Delete edges");
        let m1 = build(&d1, true, false, &[], &mut Cache::default());
        assert!(m1.errors.is_empty(), "delete: {:?}", m1.errors);
        assert_eq!(fronts(&m1), 1, "the front one again");
        let mut d2 = d.clone();
        d2.add_tweak(Tweak { comp: Comp::Edge(line(front)), delta: DVec3::new(-5.0, 0.0, 0.0), more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
        let m2 = build(&d2, true, false, &[], &mut Cache::default());
        assert!(m2.errors.is_empty(), "move: {:?}", m2.errors);
        let at = m2.edges.iter().filter(|e| e.faces[0] >= 0 && e.faces[1] >= 0).filter(|e| e.faces.iter().all(|&f| m2.faces[f as usize].tag.map(|t| t.pack()) == Some(front))).map(|e| e.pts[0][0]).collect::<Vec<_>>();
        assert!(at.iter().any(|x| (*x + 5.0).abs() < 1e-3), "the line at x -5: {at:?}");
    }

    #[test]
    fn tilted_faces_with_holes() {
        // a face with holes, bosses or pockets that tilts (an edge of it
        // raised or lowered, the face moved along its normal): their walls
        // run along to the new plane; what cannot is refused with a reason
        let block = |d: &mut Doc| d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let left_edge = |b: u32, dy: f64| Tweak { comp: Comp::Edge(EdgeRef { faces: [tag(b, 2), tag(b, 1)], at: DVec3::new(-20.0, 20.0, 0.0) }), delta: DVec3::new(0.0, dy, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true };
        let right_edge = |b: u32, dy: f64| Tweak { comp: Comp::Edge(EdgeRef { faces: [tag(b, 2), tag(b, 0)], at: DVec3::new(20.0, 20.0, 0.0) }), delta: DVec3::new(0.0, dy, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true };
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
        d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(b, 2), at: DVec3::new(10.0, 20.0, 10.0) }), delta: DVec3::new(0.0, 3.0, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
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
        d.feature_mut(s).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [0.0, 0.1], [-0.5, 0.5]], smooth: vec![false, false, false, true, false], corner: vec![], tension: vec![], open: false, spline: false });
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
        d.add_tweak(Tweak { comp: Comp::Edge(EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(6.0, 20.0, 15.0) }), delta: DVec3::new(0.0, 6.0, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
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
        d.add_tweak(Tweak { comp: Comp::Vertex(DVec3::new(20.0, 20.0, 15.0)), delta: DVec3::new(0.0, 3.0, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
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

    type ComboOp = Box<dyn Fn(&mut Doc)>;

    /// The battery's 14 operations on a test part: its block `b`, hole `h`,
    /// boss `s` and slot `sl`, the part moved by `off`.
    fn combo_ops(b: u32, h: u32, s: u32, sl: u32, off: DVec3) -> Vec<(&'static str, ComboOp)> {
        let er = move |a: i64, c: i64, x: f64, y: f64, z: f64| EdgeRef { faces: [a, c], at: DVec3::new(x, y, z) + off };
        let fr = move |t: i64, x: f64, y: f64, z: f64| FaceRef { tag: t, at: DVec3::new(x, y, z) + off };
        type Op_ = ComboOp;
        let round = |size: f64, chamfer: bool, tension: f64, e: EdgeRef| -> Op_ {
            Box::new(move |d: &mut Doc| {
                let r = d.add_round(size, chamfer, vec![e.clone()]);
                d.feature_mut(r).unwrap().round.as_mut().unwrap().tension = tension;
            })
        };
        let push = |face: FaceRef, inset: f64, depth: f64| -> Op_ { Box::new(move |d: &mut Doc| { d.add_push(Push { face: face.clone(), inset, depth }); }) };
        let tweak = |comp: Comp, delta: DVec3| -> Op_ { Box::new(move |d: &mut Doc| { d.add_tweak(Tweak { comp: comp.clone(), delta , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true }); }) };
        vec![
            ("fillet top-front", round(2.0, false, 0.0, er(tag(b, 2), tag(b, 4), -12.0, 20.0, 15.0))),
            ("tension fillet top -X", round(3.0, false, 0.7, er(tag(b, 2), tag(b, 1), -20.0, 20.0, 0.0))),
            ("chamfer side vertical", round(2.0, true, 0.0, er(tag(b, 0), tag(b, 4), 20.0, 10.0, 15.0))),
            ("fillet boss top", round(1.0, false, 0.0, er(tag(s, 0), tag(s, 1), 17.0, 30.0, 0.0))),
            ("fillet hole mouth", round(1.0, false, 0.0, er(tag(b, 2), tag(h, 0), -5.0, 20.0, 0.0))),
            ("inset front", push(fr(tag(b, 4), -12.0, 6.0, 15.0), 2.0, 0.0)),
            ("extrude front", push(fr(tag(b, 4), -12.0, 6.0, 15.0), 0.0, 3.0)),
            ("extrude +X in", push(fr(tag(b, 0), 20.0, 8.0, 0.0), 0.0, -2.0)),
            ("extrude boss side", push(fr(tag(s, 0), 17.0, 25.0, 0.0), 0.0, 1.0)),
            ("inset slot floor", push(fr(tag(sl, 3), 0.0, 14.0, 0.0), 1.0, -1.0)),
            ("move top-left up", tweak(Comp::Face(fr(tag(b, 2), -12.0, 20.0, 8.0)), DVec3::new(0.0, 3.0, 0.0))),
            ("tilt top-left", tweak(Comp::Edge(er(tag(b, 2), tag(b, 1), -20.0, 20.0, 0.0)), DVec3::new(0.0, 3.0, 0.0))),
            ("move +X out", tweak(Comp::Face(fr(tag(b, 0), 20.0, 8.0, 0.0)), DVec3::new(4.0, 0.0, 0.0))),
            ("knife front", Box::new(move |d: &mut Doc| {
                let pts = vec![
                    KnifePoint { edge: er(tag(b, 1), tag(b, 4), -20.0, 10.0, 15.0), f: 0.5, at: DVec3::new(-20.0, 10.0, 15.0) + off },
                    KnifePoint { edge: er(tag(b, 0), tag(b, 4), 20.0, 10.0, 15.0), f: 0.5, at: DVec3::new(20.0, 10.0, 15.0) + off },
                ];
                d.add_knife(Knife { pts, segs: vec![[0, 1]], remove_faces: vec![], remove_edges: vec![] }, "Knife");
            })),
        ]
    }

    /// Two test parts 100 mm apart, the battery's operations made on one
    /// then the other in turn (A on the first, B on the second, B on the
    /// first, A on the second), every ordered pair: built island by island,
    /// the same part as one history. `FORME_COMBO=name` filters.
    #[test]
    #[ignore]
    fn islands_battery() {
        let (mut base, b, s) = test_part();
        let off = DVec3::new(100.0, 0.0, 0.0);
        let b2 = base.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, off, DQuat::IDENTITY);
        let h2 = base.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-10.0, -5.0, 0.0) + off, DQuat::IDENTITY);
        let s2 = base.add(Op::Add, Prim::Cylinder { radius: 7.0, height: 10.0 }, DVec3::new(10.0, 20.0, 0.0) + off, DQuat::IDENTITY);
        let sl2 = base.add(Op::Carve, Prim::Box { x: 8.0, y: 8.0, z: 40.0 }, DVec3::new(0.0, 14.0, 0.0) + off, DQuat::IDENTITY);
        let (one, two) = (combo_ops(b, 2, s, 4, DVec3::ZERO), combo_ops(b2, h2, s2, sl2, off));
        // FORME_BATTERY=mirror-first / mirror-last: a Mirror across X through
        // the first part (cut at the plane; the second lies off it) before
        // the operations (each then made on both sides) or after them
        // array-first / array-last: an Array of the first part (2 in a line,
        // 80 mm along Z: its copy is the first part's island too, by its
        // ids), before the operations (those on the first part then brought
        // before it, as the app does) or after them
        let mode = std::env::var("FORME_BATTERY").unwrap_or_default();
        let mirror = crate::doc::Mirror { axis: 0, offset: 0.0, flip: false, single: false };
        let array = crate::doc::Array { sources: vec![b, 2, s, 4], kind: crate::doc::ArrayKind::Line, count: 2, step: DVec3::new(0.0, 0.0, 80.0), count2: 1, step2: DVec3::ZERO, axis: 1, centre: DVec3::ZERO, angle: 360.0 };
        if mode == "mirror-first" {
            base.add_mirror(mirror);
        }
        if mode == "array-first" {
            base.add_array(array.clone());
        }
        assert_eq!(islands_against_one(&base), Vec::<String>::new());
        let only = std::env::var("FORME_COMBO").ok();
        let (mut bad, mut n) = (vec![], 0);
        let t0 = Instant::now();
        for i in 0..one.len() {
            for j in 0..one.len() {
                if i == j {
                    continue;
                }
                let name = format!("{} / {}", one[i].0, one[j].0);
                if only.as_ref().is_some_and(|o| !name.contains(o.as_str())) {
                    continue;
                }
                let mut d = base.clone();
                (one[i].1)(&mut d);
                (two[j].1)(&mut d);
                (one[j].1)(&mut d);
                (two[i].1)(&mut d);
                if mode == "mirror-last" {
                    d.add_mirror(mirror);
                }
                if mode == "array-first" {
                    d.keep_arrays();
                }
                if mode == "array-last" {
                    d.add_array(array.clone());
                }
                let diff = islands_against_one(&d);
                n += 1;
                if !diff.is_empty() {
                    eprintln!("DIFFERENT {name}: {}", diff.join(" · "));
                    // FORME_DUMP=dir: the document, for `islands_probe`
                    if let Ok(dir) = std::env::var("FORME_DUMP") {
                        std::fs::write(format!("{dir}/islands-{i}-{j}.fnurbs"), d.to_json()).ok();
                    }
                    bad.push(name);
                }
            }
        }
        eprintln!("{} of {n} pairs differ ({:.0} s)", bad.len(), t0.elapsed().as_secs_f64());
        assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
    }

    /// The tools one after another on the test part: every ordered pair of
    /// a list of operations (fillets, chamfers, insets, extrusions, moves,
    /// a tilt, the Knife), each pair built on its own; prints what fails.
    /// `FORME_COMBO=name` keeps the pairs whose names contain it.
    /// `FORME_BATTERY=mirror-first`: a Mirror across X 20 mm off the part
    /// before them (the part and its image apart, each step on both),
    /// `mirror-touch`: across Z on the part's back face (one part, faces
    /// across the plane), `mirror-last`: across X through the part after
    /// them. `array-last`: an Array of the whole part after them (2 in a
    /// line, 100 mm apart: twice the pair), `array-first`: the Array first
    /// and both tools made on its copy, brought back to the original as the
    /// app does (`Doc::keep_arrays`: twice the pair again).
    #[test]
    #[ignore]
    fn combo_battery() {
        let (mut base, b, s) = test_part();
        let only = std::env::var("FORME_COMBO").ok();
        let mode = std::env::var("FORME_BATTERY").unwrap_or_default();
        let step = DVec3::new(100.0, 0.0, 0.0);
        let plain_ops = combo_ops(b, 2, s, 4, DVec3::ZERO);
        let ops = combo_ops(b, 2, s, 4, if mode == "array-first" { step } else { DVec3::ZERO });
        let plain = base.clone();
        let array = |d: &mut Doc| {
            d.add_array(crate::doc::Array { sources: vec![b, 2, s, 4], kind: crate::doc::ArrayKind::Line, count: 2, step, count2: 1, step2: DVec3::ZERO, axis: 1, centre: DVec3::ZERO, angle: 360.0 });
        };
        match mode.as_str() {
            "mirror-first" => { base.add_mirror(crate::doc::Mirror { axis: 0, offset: -40.0, flip: false, single: false }); }
            "mirror-touch" => { base.add_mirror(crate::doc::Mirror { axis: 2, offset: -15.0, flip: false, single: false }); }
            "array-first" => array(&mut base),
            _ => {}
        }
        let twice = mode == "mirror-first" || mode.starts_with("array");
        // an operation made on a copy goes back to the original
        let kept = |d: &mut Doc| {
            if mode == "array-first" {
                d.keep_arrays();
            }
        };
        let last = |d: &mut Doc| {
            if mode == "mirror-last" {
                d.add_mirror(crate::doc::Mirror { axis: 0, offset: 0.0, flip: false, single: false });
            }
            if mode == "array-last" {
                array(d);
            }
        };
        let mut bad = vec![];
        let mut n = 0;
        let mut unstable = 0;
        for (ai, (an, a)) in ops.iter().enumerate() {
            // each alone first
            let mut d = base.clone();
            a(&mut d);
            kept(&mut d);
            last(&mut d);
            let m = build(&d, true, false, &[], &mut Cache::default());
            if !m.errors.is_empty() {
                eprintln!("ALONE {an}: {:?}", m.errors);
                bad.push(format!("{an}: {:?}", m.errors));
                continue;
            }
            for (bi, (bn, bo)) in ops.iter().enumerate() {
                if an == bn {
                    continue;
                }
                let name = format!("{an} → {bn}");
                if only.as_ref().is_some_and(|o| !name.contains(o.as_str())) {
                    continue;
                }
                let mut d = base.clone();
                a(&mut d);
                kept(&mut d);
                bo(&mut d);
                kept(&mut d);
                last(&mut d);
                let t = Instant::now();
                let m = build(&d, true, false, &[], &mut Cache::default());
                n += 1;
                // a shape added far away changes nothing of the part (its
                // faces of one surface were made one when they came to be,
                // not at the next union anywhere: a face named meanwhile
                // would be gone then)
                if mode.is_empty() || mode == "stable" {
                    let mut far = d.clone();
                    far.add(Op::Add, Prim::Box { x: 10.0, y: 10.0, z: 10.0 }, DVec3::new(300.0, 0.0, 0.0), DQuat::IDENTITY);
                    let (x, y) = (digest(&m), digest(&build(&far, true, false, &[], &mut Cache::default())));
                    if x.1 + 6 != y.1 || x.2 + 12 != y.2 {
                        eprintln!("UNSTABLE {name}: faces {} then {} − 6, edges {} then {} − 12", x.1, y.1, x.2, y.2);
                        unstable += 1;
                    }
                }
                // the part and its image apart: twice the pair made with no
                // Mirror (a step made on one side only would show here)
                if twice && m.errors.is_empty() {
                    let mut p = plain.clone();
                    (plain_ops[ai].1)(&mut p);
                    (plain_ops[bi].1)(&mut p);
                    let (x, y) = (digest(&m), digest(&build(&p, true, false, &[], &mut Cache::default())));
                    if y.0.is_empty() && (x.1 != 2 * y.1 || x.2 != 2 * y.2 || (x.3 - 2.0 * y.3).abs() > 1e-3 * y.3.abs().max(1.0)) {
                        eprintln!("LOPSIDED {name}: faces {} / 2 × {}, edges {} / 2 × {}, volume {:.3} / 2 × {:.3}", x.1, y.1, x.2, y.2, x.3, y.3);
                        // FORME_DUMP=dir: both documents, for `islands_probe`
                        if let Ok(dir) = std::env::var("FORME_DUMP") {
                            std::fs::write(format!("{dir}/lop-{n}-mirror.fnurbs"), d.to_json()).ok();
                            std::fs::write(format!("{dir}/lop-{n}-plain.fnurbs"), p.to_json()).ok();
                        }
                        bad.push(format!("{name}: not twice the part"));
                    }
                }
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
        eprintln!("{} of {n} pairs failed, {unstable} changed by a shape added far away", bad.len());
        assert!(bad.is_empty(), "{} failed:\n{}", bad.len(), bad.join("\n"));
    }

    /// Found by the tools' battery with a Mirror (21 nov.). With the part
    /// and its image apart, every pair of tools gives twice what it gives
    /// with no Mirror: an edge tilted next to a hole's rounded mouth (the
    /// move and its image made before the fillet, together), and an edge
    /// tilted after its own fillet (its image was passed over in silence:
    /// neither was found, which counted as "the same"). And a curved face
    /// extruded leaves its top one face at once, not at the next union made
    /// anywhere (a face named meanwhile was gone then).
    #[test]
    fn mirror_side_moves_and_faces_made_one_at_once() {
        let (base, b, s) = test_part();
        let ops = combo_ops(b, 2, s, 4, DVec3::ZERO);
        let op = |name: &str| &ops.iter().find(|(n, _)| *n == name).unwrap().1;
        for (first, second) in [("fillet hole mouth", "tilt top-left"), ("tension fillet top -X", "tilt top-left"), ("fillet top-front", "extrude boss side")] {
            let mut plain = base.clone();
            op(first)(&mut plain);
            op(second)(&mut plain);
            let mut both = base.clone();
            both.add_mirror(crate::doc::Mirror { axis: 0, offset: -40.0, flip: false, single: false });
            op(first)(&mut both);
            op(second)(&mut both);
            let (x, y) = (digest(&build(&plain, true, false, &[], &mut Cache::default())), digest(&build(&both, true, false, &[], &mut Cache::default())));
            assert!(x.0.is_empty() && y.0.is_empty(), "{first} then {second}: {:?}, with a Mirror {:?}", x.0, y.0);
            assert_eq!((y.1, y.2), (2 * x.1, 2 * x.2), "{first} then {second}: faces and edges, twice with a Mirror");
            assert!((y.3 - 2.0 * x.3).abs() < 1e-3 * x.3, "{first} then {second}: volume {} with a Mirror, {} without", y.3, x.3);
            // a shape added far away changes nothing of the part
            let mut far = plain.clone();
            far.add(Op::Add, Prim::Box { x: 10.0, y: 10.0, z: 10.0 }, DVec3::new(300.0, 0.0, 0.0), DQuat::IDENTITY);
            let z = digest(&build(&far, true, false, &[], &mut Cache::default()));
            assert_eq!((z.1, z.2), (x.1 + 6, x.2 + 12), "{first} then {second}: faces and edges after a shape added far away");
        }
        // the boss's side extruded: its top is one face (the ring made with
        // it and the boss's own top)
        let mut d = base.clone();
        op("extrude boss side")(&mut d);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert_eq!(m.errors, Vec::<String>::new());
        let tops = m.faces.iter().filter(|f| f.kind == SurfKind::Plane && (f.point[1] - 30.0).abs() < 1e-6 && f.normal[1] > 0.9).count();
        assert_eq!(tops, 1, "the boss's top after its side is extruded");
    }

    /// Found by the tools' battery with an Array (21 nov.). A copy carries
    /// the original's tags, so a copy's fillet with tension found the
    /// original's faces, conic already, among its own: it made them conic
    /// again (their exact surface lost a little more at each copy) and said
    /// "tension on 1 of 2 faces" where that could not be done again (after
    /// an Extrude at the fillet's end, next to another fillet). They are
    /// left alone now: with the Array, twice the part and no error.
    #[test]
    fn tension_on_the_copies_of_an_array() {
        let (base, b, s) = test_part();
        let ops = combo_ops(b, 2, s, 4, DVec3::ZERO);
        let op = |name: &str| &ops.iter().find(|(n, _)| *n == name).unwrap().1;
        for (first, second) in [("fillet top-front", "tension fillet top -X"), ("tension fillet top -X", "fillet top-front"), ("tension fillet top -X", "extrude front")] {
            let mut plain = base.clone();
            op(first)(&mut plain);
            op(second)(&mut plain);
            let mut both = plain.clone();
            both.add_array(crate::doc::Array { sources: vec![b, 2, s, 4], kind: crate::doc::ArrayKind::Line, count: 2, step: DVec3::new(100.0, 0.0, 0.0), count2: 1, step2: DVec3::ZERO, axis: 1, centre: DVec3::ZERO, angle: 360.0 });
            let (x, y) = (digest(&build(&plain, true, false, &[], &mut Cache::default())), digest(&build(&both, true, false, &[], &mut Cache::default())));
            assert!(x.0.is_empty() && y.0.is_empty(), "{first} then {second}: {:?}, with an Array {:?}", x.0, y.0);
            assert_eq!((y.1, y.2), (2 * x.1, 2 * x.2), "{first} then {second}: faces and edges, twice with an Array");
            assert!((y.3 - 2.0 * x.3).abs() < 1e-3 * x.3, "{first} then {second}: volume {} with an Array, {} without", y.3, x.3);
        }
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
        let top = |b: u32, dy: f64| Tweak { comp: Comp::Face(FaceRef { tag: tag(b, 2), at: DVec3::new(0.0, 20.0, 0.0) }), delta: DVec3::new(0.0, dy, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true };
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
        d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(b, 0), at: DVec3::new(20.0, 10.0, 0.0) }), delta: DVec3::new(5.0, 0.0, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
        cases.push(("side out, vertical fillet", d, 20.0, r));
        // the same, the top front edge raised (the front tilts)
        let mut d = Doc::default();
        let b = d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let r = d.add_round(4.0, false, vec![EdgeRef { faces: [tag(b, 0), tag(b, 4)], at: DVec3::new(20.0, 10.0, 15.0) }]);
        d.add_tweak(Tweak { comp: Comp::Edge(EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(0.0, 20.0, 15.0) }), delta: DVec3::new(0.0, 3.0, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
        cases.push(("top front edge up, vertical fillet", d, 23.0, r));
        // the test part, its outer vertical edges filleted, its top moved
        // up (the hole, the boss, the slot follow)
        let (mut d, b, _) = test_part();
        let r = d.add_round(3.0, false, [(20.0, 15.0, 0, 4), (20.0, -15.0, 0, 5), (-20.0, 15.0, 1, 4), (-20.0, -15.0, 1, 5)].iter().map(|&(x, z, a, c)| EdgeRef { faces: [tag(b, a), tag(b, c)], at: DVec3::new(x, 10.0, z) }).collect());
        d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(b, 0), at: DVec3::new(20.0, 10.0, 0.0) }), delta: DVec3::new(4.0, 0.0, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
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
            d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: t, at }), delta: DVec3::new(0.0, dy, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
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
        d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(s, 1), at: DVec3::new(10.0, 30.0, 0.0) }), delta: DVec3::new(0.0, 2.0, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
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
        let mv = d.add_tweak(Tweak { comp: Comp::Face(FaceRef { tag: tag(b, 0), at: DVec3::new(20.0, 10.0, 0.0) }), delta: DVec3::new(5.0, 0.0, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
        assert_eq!(d.feature(mv).unwrap().kind_label(), "Move face");
        let mut cache = Cache::default();
        let m = build(&d, true, false, &[], &mut cache);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let side = m.faces.iter().find(|f| f.tag == Some(FaceTag { feature: b, sub: 0 })).unwrap();
        assert!((side.point[0] - 25.0).abs() < 1e-6, "{:?}", side.point);
        // the front top edge (top, +Z) raised 4 mm: the top tilts
        d.add_tweak(Tweak { comp: Comp::Edge(EdgeRef { faces: [tag(b, 2), tag(b, 4)], at: DVec3::new(2.5, 20.0, 15.0) }), delta: DVec3::new(0.0, 4.0, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
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
        d.add_tweak(Tweak { comp: Comp::Edge(EdgeRef { faces: [tag(b, 3), tag(b, 4)], at: DVec3::new(2.5, 0.0, 15.0) }), delta: DVec3::new(0.0, 0.0, 4.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
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
        d.add_tweak(Tweak { comp: Comp::Vertex(DVec3::new(10.0, 20.0, 10.0)), delta: DVec3::new(0.0, 4.0, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
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
        d.add_tweak(Tweak { comp: Comp::Vertex(DVec3::new(10.0, 0.0, 10.0)), delta: DVec3::new(3.0, 0.0, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
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
        d.feature_mut(l).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, -0.2], [-0.2, -0.2], [-0.2, 0.5], [-0.5, 0.5]], smooth: vec![false; 6], corner: vec![], tension: vec![], open: false, spline: false });
        d.add_tweak(Tweak { comp: Comp::Vertex(DVec3::new(10.0, 10.0, -10.0)), delta: DVec3::new(0.0, 3.0, 0.0) , more: vec![], scale: DVec3::ONE, copy: false, about: None, sweep: true });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!(m.positions.iter().any(|p| (p[1] - 13.0).abs() < 1e-3), "the corner is up");
        let top = m.faces.iter().find(|f| f.tag == Some(FaceTag { feature: l, sub: 1 })).unwrap();
        assert!(!matches!(top.kind, SurfKind::Plane), "the top bends: {:?}", top.kind);
    }

    /// The test part of the app (block, hole, boss, slot).
    #[test]
    fn only_selected_edges() {
        // Romain 20 nov., 10 h 54: "Only Selected" rounds the edge picked
        // alone, not the pill's whole tangent outline OpenCascade carries
        // the fillet along; it ends square where the edge ends
        let mut d = Doc::default();
        let p = d.add(Op::Add, Prim::Profile { x: 60.0, y: 20.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        d.feature_mut(p).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![10.0; 4], tension: vec![], open: false, spline: false });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        // the top edge along +Z's straight side, then the top arc at +X
        let near = |at: DVec3| {
            let mid = |e: &Edge| {
                let p = e.pts.iter().map(|q| DVec3::from_array(q.map(|v| v as f64))).min_by(|a, b| a.distance(at).partial_cmp(&b.distance(at)).unwrap()).unwrap();
                let ends = (DVec3::from_array(e.pts[0].map(|v| v as f64)) + DVec3::from_array(e.pts[e.pts.len() - 1].map(|v| v as f64))) * 0.5;
                p.distance(at).min(ends.distance(at))
            };
            let e = m.edges.iter().min_by(|a, b| mid(a).partial_cmp(&mid(b)).unwrap()).unwrap();
            EdgeRef { faces: e.faces.map(|f| m.faces[f as usize].tag.unwrap().pack()), at }
        };
        for (top, span) in [(near(DVec3::new(0.0, 20.0, 10.0)), (-20.1, 20.1)), (near(DVec3::new(30.0, 20.0, 0.0)), (19.9, 30.1))] {
            for chamfer in [false, true] {
                for only in [false, true] {
                    let mut d2 = d.clone();
                    let id = d2.add_round(2.0, chamfer, vec![top.clone()]);
                    d2.feature_mut(id).unwrap().round.as_mut().unwrap().only = only;
                    let m2 = build(&d2, true, false, &[], &mut Cache::default());
                    let mine: Vec<SurfKind> = m2.faces.iter().filter(|f| f.tag.map_or(false, |t| t.feature == id)).map(|f| f.kind).collect();
                    eprintln!("chamfer {chamfer} only {only}: errors {:?} faces {} its faces {:?}", m2.errors, m2.faces.len(), mine);
                    assert!(m2.errors.is_empty(), "{:?}", m2.errors);
                    // its faces' box: along X the edge's length alone when only
                    let xs: Vec<f32> = m2.triangles.iter().enumerate().filter(|(t, _)| m2.faces[m2.tri_face[*t] as usize].tag.map_or(false, |g| g.feature == id)).flat_map(|(_, tri)| tri.iter().map(|&v| m2.positions[v as usize][0])).collect();
                    let (lo, hi) = xs.iter().fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
                    eprintln!("  x {lo} .. {hi}");
                    if only {
                        assert!(lo > span.0 && hi < span.1, "only the edge: {lo} {hi}");
                    } else {
                        assert!(lo < -25.0 && hi > 25.0, "the whole outline: {lo} {hi}");
                    }
                }
            }
        }
    }

    #[test]
    fn only_selected_in_a_pocket() {
        // Romain 20 nov., 10 h 47: "Only selected" on a pill pocket's rim:
        // the chamfer carried round the rim was kept on the other rim too
        // (it lay within the region), a slot-shaped hole there
        let mut d = Doc::default();
        d.add(Op::Add, Prim::Box { x: 60.0, y: 20.0, z: 40.0 }, DVec3::ZERO, DQuat::IDENTITY);
        let c = d.add(Op::Carve, Prim::Profile { x: 40.0, y: 10.5, z: 15.0 }, DVec3::new(0.0, 10.0, 0.0), DQuat::IDENTITY);
        d.feature_mut(c).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![7.5; 4], tension: vec![], open: false, spline: false });
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let at = DVec3::new(0.0, 20.0, 7.5);
        let e = m
            .edges
            .iter()
            .min_by(|a, b| {
                let dist = |e: &Edge| {
                    let p: Vec<DVec3> = e.pts.iter().map(|q| DVec3::from_array(q.map(|v| v as f64))).collect();
                    p.windows(2).map(|w| at.distance(w[0] + (w[1] - w[0]) * ((at - w[0]).dot(w[1] - w[0]) / (w[1] - w[0]).length_squared().max(1e-12)).clamp(0.0, 1.0))).fold(f64::MAX, f64::min)
                };
                dist(a).partial_cmp(&dist(b)).unwrap()
            })
            .unwrap();
        let rim = EdgeRef { faces: e.faces.map(|f| m.faces[f as usize].tag.unwrap().pack()), at };
        for chamfer in [true, false] {
            for only in [false, true] {
                let mut d2 = d.clone();
                let id = d2.add_round(5.2, chamfer, vec![rim.clone()]);
                d2.feature_mut(id).unwrap().round.as_mut().unwrap().only = only;
                let t0 = Instant::now();
                let m2 = build(&d2, true, false, &[], &mut Cache::default());
                assert!(m2.errors.is_empty(), "{:?}", m2.errors);
                let zs: Vec<f32> = m2.triangles.iter().enumerate().filter(|(t, _)| m2.faces[m2.tri_face[*t] as usize].tag.map_or(false, |g| g.feature == id)).flat_map(|(_, tri)| tri.iter().map(|&v| m2.positions[v as usize][2])).collect();
                let (lo, hi) = zs.iter().fold((f32::MAX, f32::MIN), |(a, b), z| (a.min(*z), b.max(*z)));
                eprintln!("chamfer {chamfer} only {only}: z {lo} .. {hi}, {:.2} s", t0.elapsed().as_secs_f64());
                if only {
                    assert!(lo > 2.0, "the other rim left as it was: {lo}");
                } else {
                    assert!(lo < -7.0, "the whole rim: {lo}");
                }
            }
        }
    }

    #[test]
    fn corner_of_four() {
        // Romain 20 nov., 9 h 56: a pocket carved with the same outline as
        // the cap standing on its end ("extrude en aller-retour"): the cap's
        // flat face is the pocket's end wall, its round side tangent to the
        // pocket's side wall, so four edges meet at a corner (the cap's foot,
        // the cap's upright edge, the pocket's upright corner, the pocket's
        // rim). OpenCascade rounds one or all four in one go, not two or
        // three; one at a time in some order it does; and all four after
        // the cap's top edge was rounded, made before that fillet.
        let mut d = Doc::default();
        let a = d.add(Op::Add, Prim::Box { x: 40.0, y: 10.0, z: 20.0 }, DVec3::new(15.0, 0.0, 0.0), DQuat::IDENTITY);
        let c = d.add(Op::Add, Prim::Cylinder { radius: 2.0, height: 10.0 }, DVec3::new(30.0, 10.0, 0.0), DQuat::IDENTITY);
        let k = d.add(Op::Carve, Prim::Box { x: 25.0, y: 18.0, z: 4.0 }, DVec3::new(17.5, 7.0, 0.0), DQuat::IDENTITY);
        let m = build(&d, true, false, &[], &mut Cache::default());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let foot = EdgeRef { faces: [tag(c, 0), tag(a, 2)], at: DVec3::new(32.0, 10.0, 0.0) };
        let upright = EdgeRef { faces: [tag(k, 0), tag(c, 0)], at: DVec3::new(30.0, 15.0, -2.0) };
        let corner = EdgeRef { faces: [tag(k, 0), tag(k, 5)], at: DVec3::new(30.0, 8.5, -2.0) };
        let rim = EdgeRef { faces: [tag(k, 5), tag(a, 2)], at: DVec3::new(17.5, 10.0, -2.0) };
        let top = EdgeRef { faces: [tag(k, 0), tag(c, 1)], at: DVec3::new(30.0, 20.0, 0.0) };
        let made = |pre: &[(EdgeRef, f64)], edges: &[&EdgeRef], what: &str| {
            let mut d = d.clone();
            for (e, r) in pre {
                d.add_round(*r, false, vec![e.clone()]);
            }
            let id = d.add_round(0.5, false, edges.iter().map(|e| (*e).clone()).collect());
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "{what}: {:?}", m.errors);
            assert!(!m.limits.iter().any(|l| l.0 == id), "{what}: made smaller {:?}", m.limits);
            assert!(m.faces.iter().any(|f| f.tag.is_some_and(|t| t.feature == id)), "{what}: no fillet face");
        };
        made(&[], &[&foot, &upright, &corner, &rim], "all four");
        made(&[], &[&foot], "the foot");
        made(&[], &[&foot, &rim], "the foot and the rim");
        made(&[], &[&upright, &corner], "the upright edge and the corner");
        made(&[(top.clone(), 0.9)], &[&foot, &upright, &corner, &rim], "all four after the top's fillet");
        // the top cut across first (as Romain's Loop cut): the rim in two
        // pieces, the piece by the corner named
        let mut d = Doc::default();
        let a = d.add(Op::Add, Prim::Box { x: 40.0, y: 10.0, z: 20.0 }, DVec3::new(15.0, 0.0, 0.0), DQuat::IDENTITY);
        let front = EdgeRef { faces: [tag(a, 2), tag(a, 5)], at: DVec3::new(15.0, 10.0, -10.0) };
        let back = EdgeRef { faces: [tag(a, 2), tag(a, 4)], at: DVec3::new(15.0, 10.0, 10.0) };
        d.add_knife(
            Knife {
                pts: vec![KnifePoint { edge: front, f: 0.4375, at: DVec3::new(17.5, 10.0, -10.0) }, KnifePoint { edge: back, f: 0.4375, at: DVec3::new(17.5, 10.0, 10.0) }],
                segs: vec![[0, 1]],
                remove_faces: vec![],
                remove_edges: vec![],
            },
            "Knife",
        );
        let c = d.add(Op::Add, Prim::Cylinder { radius: 2.0, height: 10.0 }, DVec3::new(30.0, 10.0, 0.0), DQuat::IDENTITY);
        let k = d.add(Op::Carve, Prim::Box { x: 25.0, y: 18.0, z: 4.0 }, DVec3::new(17.5, 7.0, 0.0), DQuat::IDENTITY);
        let foot = EdgeRef { faces: [tag(c, 0), tag(a, 2)], at: DVec3::new(32.0, 10.0, 0.0) };
        let upright = EdgeRef { faces: [tag(k, 0), tag(c, 0)], at: DVec3::new(30.0, 15.0, -2.0) };
        let corner = EdgeRef { faces: [tag(k, 0), tag(k, 5)], at: DVec3::new(30.0, 8.5, -2.0) };
        let rim = EdgeRef { faces: [tag(k, 5), tag(a, 2)], at: DVec3::new(23.75, 10.0, -2.0) };
        for (what, es) in [("cut: all four", vec![&foot, &upright, &corner, &rim]), ("cut: the rim's piece and the foot", vec![&rim, &foot])] {
            let mut d = d.clone();
            let id = d.add_round(0.5, false, es.iter().map(|e| (*e).clone()).collect());
            let m = build(&d, true, false, &[], &mut Cache::default());
            assert!(m.errors.is_empty(), "{what}: {:?}", m.errors);
            assert!(m.faces.iter().any(|f| f.tag.is_some_and(|t| t.feature == id)), "{what}: no fillet face");
        }
    }

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
        l.feature_mut(li).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, -0.2], [-0.2, -0.2], [-0.2, 0.5], [-0.5, 0.5]], smooth: vec![false, false, false, true, false, false], corner: vec![], tension: vec![], open: false, spline: false });
        cases.push(("sketch L, curve".into(), l, FaceRef { tag: tag(li, 1), at: DVec3::new(-7.0, 10.0, -7.0) }));
        let mut t = Doc::default();
        let ti = t.add(Op::Add, Prim::Profile { x: 30.0, y: 6.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        t.feature_mut(ti).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.0, 0.5]], smooth: vec![false; 3], corner: vec![], tension: vec![], open: false, spline: false });
        cases.push(("sketch triangle".into(), t, FaceRef { tag: tag(ti, 1), at: DVec3::new(0.0, 6.0, -3.0) }));
        let mut r = Doc::default();
        let ri = r.add(Op::Add, Prim::Profile { x: 30.0, y: 6.0, z: 20.0 }, DVec3::ZERO, DQuat::IDENTITY);
        r.feature_mut(ri).unwrap().profile = Some(crate::doc::Profile { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], smooth: vec![false; 4], corner: vec![3.0, 3.0, -2.0, 6.0], tension: vec![], open: false, spline: false });
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

#[cfg(test)]
mod crash_probe {
    use crate::client::Kernel;
    use crate::doc::Doc;
    use std::time::Duration;

    #[test]
    #[ignore]
    fn crash_probe() {
        // FORME_DOC through the kernel process (FORME_NURBS_KERNEL): a step
        // that crashes OpenCascade is skipped and the build goes on; prints
        // the errors of the features from FORME_FROM on
        let Ok(p) = std::env::var("FORME_DOC") else { return };
        let d = Doc::from_json(&std::fs::read_to_string(p).unwrap()).unwrap();
        let from: u32 = std::env::var("FORME_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        let k = Kernel::start(crate::client::kernel_exe());
        let t0 = std::time::Instant::now();
        k.submit(1, &d, true, false);
        let b = k.wait_for(1, Duration::from_secs(120)).expect("an answer");
        let names: Vec<String> = d.features.iter().filter(|f| f.id >= from).map(|f| f.name.clone()).collect();
        let errs: Vec<&String> = b.model.errors.iter().filter(|e| names.iter().any(|n| e.starts_with(&format!("{n}:")))).collect();
        eprintln!("CRASH {:.2} s: faces {} errors {:?}", t0.elapsed().as_secs_f64(), b.model.faces.len(), errs);
    }
}

#[cfg(test)]
mod vertex4_probe {
    use super::*;
    use crate::doc::*;
    use glam::DVec3;

    #[test]
    #[ignore]
    fn vertex4_probe() {
        // FORME_DOC: the scene of 20 nov. 9 h 56; the four edges round the
        // vertex (239.485, 33.17, -52.33), filleted in every subset.
        let Ok(p) = std::env::var("FORME_DOC") else { return };
        let mut d = Doc::from_json(&std::fs::read_to_string(p).unwrap()).unwrap();
        if let Ok(modes) = std::env::var("FORME_LAST") {
            // FORME_KEEP: the features before that id only, and the last
            if let Some(keep) = std::env::var("FORME_KEEP").ok().and_then(|v| v.parse::<u32>().ok()) {
                let last = d.features.last().unwrap().clone();
                d.features.retain(|f| f.id < keep);
                d.features.push(last);
            }
            // the last step as it is, made a chamfer (c) or a fillet (f), with or
            // without tension (t0), blend (b0), at a size (s0.5): "c,f,c s0.2"
            for m in modes.split(',') {
                let mut d2 = d.clone();
                let r = d2.features.last_mut().unwrap().round.as_mut().unwrap();
                for w in m.split_whitespace() {
                    match w {
                        "c" => r.chamfer = true,
                        "f" => r.chamfer = false,
                        "t0" => r.tension = 0.0,
                        "b0" => r.blend = false,
                        "o0" => r.only = false,
                        "o1" => r.only = true,
                        _ if w.starts_with('s') => r.size = w[1..].parse().unwrap(),
                        _ if w.starts_with('e') => {
                            let keep: Vec<usize> = w[1..].chars().map(|c| c.to_digit(10).unwrap() as usize).collect();
                            r.edges = keep.iter().map(|&i| r.edges[i].clone()).collect();
                        }
                        _ => {}
                    }
                }
                let last = d2.features.last().unwrap().clone();
                // FORME_BEFORE: the last step moved before that id
                if let Some(b) = std::env::var("FORME_BEFORE").ok().and_then(|v| v.parse::<u32>().ok()) {
                    let l = d2.features.pop().unwrap();
                    let at = d2.features.iter().position(|f| f.id == b).unwrap();
                    d2.features.insert(at, l);
                }
                let t0 = std::time::Instant::now();
                let mut cache = Cache::default();
                let m2 = build(&d2, true, false, &[], &mut cache);
                let errs: Vec<&String> = m2.errors.iter().filter(|s| s.starts_with(&format!("{}:", last.name)) || s.starts_with("Chamfer") || s.starts_with("Fillet 8")).collect();
                let lim: Vec<_> = m2.limits.iter().filter(|l| l.0 == last.id).collect();
                let faces = m2.faces.iter().filter(|f| f.tag.map_or(false, |t| t.feature == last.id)).count();
                eprintln!("LAST [{m}] {:.2} s: errors {:?} limits {:?} faces {faces}", t0.elapsed().as_secs_f64(), errs, lim);
            }
            return;
        }
        let keep: u32 = std::env::var("FORME_KEEP").ok().and_then(|v| v.parse().ok()).unwrap_or(102);
        d.features.retain(|f| f.id < keep);
        let t = |f: u32, s: u32| ((f as i64) << 16) | s as i64;
        let e = [
            EdgeRef { faces: [t(83, 3), t(80, 1)], at: DVec3::new(241.837, 33.168, -50.061) },
            EdgeRef { faces: [t(90, 5), t(83, 3)], at: DVec3::new(239.485, 38.718, -52.328) },
            EdgeRef { faces: [t(90, 5), t(89, 4)], at: DVec3::new(239.485, 31.666, -52.328) },
            EdgeRef { faces: [t(89, 4), t(80, 1)], at: DVec3::new(227.204, 33.168, -52.103) },
            EdgeRef { faces: [t(90, 5), t(83, 1)], at: DVec3::new(239.52782, 45.167965, -50.01887) },
        ];
        let names = ["base arc", "cap vertical", "pocket corner", "pocket rim"];
        let sizes: Vec<f64> = std::env::var("FORME_SIZES").ok().map(|s| s.split(',').map(|x| x.parse().unwrap()).collect()).unwrap_or(vec![0.3, 0.8]);
        let mut cache = Cache::default();
        if let Ok(plan) = std::env::var("FORME_PLAN") {
            // steps "0123@0.5b;1@0.3": the edges by number, the size, b = blend
            for case in plan.split('|') {
                let mut d2 = d.clone();
                let mut ids = vec![];
                for st in case.split(';') {
                    let (es, rest) = st.split_once('@').unwrap();
                    let blend = rest.contains('b');
                    let chamfer = rest.contains('c');
                    let size: f64 = rest.trim_end_matches(['b', 'c']).parse().unwrap();
                    let edges: Vec<EdgeRef> = es.chars().map(|c| e[c.to_digit(10).unwrap() as usize].clone()).collect();
                    let id = d2.add_round(size, chamfer, edges);
                    d2.features.last_mut().unwrap().round.as_mut().unwrap().blend = blend;
                    ids.push(id);
                }
                let t0 = std::time::Instant::now();
                if std::env::var("FORME_FRESH").is_ok() {
                    cache = Cache::default();
                }
                let m = build(&d2, true, false, &[], &mut cache);
                eprintln!("PLAN time {:.2} s", t0.elapsed().as_secs_f64());
                let n0 = d.features.len();
                let errs: Vec<&String> = m.errors.iter().filter(|s| d2.features[n0..].iter().any(|f| s.starts_with(&format!("{}:", f.name)))).collect();
                let lim: Vec<_> = m.limits.iter().filter(|l| ids.contains(&l.0)).collect();
                let rounds = m.faces.iter().filter(|f| f.tag.map_or(false, |t| ids.contains(&t.feature))).count();
                eprintln!("PLAN {case}: errors {:?} limits {:?} faces {} (fillet faces {rounds})", errs, lim, m.faces.len());
            }
            return;
        }
        if std::env::var("FORME_SEQ").is_ok() {
            // each edge its own step, in every order of the subsets of two and more
            let blend = std::env::var("FORME_BLEND").is_ok();
            let orders: Vec<Vec<usize>> = vec![vec![0, 1], vec![1, 0], vec![0, 2], vec![2, 0], vec![0, 3], vec![3, 0], vec![1, 2], vec![2, 1], vec![1, 3], vec![3, 1], vec![2, 3], vec![3, 2], vec![0, 1, 2, 3], vec![3, 2, 1, 0], vec![1, 3, 0, 2], vec![0, 2, 1, 3]];
            for o in orders {
                for &size in &sizes {
                    let mut d2 = d.clone();
                    let mut ids = vec![];
                    for &i in &o {
                        let id = d2.add_round(size, false, vec![e[i].clone()]);
                        d2.features.last_mut().unwrap().round.as_mut().unwrap().blend = blend;
                        ids.push(id);
                    }
                    let m = build(&d2, true, false, &[], &mut cache);
                    let errs: Vec<&String> = m.errors.iter().filter(|s| s.starts_with("Fillet") && !s.contains("not found") && !s.contains("Fillet 6")).collect();
                    let lim: Vec<_> = m.limits.iter().filter(|l| ids.contains(&l.0)).collect();
                    let set: Vec<&str> = o.iter().map(|&i| names[i]).collect();
                    eprintln!("SEQ {:?} r {size}: errors {:?} limits {:?} faces {}", set, errs, lim, m.faces.len());
                }
            }
            return;
        }
        for mask in 1u32..16 {
            for &size in &sizes {
                let mut d2 = d.clone();
                let edges: Vec<EdgeRef> = (0..4).filter(|i| mask & (1 << i) != 0).map(|i| e[i].clone()).collect();
                let id = d2.add_round(size, false, edges);
                let m = build(&d2, true, false, &[], &mut cache);
                let errs: Vec<&String> = m.errors.iter().filter(|s| s.starts_with("Fillet")).collect();
                let lim = m.limits.iter().find(|l| l.0 == id).map(|l| l.1);
                let set: Vec<&str> = (0..4).filter(|i| mask & (1 << i) != 0).map(|i| names[i]).collect();
                eprintln!("{:?} r {size}: errors {:?} limit {:?} faces {}", set, errs, lim, m.faces.len());
            }
        }
    }
}
