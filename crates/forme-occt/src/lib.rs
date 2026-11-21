//! Forme's thin layer over OpenCascade (cpp/forme_occt.cpp): primitives,
//! booleans with their junction edges, fillets and chamfers by edge index,
//! meshing. OpenCascade's errors are caught on the C++ side and come back as
//! `Err(message)`: nothing here aborts the process on a failed operation.
//!
//! Conventions (Forme's): shapes centred on the origin, round shapes along
//! +Y, millimetres. Edges are numbered 1.. in the shape's edge map (each
//! edge once).

use std::ffi::CStr;
use std::os::raw::{c_char, c_double, c_int};

#[repr(C)]
struct FShape {
    _p: [u8; 0],
}
#[repr(C)]
struct FMesh {
    _p: [u8; 0],
}
#[repr(C)]
struct FLines {
    _p: [u8; 0],
}
#[repr(C)]
struct FBuf {
    _p: [u8; 0],
}

extern "C" {
    fn focc_last_error() -> *const c_char;
    fn focc_free(s: *mut FShape);
    fn focc_box(dx: c_double, dy: c_double, dz: c_double) -> *mut FShape;
    fn focc_cylinder(r: c_double, h: c_double) -> *mut FShape;
    fn focc_sphere(r: c_double) -> *mut FShape;
    fn focc_torus(big: c_double, small: c_double) -> *mut FShape;
    fn focc_prism(xy: *const c_double, smooth: *const c_int, n: c_int, h: c_double) -> *mut FShape;
    fn focc_prism_cut(xy: *const c_double, smooth: *const c_int, cut: *const c_int, abm: *const c_double, n: c_int, h: c_double) -> *mut FShape;
    fn focc_outline(o: *const c_double, n: *const c_double, xyz: *const c_double, count: c_int, closed: c_int, cut: *const c_int, abm: *const c_double) -> *mut FShape;
    fn focc_disc(o: *const c_double, n: *const c_double, r: c_double) -> *mut FShape;
    fn focc_fill(xyz: *const c_double, n: c_int, inner: *const c_double, m: c_int) -> *mut FShape;
    fn focc_compound(a: *const FShape, b: *const FShape) -> *mut FShape;
    fn focc_sheet_grid(fr: *const c_double, nx: c_int, nz: c_int, x0: c_double, z0: c_double, x1: c_double, z1: c_double, h: *const c_double, poly: *const c_double, n: c_int) -> *mut FShape;
    fn focc_extrude_edge(s: *const FShape, index: c_int, v: *const c_double, tag: i64) -> *mut FShape;
    fn focc_thicken(s: *const FShape, t: c_double, salt: i64) -> *mut FShape;
    fn focc_round(s: *const FShape, idx: *const c_int, n: c_int, r: c_double, fresh: i64) -> *mut FShape;
    fn focc_bevel(s: *const FShape, idx: *const c_int, n: c_int, d: c_double, fresh: i64) -> *mut FShape;
    fn focc_transform(s: *const FShape, m: *const c_double) -> *mut FShape;
    fn focc_fuse(a: *const FShape, b: *const FShape) -> *mut FShape;
    fn focc_cut(a: *const FShape, b: *const FShape) -> *mut FShape;
    fn focc_common(a: *const FShape, b: *const FShape) -> *mut FShape;
    fn focc_region_cut(xy: *const c_double, smooth: *const c_int, cut: *const c_int, abm: *const c_double, n: c_int) -> *mut FShape;
    fn focc_outline_wire(xy: *const c_double, smooth: *const c_int, cut: *const c_int, abm: *const c_double, n: c_int, open: c_int) -> *mut FShape;
    fn focc_revolve(s: *const FShape, o: *const c_double, d: *const c_double, angle: c_double) -> *mut FShape;
    fn focc_form(poles: *const c_double, nu: c_int, nv: c_int) -> *mut FShape;
    fn focc_loft(secs: *const *const FShape, n: c_int, solid: c_int, ruled: c_int) -> *mut FShape;
    fn focc_section_count(s: *const FShape) -> c_int;
    fn focc_section_edge(s: *const FShape, k: c_int) -> c_int;
    fn focc_edge_count(s: *const FShape) -> c_int;
    fn focc_edge_faces(s: *const FShape, out: *mut c_int, cap: c_int) -> c_int;
    fn focc_edge_sample(s: *const FShape, index: c_int, n: c_int, xyz: *mut c_double) -> c_int;
    fn focc_fillet_tension(s: *const FShape, before: *const FShape, tags: *const i64, rhos: *const c_double, n: c_int, done: *mut c_int, total: *mut c_int, tight: *mut c_double) -> *mut FShape;
    fn focc_fillet(s: *const FShape, idx: *const c_int, n: c_int, r: c_double, fresh: i64) -> *mut FShape;
    fn focc_fillet_multi(s: *const FShape, idx: *const c_int, r: *const c_double, tags: *const i64, n: c_int, fresh: i64) -> *mut FShape;
    fn focc_chamfer(s: *const FShape, idx: *const c_int, n: c_int, d: c_double, fresh: i64) -> *mut FShape;
    fn focc_inset_face(s: *const FShape, face: c_int, d: c_double) -> *mut FShape;
    fn focc_extrude(s: *const FShape, dx: c_double, dy: c_double, dz: c_double) -> *mut FShape;
    fn focc_thicken_face(s: *const FShape, index: c_int, t: c_double, base: i64, top: i64, side: i64) -> *mut FShape;
    fn focc_move_face(s: *const FShape, index: c_int, d: c_double) -> *mut FShape;
    fn focc_imprint(s: *const FShape, tool: *const FShape) -> *mut FShape;
    fn focc_edge_point(s: *const FShape, edge: c_int, f: f64, xyz: *mut f64) -> c_int;
    fn focc_split(s: *const FShape, pts: *const f64, n: c_int, segs: *const c_int, nseg: c_int) -> *mut FShape;
    fn focc_remove_faces(s: *const FShape, idx: *const c_int, n: c_int) -> *mut FShape;
    fn focc_defeature(s: *const FShape, idx: *const c_int, n: c_int) -> *mut FShape;
    fn focc_shell(s: *const FShape, idx: *const c_int, n: c_int, t: c_double, out: c_int, base: i64, arc: c_int) -> *mut FShape;
    fn focc_piece(s: *const FShape, idx: *const c_int, n: c_int, m: *const c_double, tag: i64) -> *mut FShape;
    fn focc_remove_edges(s: *const FShape, idx: *const c_int, n: c_int) -> *mut FShape;
    fn focc_face_distance(s: *const FShape, face: c_int, p: *const c_double) -> c_double;
    fn focc_vertex_points(s: *const FShape, xyz: *mut c_double, cap: c_int) -> c_int;
    fn focc_sub_vertices(s: *const FShape, kind: c_int, index: c_int, out: *mut c_int, cap: c_int) -> c_int;
    fn focc_move_vertices(s: *const FShape, idx: *const c_int, xyz: *const c_double, n: c_int) -> *mut FShape;
    fn focc_move_lumps(s: *const FShape, idx: *const c_int, n: c_int, m: *const c_double) -> *mut FShape;
    fn focc_lumps_of(s: *const FShape, solids: c_int) -> *mut FShape;
    fn focc_face_count(s: *const FShape) -> c_int;
    fn focc_face_info(s: *const FShape, index: c_int, out: *mut c_double) -> c_int;
    fn focc_face_box(s: *const FShape, index: c_int, out: *mut c_double) -> c_int;
    fn focc_lump_boxes(s: *const FShape, out: *mut c_double, cap: c_int) -> c_int;
    fn focc_simplified(s: *const FShape) -> *mut FShape;
    fn focc_keep_lines(tags: *const i64, n: c_int);
    fn focc_set_tags(s: *mut FShape, tags: *const i64, n: c_int);
    fn focc_face_tag(s: *const FShape, index: c_int) -> i64;
    fn focc_topo(s: *const FShape, defl: c_double, angle: c_double) -> *mut FBuf;
    fn focc_buf_len(b: *const FBuf) -> c_int;
    fn focc_buf_data(b: *const FBuf) -> *const u8;
    fn focc_buf_free(b: *mut FBuf);
    fn focc_quad_topo(s: *const FShape) -> *mut FBuf;
    fn focc_edge_on_face(s: *const FShape, face: c_int, edge: c_int, reversed: c_int, n: c_int, t: *const c_double, xyz: *mut c_double, uv: *mut c_double) -> c_int;
    fn focc_face_project(s: *const FShape, face: c_int, n: c_int, xyz: *const c_double, out: *mut c_double) -> c_int;
    fn focc_face_eval(s: *const FShape, face: c_int, n: c_int, uv: *const c_double, out: *mut c_double) -> c_int;
    fn focc_volume(s: *const FShape) -> c_double;
    fn focc_is_valid(s: *const FShape) -> c_int;
    fn focc_mesh(s: *const FShape, defl: c_double, angle: c_double) -> *mut FMesh;
    fn focc_mesh_vertices(m: *const FMesh) -> c_int;
    fn focc_mesh_triangles(m: *const FMesh) -> c_int;
    fn focc_mesh_positions(m: *const FMesh) -> *const f32;
    fn focc_mesh_normals(m: *const FMesh) -> *const f32;
    fn focc_mesh_indices(m: *const FMesh) -> *const u32;
    fn focc_mesh_free(m: *mut FMesh);
    fn focc_lines(s: *const FShape, defl: c_double, angle: c_double) -> *mut FLines;
    fn focc_lines_count(l: *const FLines) -> c_int;
    fn focc_lines_counts(l: *const FLines) -> *const c_int;
    fn focc_lines_points(l: *const FLines) -> c_int;
    fn focc_lines_xyz(l: *const FLines) -> *const c_double;
    fn focc_lines_free(l: *mut FLines);
}

fn last_error() -> String {
    unsafe {
        let p = focc_last_error();
        if p.is_null() {
            return "OpenCascade error".into();
        }
        let s = CStr::from_ptr(p).to_string_lossy().into_owned();
        if s.is_empty() {
            "OpenCascade error".into()
        } else {
            s
        }
    }
}

/// How a corner of a prism's outline is cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutKind {
    Arc,
    Chamfer,
    /// A fillet with tension: a conic.
    Conic,
    /// A curve point with tension: its curve the conic of rho (given as the
    /// middle point's x) instead of the quadratic (an ellipse's quarter).
    Curve,
    /// A spline's span: the side from this point to the next a cubic
    /// Bezier, its inner controls given as the cut's start and middle.
    Span,
}

/// What `Shape::fillet_tension` did for one fillet: how many of its faces
/// took the tension, of how many.
#[derive(Clone, Copy, Debug)]
pub struct Tension {
    pub done: usize,
    pub total: usize,
    /// Its faces left round because a fillet running round them is bigger
    /// than their conic's tightest curve.
    pub tight: Option<Tight>,
}

/// A conic too tight for the fillet running round it.
#[derive(Clone, Copy, Debug)]
pub struct Tight {
    /// Its tightest radius of curvature at this tension.
    pub radius: f64,
    /// The size of the fillet running round it.
    pub need: f64,
    /// The tensions (rho) that would do, from and to.
    pub works: Option<(f64, f64)>,
}

/// A B-rep shape (solid) owned on the C++ side.
pub struct Shape(*mut FShape);

/// The tags of the faces whose outline is a line drawn on purpose (an
/// inset left as a line): from now on, on this thread, no union, cut or
/// clean-up makes such a face one with the face round it.
pub fn keep_lines(tags: &[i64]) {
    unsafe { focc_keep_lines(tags.as_ptr(), tags.len() as c_int) }
}

// the C++ side keeps no global state per shape: a shape can move to another thread
unsafe impl Send for Shape {}

impl Drop for Shape {
    fn drop(&mut self) {
        unsafe { focc_free(self.0) }
    }
}

fn shape(p: *mut FShape) -> Result<Shape, String> {
    if p.is_null() {
        Err(last_error())
    } else {
        Ok(Shape(p))
    }
}

/// Kind of an edge's curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Line,
    Circle,
    Curve,
}

/// An edge: its index in the shape, its kind, points along it.
#[derive(Clone, Debug)]
pub struct Edge {
    pub index: i32,
    pub kind: EdgeKind,
    pub pts: Vec<[f64; 3]>,
}

/// Kind of a face's surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfKind {
    Plane,
    Cylinder,
    Cone,
    Sphere,
    Torus,
    Other,
}

impl SurfKind {
    fn from_code(k: i32) -> SurfKind {
        match k {
            0 => SurfKind::Plane,
            1 => SurfKind::Cylinder,
            2 => SurfKind::Cone,
            3 => SurfKind::Sphere,
            4 => SurfKind::Torus,
            _ => SurfKind::Other,
        }
    }
}

/// What a face is: a point in its middle and the outward normal there, its
/// axis (a plane's: through that point along the normal) and its radius
/// (round faces).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceInfo {
    pub kind: SurfKind,
    pub point: [f64; 3],
    pub normal: [f64; 3],
    pub axis_origin: [f64; 3],
    pub axis_dir: [f64; 3],
    pub radius: f64,
}

impl FaceInfo {
    fn from_raw(kind: i32, d: &[f64]) -> FaceInfo {
        FaceInfo {
            kind: SurfKind::from_code(kind),
            point: [d[0], d[1], d[2]],
            normal: [d[3], d[4], d[5]],
            axis_origin: [d[6], d[7], d[8]],
            axis_dir: [d[9], d[10], d[11]],
            radius: d[12],
        }
    }
}

/// A face of a `Topo`: its tag (`Shape::set_tags`, -1 = none) and what it is.
#[derive(Clone, Debug)]
pub struct TopoFace {
    pub tag: i64,
    pub info: FaceInfo,
}

/// An edge of a `Topo`: its index in the shape (for fillets), the faces on
/// each side (indices in `Topo::faces`), its kind, whether it is a closed
/// surface's own seam, and a polyline along it.
#[derive(Clone, Debug)]
pub struct TopoEdge {
    pub index: i32,
    pub faces: [i32; 2],
    pub kind: EdgeKind,
    pub seam: bool,
    pub pts: Vec<[f64; 3]>,
}

/// Everything shown and picked: faces, triangles (each with its face) and
/// edges.
#[derive(Clone, Debug, Default)]
pub struct Topo {
    pub faces: Vec<TopoFace>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub tri_face: Vec<u32>,
    pub edges: Vec<TopoEdge>,
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let s = self.b.get(self.at..self.at + N)?;
        self.at += N;
        s.try_into().ok()
    }
    fn u32(&mut self) -> Option<u32> {
        self.take::<4>().map(u32::from_le_bytes)
    }
    fn i32(&mut self) -> Option<i32> {
        self.take::<4>().map(i32::from_le_bytes)
    }
    fn i64(&mut self) -> Option<i64> {
        self.take::<8>().map(i64::from_le_bytes)
    }
    fn f32(&mut self) -> Option<f32> {
        self.take::<4>().map(f32::from_le_bytes)
    }
    fn f64(&mut self) -> Option<f64> {
        self.take::<8>().map(f64::from_le_bytes)
    }
}

/// What a quad mesher walks: each face's wires (the outer one first) as
/// oriented edges, and each edge's range, ends and a sampling.
#[derive(Clone, Debug, Default)]
pub struct QuadTopo {
    pub faces: Vec<QuadFace>,
    /// `edges[i]` is edge i + 1 of the edge map.
    pub edges: Vec<QuadEdge>,
}

#[derive(Clone, Debug)]
pub struct QuadFace {
    pub kind: SurfKind,
    /// Each wire: (edge, reversed) in order around the face.
    pub wires: Vec<Vec<(i32, bool)>>,
}

#[derive(Clone, Debug)]
pub struct QuadEdge {
    /// Parameter range.
    pub t: [f64; 2],
    /// The vertices at t0 and t1 (1-based vertex map, 0 = none).
    pub v: [i32; 2],
    /// 0 line, 1 circle, 2 other.
    pub kind: i32,
    pub radius: f64,
    pub degenerated: bool,
    /// 33 points from t0 to t1, evenly in the parameter.
    pub pts: Vec<[f64; 3]>,
}

fn parse_quad_topo(b: &[u8]) -> Option<QuadTopo> {
    let mut r = Reader { b, at: 0 };
    let mut t = QuadTopo::default();
    let nf = r.u32()? as usize;
    for _ in 0..nf {
        let kind = SurfKind::from_code(r.i32()?);
        let nw = r.u32()? as usize;
        let mut wires = vec![];
        for _ in 0..nw {
            let n = r.u32()? as usize;
            let mut w = vec![];
            for _ in 0..n {
                w.push((r.i32()?, r.i32()? != 0));
            }
            wires.push(w);
        }
        t.faces.push(QuadFace { kind, wires });
    }
    let ne = r.u32()? as usize;
    for _ in 0..ne {
        let tt = [r.f64()?, r.f64()?];
        let v = [r.i32()?, r.i32()?];
        let kind = r.i32()?;
        let radius = r.f64()?;
        let degenerated = r.i32()? != 0;
        let mut pts = Vec::with_capacity(33);
        for _ in 0..33 {
            pts.push([r.f64()?, r.f64()?, r.f64()?]);
        }
        t.edges.push(QuadEdge { t: tt, v, kind, radius, degenerated, pts });
    }
    Some(t)
}

fn parse_topo(b: &[u8]) -> Option<Topo> {
    let mut r = Reader { b, at: 0 };
    let mut t = Topo::default();
    let nf = r.u32()? as usize;
    for _ in 0..nf {
        let tag = r.i64()?;
        let kind = r.i32()?;
        let mut d = [0.0; 13];
        for v in d.iter_mut() {
            *v = r.f64()?;
        }
        t.faces.push(TopoFace { tag, info: FaceInfo::from_raw(kind, &d) });
    }
    let nv = r.u32()? as usize;
    for _ in 0..nv {
        t.positions.push([r.f32()?, r.f32()?, r.f32()?]);
    }
    for _ in 0..nv {
        t.normals.push([r.f32()?, r.f32()?, r.f32()?]);
    }
    let nt = r.u32()? as usize;
    for _ in 0..nt {
        t.triangles.push([r.u32()?, r.u32()?, r.u32()?]);
    }
    for _ in 0..nt {
        t.tri_face.push(r.u32()?);
    }
    let ne = r.u32()? as usize;
    for _ in 0..ne {
        let index = r.i32()?;
        let faces = [r.i32()?, r.i32()?];
        let kind = match r.i32()? {
            0 => EdgeKind::Line,
            1 => EdgeKind::Circle,
            _ => EdgeKind::Curve,
        };
        let seam = r.i32()? != 0;
        let n = r.u32()? as usize;
        let mut pts = Vec::with_capacity(n);
        for _ in 0..n {
            pts.push([r.f64()?, r.f64()?, r.f64()?]);
        }
        t.edges.push(TopoEdge { index, faces, kind, seam, pts });
    }
    Some(t)
}

/// Triangles of a shape: per-face vertices (sharp edges stay sharp).
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

impl Shape {
    /// Box of full sizes dx, dy, dz.
    pub fn cuboid(dx: f64, dy: f64, dz: f64) -> Result<Shape, String> {
        shape(unsafe { focc_box(dx, dy, dz) })
    }
    /// Cylinder of radius r and full height h along Y.
    pub fn cylinder(r: f64, h: f64) -> Result<Shape, String> {
        shape(unsafe { focc_cylinder(r, h) })
    }
    pub fn sphere(r: f64) -> Result<Shape, String> {
        shape(unsafe { focc_sphere(r) })
    }
    /// Torus around Y: radius of the ring, radius of the tube.
    pub fn torus(big: f64, small: f64) -> Result<Shape, String> {
        shape(unsafe { focc_torus(big, small) })
    }

    /// Closed outline in the XY plane extruded along Z between -h and +h.
    /// A smooth point is rounded into a quadratic curve between the middles
    /// of its two edges (the Sketch tool's outlines, `nurbs_kernel::doc::Profile`).
    pub fn prism(points: &[[f64; 2]], smooth: &[bool], h: f64) -> Result<Shape, String> {
        let xy: Vec<f64> = points.iter().flatten().copied().collect();
        let sm: Vec<c_int> = (0..points.len()).map(|i| smooth.get(i).copied().unwrap_or(false) as c_int).collect();
        shape(unsafe { focc_prism(xy.as_ptr(), sm.as_ptr(), points.len() as c_int, h) })
    }
    /// The same with corners cut: per point, None, or (chamfer, start,
    /// middle, end): a fillet is the arc through the three points, a
    /// chamfer the straight side from start to end.
    pub fn prism_cut(points: &[[f64; 2]], smooth: &[bool], cuts: &[Option<(bool, [f64; 2], [f64; 2], [f64; 2])>], h: f64) -> Result<Shape, String> {
        let kinds: Vec<Option<(CutKind, [f64; 2], [f64; 2], [f64; 2])>> = cuts.iter().map(|c| c.map(|(ch, a, m, b)| (if ch { CutKind::Chamfer } else { CutKind::Arc }, a, m, b))).collect();
        Shape::prism_cuts(points, smooth, &kinds, h)
    }
    /// The same, each cut an arc, a chamfer or a conic (a fillet with
    /// tension: the point given is on it, the corner its middle pole).
    pub fn prism_cuts(points: &[[f64; 2]], smooth: &[bool], cuts: &[Option<(CutKind, [f64; 2], [f64; 2], [f64; 2])>], h: f64) -> Result<Shape, String> {
        let xy: Vec<f64> = points.iter().flat_map(|p| [p[0], p[1]]).collect();
        let sm: Vec<c_int> = (0..points.len()).map(|i| smooth.get(i).copied().unwrap_or(false) as c_int).collect();
        let ck: Vec<c_int> = (0..points.len()).map(|i| match cuts.get(i).copied().flatten() { Some((CutKind::Chamfer, ..)) => 2, Some((CutKind::Conic, ..)) => 3, Some((CutKind::Curve, ..)) => 4, Some((CutKind::Span, ..)) => 5, Some(_) => 1, None => 0 }).collect();
        let abm: Vec<f64> = (0..points.len()).flat_map(|i| match cuts.get(i).copied().flatten() { Some((_, a, m, b)) => [a[0], a[1], m[0], m[1], b[0], b[1]], None => [0.0; 6] }).collect();
        shape(unsafe { focc_prism_cut(xy.as_ptr(), sm.as_ptr(), ck.as_ptr(), abm.as_ptr(), points.len() as c_int, h) })
    }

    /// The outline of `prism_cuts` as a flat face in the XY plane (a closed
    /// curve's region, the Curves set's 2D booleans).
    pub fn region_cuts(points: &[[f64; 2]], smooth: &[bool], cuts: &[Option<(CutKind, [f64; 2], [f64; 2], [f64; 2])>]) -> Result<Shape, String> {
        let xy: Vec<f64> = points.iter().flat_map(|p| [p[0], p[1]]).collect();
        let sm: Vec<c_int> = (0..points.len()).map(|i| smooth.get(i).copied().unwrap_or(false) as c_int).collect();
        let ck: Vec<c_int> = (0..points.len()).map(|i| match cuts.get(i).copied().flatten() { Some((CutKind::Chamfer, ..)) => 2, Some((CutKind::Conic, ..)) => 3, Some((CutKind::Curve, ..)) => 4, Some((CutKind::Span, ..)) => 5, Some(_) => 1, None => 0 }).collect();
        let abm: Vec<f64> = (0..points.len()).flat_map(|i| match cuts.get(i).copied().flatten() { Some((_, a, m, b)) => [a[0], a[1], m[0], m[1], b[0], b[1]], None => [0.0; 6] }).collect();
        shape(unsafe { focc_region_cut(xy.as_ptr(), sm.as_ptr(), ck.as_ptr(), abm.as_ptr(), points.len() as c_int) })
    }

    /// The outline of `prism_cuts` as a wire in the XY plane, open (a
    /// path: its first point to its last, the ends plain) or closed.
    pub fn outline_wire(points: &[[f64; 2]], smooth: &[bool], cuts: &[Option<(CutKind, [f64; 2], [f64; 2], [f64; 2])>], open: bool) -> Result<Shape, String> {
        let xy: Vec<f64> = points.iter().flat_map(|p| [p[0], p[1]]).collect();
        let sm: Vec<c_int> = (0..points.len()).map(|i| smooth.get(i).copied().unwrap_or(false) as c_int).collect();
        let ck: Vec<c_int> = (0..points.len()).map(|i| match cuts.get(i).copied().flatten() { Some((CutKind::Chamfer, ..)) => 2, Some((CutKind::Conic, ..)) => 3, Some((CutKind::Curve, ..)) => 4, Some((CutKind::Span, ..)) => 5, Some(_) => 1, None => 0 }).collect();
        let abm: Vec<f64> = (0..points.len()).flat_map(|i| match cuts.get(i).copied().flatten() { Some((_, a, m, b)) => [a[0], a[1], m[0], m[1], b[0], b[1]], None => [0.0; 6] }).collect();
        shape(unsafe { focc_outline_wire(xy.as_ptr(), sm.as_ptr(), ck.as_ptr(), abm.as_ptr(), points.len() as c_int, open as c_int) })
    }
    /// Turned by `angle` (radians) round the axis through `o` along `d`
    /// (Revolve): a face makes a solid, a wire a sheet.
    pub fn revolve(&self, o: [f64; 3], d: [f64; 3], angle: f64) -> Result<Shape, String> {
        shape(unsafe { focc_revolve(self.0, o.as_ptr(), d.as_ptr(), angle) })
    }
    /// A sculpted form: the bicubic BSpline through `poles` (world, nu × nv
    /// row major), periodic round, clamped bottom to top, its end rows
    /// single points: a solid.
    pub fn form(poles: &[[f64; 3]], nu: usize, nv: usize) -> Result<Shape, String> {
        if poles.len() != nu * nv {
            return Err("a form's poles do not match its grid".into());
        }
        let v: Vec<f64> = poles.iter().flatten().copied().collect();
        shape(unsafe { focc_form(v.as_ptr(), nu as c_int, nv as c_int) })
    }
    /// A loft through `secs` in order (wires, or faces: their outline): a
    /// solid when `solid`, else a sheet; `ruled`: straight between them.
    pub fn loft(secs: &[&Shape], solid: bool, ruled: bool) -> Result<Shape, String> {
        let ptrs: Vec<*const FShape> = secs.iter().map(|s| s.0 as *const FShape).collect();
        shape(unsafe { focc_loft(ptrs.as_ptr(), ptrs.len() as c_int, solid as c_int, ruled as c_int) })
    }

    /// A flat outline (Forme Rectify's Pen) through `pts` on the plane
    /// through `o` facing `n`: closed, a face facing `n`; open, a line (its
    /// edges free). Corner i cut by an arc through `cuts[i]` (its start,
    /// middle and end); an open line's ends are never cut.
    pub fn outline(o: [f64; 3], n: [f64; 3], pts: &[[f64; 3]], closed: bool, cuts: &[Option<[[f64; 3]; 3]>]) -> Result<Shape, String> {
        Shape::outline_bevel(o, n, pts, closed, cuts, &[])
    }
    /// The same, corner i cut straight from its start to its end where
    /// `bevel[i]` (a bevel) instead of by an arc.
    pub fn outline_bevel(o: [f64; 3], n: [f64; 3], pts: &[[f64; 3]], closed: bool, cuts: &[Option<[[f64; 3]; 3]>], bevel: &[bool]) -> Result<Shape, String> {
        let xyz: Vec<f64> = pts.iter().flatten().copied().collect();
        let ck: Vec<c_int> = (0..pts.len()).map(|i| if cuts.get(i).copied().flatten().is_none() { 0 } else if bevel.get(i).copied().unwrap_or(false) { 2 } else { 1 }).collect();
        let abm: Vec<f64> = (0..pts.len()).flat_map(|i| cuts.get(i).copied().flatten().map(|c| [c[0], c[1], c[2]]).unwrap_or([[0.0; 3]; 3]).into_iter().flatten()).collect();
        shape(unsafe { focc_outline(o.as_ptr(), n.as_ptr(), xyz.as_ptr(), pts.len() as c_int, closed as c_int, ck.as_ptr(), abm.as_ptr()) })
    }
    /// A disc of radius `r` round `o`, facing `n`.
    pub fn disc(o: [f64; 3], n: [f64; 3], r: f64) -> Result<Shape, String> {
        shape(unsafe { focc_disc(o.as_ptr(), n.as_ptr(), r) })
    }
    /// Edge `edge` (1-based) pulled along `v` into a new face (tag `tag`),
    /// sewn to the faces along it; a free edge (a line's) becomes its side.
    pub fn extrude_edge(&self, edge: i32, v: [f64; 3], tag: i64) -> Result<Shape, String> {
        shape(unsafe { focc_extrude_edge(self.0, edge, v.as_ptr(), tag) })
    }
    /// The faces made a solid `t` thick along their normals (< 0: the
    /// other way); the tops and sides get tags mixed from their face's and
    /// `salt`.
    pub fn thicken(&self, t: f64, salt: i64) -> Result<Shape, String> {
        shape(unsafe { focc_thicken(self.0, t, salt) })
    }
    /// Edges rounded with radius `r` on the shape's faces (a solid or a
    /// sheet), free edges kept; the new faces get `fresh`.
    pub fn round(&self, edges: &[i32], r: f64, fresh: i64) -> Result<Shape, String> {
        shape(unsafe { focc_round(self.0, edges.as_ptr(), edges.len() as c_int, r, fresh) })
    }
    /// Edges cut flat, `d` along each face (a bevel), on the shape's faces
    /// (a solid or a sheet), free edges kept; the new faces get `fresh`.
    pub fn bevel(&self, edges: &[i32], d: f64, fresh: i64) -> Result<Shape, String> {
        shape(unsafe { focc_bevel(self.0, edges.as_ptr(), edges.len() as c_int, d, fresh) })
    }

    /// Copy moved by an affine map: 3 rows of 4 (rotation × uniform scale,
    /// a mirror allowed, then translation).
    pub fn transformed(&self, rows: [[f64; 4]; 3]) -> Result<Shape, String> {
        let m: Vec<f64> = rows.iter().flatten().copied().collect();
        shape(unsafe { focc_transform(self.0, m.as_ptr()) })
    }

    /// A surface with no thickness through a closed outline of straight
    /// sides, smooth inside and passing near `inner` (a flat outline: a
    /// plane).
    pub fn fill(pts: &[[f64; 3]], inner: &[[f64; 3]]) -> Result<Shape, String> {
        let xyz: Vec<f64> = pts.iter().flatten().copied().collect();
        let iv: Vec<f64> = inner.iter().flatten().copied().collect();
        shape(unsafe { focc_fill(xyz.as_ptr(), pts.len() as c_int, iv.as_ptr(), inner.len() as c_int) })
    }
    /// A surface following heights `h` (nx × nz, x major) over the box
    /// (x0, z0)-(x1, z1) of the frame `fr` (origin, x, y up, z), trimmed by
    /// the outline `poly` (x, z in that frame).
    #[allow(clippy::too_many_arguments)]
    pub fn sheet_grid(fr: &[f64; 12], nx: usize, nz: usize, lo: [f64; 2], hi: [f64; 2], h: &[f64], poly: &[[f64; 2]]) -> Result<Shape, String> {
        if h.len() != nx * nz {
            return Err("the grid's heights do not match it".into());
        }
        let pv: Vec<f64> = poly.iter().flatten().copied().collect();
        shape(unsafe { focc_sheet_grid(fr.as_ptr(), nx as c_int, nz as c_int, lo[0], lo[1], hi[0], hi[1], h.as_ptr(), pv.as_ptr(), poly.len() as c_int) })
    }
    /// This shape and `other` side by side, no boolean (a sheet next to the
    /// part), the faces' tags kept.
    pub fn compound(&self, other: &Shape) -> Result<Shape, String> {
        shape(unsafe { focc_compound(self.0, other.0) })
    }
    /// Its solids alone, or (false) what is not a solid (sheets beside the
    /// part); Err when there is none.
    pub fn lumps_of(&self, solids: bool) -> Result<Shape, String> {
        shape(unsafe { focc_lumps_of(self.0, solids as c_int) })
    }
    /// A union or a cut made on the solids, the sheets beside them kept
    /// as they are (a boolean with a sheet in the part fails).
    fn solid_boolean(&self, tool: &Shape, f: unsafe extern "C" fn(*const FShape, *const FShape) -> *mut FShape) -> Result<Shape, String> {
        match self.lumps_of(false) {
            Ok(sheets) => match self.lumps_of(true) {
                Ok(solids) => shape(unsafe { f(solids.0, tool.0) })?.compound(&sheets),
                Err(_) => shape(unsafe { f(self.0, tool.0) }),
            },
            Err(_) => shape(unsafe { f(self.0, tool.0) }),
        }
    }
    pub fn fuse(&self, other: &Shape) -> Result<Shape, String> {
        self.solid_boolean(other, focc_fuse)
    }
    pub fn cut(&self, tool: &Shape) -> Result<Shape, String> {
        self.solid_boolean(tool, focc_cut)
    }
    /// What the two have in common (the Curves set's Intersect: two flat
    /// regions).
    pub fn common(&self, other: &Shape) -> Result<Shape, String> {
        shape(unsafe { focc_common(self.0, other.0) })
    }

    /// Junction edges made by the boolean that produced this shape.
    pub fn section_edges(&self) -> Vec<i32> {
        let n = unsafe { focc_section_count(self.0) };
        let mut v: Vec<i32> = (0..n).map(|k| unsafe { focc_section_edge(self.0, k) }).filter(|i| *i > 0).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Every edge once (degenerated ones, like a sphere's poles, left out),
    /// sampled at `n` points.
    pub fn edges(&self, n: usize) -> Vec<Edge> {
        let count = unsafe { focc_edge_count(self.0) };
        (1..=count).filter_map(|i| self.edge(i, n)).collect()
    }

    pub fn edge(&self, index: i32, n: usize) -> Option<Edge> {
        let n = n.max(2);
        let mut buf = vec![0.0f64; 3 * n];
        let k = unsafe { focc_edge_sample(self.0, index, n as c_int, buf.as_mut_ptr()) };
        let kind = match k {
            0 => EdgeKind::Line,
            1 => EdgeKind::Circle,
            2 => EdgeKind::Curve,
            _ => return None,
        };
        Some(Edge { index, kind, pts: buf.chunks(3).map(|c| [c[0], c[1], c[2]]).collect() })
    }

    /// For every edge (index i at position i - 1): the indices of the two
    /// faces it joins (-1 = none; a seam joins its face to itself, so its
    /// second face is -1).
    pub fn edge_faces(&self) -> Vec<[i32; 2]> {
        let count = unsafe { focc_edge_count(self.0) };
        if count <= 0 {
            return vec![];
        }
        let mut buf = vec![-1 as c_int; 2 * count as usize];
        let n = unsafe { focc_edge_faces(self.0, buf.as_mut_ptr(), buf.len() as c_int) };
        if n < 0 {
            return vec![];
        }
        buf.chunks(2).take(n as usize).map(|c| [c[0], c[1]]).collect()
    }

    /// Fillets' straight and circular faces made conic, each fillet (its
    /// faces' tag) with its own tension (a conic's rho): the solid, and for
    /// each fillet how many of its faces took it out of how many. Two
    /// fillets meeting in a mitre go together when their tensions agree.
    pub fn fillet_tension(&self, fillets: &[(i64, f64)]) -> Result<(Shape, Vec<Tension>), String> {
        self.fillet_tension_since(None, fillets)
    }
    /// The same, `before` the shape the fillets were made on: a face of
    /// theirs that was there already, as it is now, is left alone and not
    /// counted (another copy's of an Array: it has its tension).
    pub fn fillet_tension_since(&self, before: Option<&Shape>, fillets: &[(i64, f64)]) -> Result<(Shape, Vec<Tension>), String> {
        let tags: Vec<i64> = fillets.iter().map(|f| f.0).collect();
        let rhos: Vec<f64> = fillets.iter().map(|f| f.1).collect();
        let mut done = vec![0 as c_int; fillets.len()];
        let mut total = vec![0 as c_int; fillets.len()];
        let mut tight = vec![0.0 as c_double; 4 * fillets.len()];
        let s = shape(unsafe { focc_fillet_tension(self.0, before.map_or(std::ptr::null(), |b| b.0 as *const FShape), tags.as_ptr(), rhos.as_ptr(), fillets.len() as c_int, done.as_mut_ptr(), total.as_mut_ptr(), tight.as_mut_ptr()) })?;
        Ok((
            s,
            (0..fillets.len())
                .map(|k| Tension {
                    done: done[k] as usize,
                    total: total[k] as usize,
                    tight: (tight[4 * k + 1] > 0.0).then(|| Tight { radius: tight[4 * k], need: tight[4 * k + 1], works: (tight[4 * k + 2] > 0.0).then(|| (tight[4 * k + 2], tight[4 * k + 3])) }),
                })
                .collect(),
        ))
    }
    /// Rounds the edges (indices in this shape) with radius r, in one
    /// operation. Err when OpenCascade cannot make it or the result is not
    /// a valid solid.
    /// The new (fillet) faces get the tag `fresh`; the others keep theirs.
    pub fn fillet(&self, edges: &[i32], r: f64, fresh: i64) -> Result<Shape, String> {
        shape(unsafe { focc_fillet(self.0, edges.as_ptr(), edges.len() as c_int, r, fresh) })
    }
    /// Several fillets in one operation (corners blended): each edge
    /// (1-based) with its radius and the tag of the faces made along it;
    /// corner faces get `fresh`.
    pub fn fillet_multi(&self, edges: &[(i32, f64, i64)], fresh: i64) -> Result<Shape, String> {
        let idx: Vec<c_int> = edges.iter().map(|e| e.0).collect();
        let r: Vec<c_double> = edges.iter().map(|e| e.1).collect();
        let t: Vec<i64> = edges.iter().map(|e| e.2).collect();
        shape(unsafe { focc_fillet_multi(self.0, idx.as_ptr(), r.as_ptr(), t.as_ptr(), idx.len() as c_int, fresh) })
    }
    pub fn chamfer(&self, edges: &[i32], d: f64, fresh: i64) -> Result<Shape, String> {
        shape(unsafe { focc_chamfer(self.0, edges.as_ptr(), edges.len() as c_int, d, fresh) })
    }

    /// The inside of a flat face (index in the face map), its outline
    /// moved in by `d` mm (0: the whole face): a new face on the same plane,
    /// facing the same way, with its own edges.
    pub fn inset_face(&self, face: usize, d: f64) -> Result<Shape, String> {
        shape(unsafe { focc_inset_face(self.0, face as c_int, d) })
    }
    /// A face swept along `v` into a solid.
    pub fn extrude(&self, v: [f64; 3]) -> Result<Shape, String> {
        shape(unsafe { focc_extrude(self.0, v[0], v[1], v[2]) })
    }
    /// A face (index in the face map) copied and made a solid `t` thick
    /// along its outward normal (< 0: inwards), for any surface: tags
    /// `[base, top, side]` (the sides `side`, `side` + 1…).
    pub fn thicken_face(&self, face: usize, t: f64, tags: [i64; 3]) -> Result<Shape, String> {
        shape(unsafe { focc_thicken_face(self.0, face as c_int, t, tags[0], tags[1], tags[2]) })
    }
    /// Flat face `face` moved `d` along its outward normal, the faces round
    /// it (flat or curved, running along the move) made longer or
    /// shorter; tags kept.
    pub fn move_face(&self, face: usize, d: f64) -> Result<Shape, String> {
        shape(unsafe { focc_move_face(self.0, face as c_int, d) })
    }
    /// The solid with `tool` (a face lying on it) drawn into its faces; the
    /// part under the tool takes the tool's tag.
    pub fn imprint(&self, tool: &Shape) -> Result<Shape, String> {
        shape(unsafe { focc_imprint(self.0, tool.0) })
    }
    /// Edge `edge` (1-based) at fraction `f` of its parameter range.
    pub fn edge_point(&self, edge: i32, f: f64) -> Option<[f64; 3]> {
        let mut p = [0.0; 3];
        (unsafe { focc_edge_point(self.0, edge, f, p.as_mut_ptr()) } == 0).then_some(p)
    }
    /// The Knife: points cut into the edges they lie on, and straight
    /// lines between pairs of them (indices) cut into the faces they lie on.
    pub fn split(&self, pts: &[[f64; 3]], segs: &[[i32; 2]]) -> Result<Shape, String> {
        let flat: Vec<f64> = pts.iter().flatten().copied().collect();
        let s: Vec<c_int> = segs.iter().flatten().map(|&i| i as c_int).collect();
        shape(unsafe { focc_split(self.0, flat.as_ptr(), pts.len() as c_int, s.as_ptr(), segs.len() as c_int) })
    }
    /// Faces (0-based) taken away, the faces round them grown to close the
    /// part again (a fillet's faces: its edges sharp again).
    pub fn defeature(&self, faces: &[usize]) -> Result<Shape, String> {
        let idx: Vec<c_int> = faces.iter().map(|&i| i as c_int).collect();
        shape(unsafe { focc_defeature(self.0, idx.as_ptr(), idx.len() as c_int) })
    }
    /// The part hollowed (Shell): faces (0-based) taken away as openings,
    /// the walls `t` thick inwards (or `outward`); only the solids holding
    /// an opening (every one when there is none: a cavity inside). `arc`:
    /// the gaps between two faces' offsets filled round, not grown sharp.
    /// Tags: a face kept its own, its offset `base` + 1 + a hash of its
    /// tag, the rims `base`.
    pub fn shell(&self, faces: &[usize], t: f64, outward: bool, base: i64, arc: bool) -> Result<Shape, String> {
        let idx: Vec<c_int> = faces.iter().map(|&i| i as c_int).collect();
        shape(unsafe { focc_shell(self.0, idx.as_ptr(), idx.len() as c_int, t, outward as c_int, base, arc as c_int) })
    }
    /// Faces (0-based) copied as a piece (closed by flat faces where they
    /// are open, else a sheet: volume 0), moved by the affine map `rows`;
    /// the copies tagged `tag | 1`, the closing faces `tag`.
    pub fn piece(&self, faces: &[usize], rows: [[f64; 4]; 3], tag: i64) -> Result<Shape, String> {
        let idx: Vec<c_int> = faces.iter().map(|&i| i as c_int).collect();
        let m: Vec<f64> = rows.iter().flatten().copied().collect();
        shape(unsafe { focc_piece(self.0, idx.as_ptr(), idx.len() as c_int, m.as_ptr(), tag) })
    }
    /// Faces (0-based) taken out: the part left open there.
    pub fn remove_faces(&self, faces: &[usize]) -> Result<Shape, String> {
        let idx: Vec<c_int> = faces.iter().map(|&i| i as c_int).collect();
        shape(unsafe { focc_remove_faces(self.0, idx.as_ptr(), idx.len() as c_int) })
    }
    /// Edges (1-based) taken out: the two faces each parts made one (they
    /// must lie on one surface).
    pub fn remove_edges(&self, edges: &[i32]) -> Result<Shape, String> {
        let idx: Vec<c_int> = edges.iter().map(|&i| i as c_int).collect();
        shape(unsafe { focc_remove_edges(self.0, idx.as_ptr(), idx.len() as c_int) })
    }
    /// Distance from a point to a face (None when it cannot be measured).
    pub fn face_distance(&self, face: usize, p: [f64; 3]) -> Option<f64> {
        let d = unsafe { focc_face_distance(self.0, face as c_int, p.as_ptr()) };
        (d >= 0.0).then_some(d)
    }
    /// The vertices' points, vertex `i` (1-based in OpenCascade's vertex
    /// map) at `[i - 1]`.
    pub fn vertex_points(&self) -> Vec<[f64; 3]> {
        let n = unsafe { focc_vertex_points(self.0, std::ptr::null_mut(), 0) }.max(0) as usize;
        let mut v = vec![0.0f64; 3 * n];
        unsafe { focc_vertex_points(self.0, v.as_mut_ptr(), n as c_int) };
        v.chunks(3).map(|c| [c[0], c[1], c[2]]).collect()
    }
    /// The vertices (1-based) of an edge (1-based index in the edge map).
    pub fn edge_vertices(&self, edge: i32) -> Vec<i32> {
        self.sub_vertices(1, edge)
    }
    /// The vertices (1-based) of a face (0-based).
    pub fn face_vertices(&self, face: usize) -> Vec<i32> {
        self.sub_vertices(2, face as i32)
    }
    fn sub_vertices(&self, kind: i32, index: i32) -> Vec<i32> {
        let mut v = vec![0 as c_int; 256];
        let n = unsafe { focc_sub_vertices(self.0, kind, index, v.as_mut_ptr(), v.len() as c_int) };
        v.truncate(n.max(0) as usize);
        v
    }
    /// The solid with vertices moved (1-based index, new point), the faces
    /// round them following (flat faces with straight edges for now; a bent
    /// four-sided face becomes a bilinear patch). Tags stay.
    /// The pieces (lumps) whose vertices (1-based) are all in `vs` moved
    /// whole by the matrix (3 rows of 4); Err when a piece has only some of
    /// them (a deformation) or none moves.
    pub fn move_lumps(&self, vs: &[i32], rows: [[f64; 4]; 3]) -> Result<Shape, String> {
        let idx: Vec<c_int> = vs.to_vec();
        let m: Vec<f64> = rows.iter().flatten().copied().collect();
        shape(unsafe { focc_move_lumps(self.0, idx.as_ptr(), idx.len() as c_int, m.as_ptr()) })
    }
    pub fn move_vertices(&self, moves: &[(i32, [f64; 3])]) -> Result<Shape, String> {
        let idx: Vec<c_int> = moves.iter().map(|m| m.0).collect();
        let xyz: Vec<f64> = moves.iter().flat_map(|m| m.1).collect();
        shape(unsafe { focc_move_vertices(self.0, idx.as_ptr(), xyz.as_ptr(), idx.len() as c_int) })
    }
    pub fn face_count(&self) -> usize {
        unsafe { focc_face_count(self.0) }.max(0) as usize
    }

    /// Face `index` (0-based, the shape's face order).
    pub fn face_info(&self, index: usize) -> Option<FaceInfo> {
        let mut d = [0.0f64; 13];
        let k = unsafe { focc_face_info(self.0, index as c_int, d.as_mut_ptr()) };
        (k >= 0).then(|| FaceInfo::from_raw(k, &d))
    }

    /// Face `index`'s bounding box (min, max).
    pub fn face_box(&self, index: usize) -> Option<([f64; 3], [f64; 3])> {
        let mut d = [0.0f64; 6];
        let k = unsafe { focc_face_box(self.0, index as c_int, d.as_mut_ptr()) };
        (k >= 0).then(|| ([d[0], d[1], d[2]], [d[3], d[4], d[5]]))
    }

    /// Its faces of one surface made one, as a union or a cut leaves the
    /// whole part (the edges between two faces of one tag kept).
    pub fn simplified(&self) -> Result<Shape, String> {
        shape(unsafe { focc_simplified(self.0) })
    }

    /// The boxes (min, max) of its lumps: each solid, and each shell or
    /// face beside them (from the geometry, a little wide on splines).
    pub fn lump_boxes(&self) -> Vec<([f64; 3], [f64; 3])> {
        let mut d = vec![0.0f64; 6 * 16];
        let mut n = unsafe { focc_lump_boxes(self.0, d.as_mut_ptr(), 16) };
        if n > 16 {
            d = vec![0.0f64; 6 * n as usize];
            n = unsafe { focc_lump_boxes(self.0, d.as_mut_ptr(), n) };
        }
        (0..n.max(0) as usize).map(|i| ([d[6 * i], d[6 * i + 1], d[6 * i + 2]], [d[6 * i + 3], d[6 * i + 4], d[6 * i + 5]])).collect()
    }

    /// Names the faces (one tag per face, in face order). Transforms,
    /// booleans and fillets pass each face's tag on to what it becomes.
    pub fn set_tags(&mut self, tags: &[i64]) {
        unsafe { focc_set_tags(self.0, tags.as_ptr(), tags.len() as c_int) }
    }

    pub fn face_tag(&self, index: usize) -> i64 {
        unsafe { focc_face_tag(self.0, index as c_int) }
    }

    /// Meshes the shape (chord `defl` mm, angle `angle` rad) and returns its
    /// faces, triangles and edges.
    /// The faces' wires and the edges, for the quad mesher.
    pub fn quad_topo(&self) -> Result<QuadTopo, String> {
        let b = unsafe { focc_quad_topo(self.0) };
        if b.is_null() {
            return Err(last_error());
        }
        let out = unsafe {
            let bytes = std::slice::from_raw_parts(focc_buf_data(b), focc_buf_len(b).max(0) as usize);
            let t = parse_quad_topo(bytes);
            focc_buf_free(b);
            t
        };
        out.ok_or_else(|| "quad topology buffer cut short".to_string())
    }

    /// Points of an edge (1-based) at parameters `t`: 3D, and the surface
    /// coordinates on face `face` (0-based) where it is used `reversed` or not.
    pub fn edge_on_face(&self, face: usize, edge: i32, reversed: bool, t: &[f64]) -> Result<(Vec<[f64; 3]>, Vec<[f64; 2]>), String> {
        let mut xyz = vec![0.0; t.len() * 3];
        let mut uv = vec![0.0; t.len() * 2];
        let n = unsafe { focc_edge_on_face(self.0, face as c_int, edge, reversed as c_int, t.len() as c_int, t.as_ptr(), xyz.as_mut_ptr(), uv.as_mut_ptr()) };
        if n < 0 {
            return Err(last_error());
        }
        Ok((xyz.chunks(3).map(|c| [c[0], c[1], c[2]]).collect(), uv.chunks(2).map(|c| [c[0], c[1]]).collect()))
    }

    /// Points of face `face` (0-based) at surface coordinates: the point
    /// and the outward normal.
    /// Points put on a face's surface (its nearest point): (u, v), the
    /// point, the outward normal.
    pub fn face_project(&self, face: usize, pts: &[[f64; 3]]) -> Result<Vec<([f64; 2], [f64; 3], [f64; 3])>, String> {
        let flat: Vec<f64> = pts.iter().flatten().copied().collect();
        let mut out = vec![0.0; pts.len() * 8];
        let n = unsafe { focc_face_project(self.0, face as c_int, pts.len() as c_int, flat.as_ptr(), out.as_mut_ptr()) };
        if n < 0 {
            return Err(last_error());
        }
        Ok(out.chunks(8).map(|c| ([c[0], c[1]], [c[2], c[3], c[4]], [c[5], c[6], c[7]])).collect())
    }

    pub fn face_eval(&self, face: usize, uv: &[[f64; 2]]) -> Result<Vec<([f64; 3], [f64; 3])>, String> {
        let flat: Vec<f64> = uv.iter().flatten().copied().collect();
        let mut out = vec![0.0; uv.len() * 6];
        let n = unsafe { focc_face_eval(self.0, face as c_int, uv.len() as c_int, flat.as_ptr(), out.as_mut_ptr()) };
        if n < 0 {
            return Err(last_error());
        }
        Ok(out.chunks(6).map(|c| ([c[0], c[1], c[2]], [c[3], c[4], c[5]])).collect())
    }

    pub fn topo(&self, defl: f64, angle: f64) -> Result<Topo, String> {
        let b = unsafe { focc_topo(self.0, defl, angle) };
        if b.is_null() {
            return Err(last_error());
        }
        let out = unsafe {
            let bytes = std::slice::from_raw_parts(focc_buf_data(b), focc_buf_len(b).max(0) as usize);
            let t = parse_topo(bytes);
            focc_buf_free(b);
            t
        };
        out.ok_or_else(|| "topology buffer cut short".to_string())
    }

    pub fn volume(&self) -> f64 {
        unsafe { focc_volume(self.0) }
    }
    pub fn is_valid(&self) -> bool {
        unsafe { focc_is_valid(self.0) == 1 }
    }

    /// The edges a person would draw (face borders, fillet boundaries
    /// included; no seams, no poles) as polylines within `defl` mm and
    /// `angle` rad of the true curves.
    pub fn edge_lines(&self, defl: f64, angle: f64) -> Result<Vec<Vec<[f64; 3]>>, String> {
        let l = unsafe { focc_lines(self.0, defl, angle) };
        if l.is_null() {
            return Err(last_error());
        }
        unsafe {
            let n = focc_lines_count(l).max(0) as usize;
            let counts = std::slice::from_raw_parts(focc_lines_counts(l), n);
            let xyz = std::slice::from_raw_parts(focc_lines_xyz(l), focc_lines_points(l).max(0) as usize * 3);
            let mut out = Vec::with_capacity(n);
            let mut at = 0;
            for &c in counts {
                let c = c.max(0) as usize;
                out.push(xyz[at * 3..(at + c) * 3].chunks(3).map(|q| [q[0], q[1], q[2]]).collect());
                at += c;
            }
            focc_lines_free(l);
            Ok(out)
        }
    }

    /// Triangles: chord deflection `defl` (mm), angular deflection (rad).
    pub fn mesh(&self, defl: f64, angle: f64) -> Result<Mesh, String> {
        let m = unsafe { focc_mesh(self.0, defl, angle) };
        if m.is_null() {
            return Err(last_error());
        }
        unsafe {
            let nv = focc_mesh_vertices(m).max(0) as usize;
            let nt = focc_mesh_triangles(m).max(0) as usize;
            let p = std::slice::from_raw_parts(focc_mesh_positions(m), nv * 3);
            let n = std::slice::from_raw_parts(focc_mesh_normals(m), nv * 3);
            let t = std::slice::from_raw_parts(focc_mesh_indices(m), nt * 3);
            let out = Mesh {
                positions: p.chunks(3).map(|c| [c[0], c[1], c[2]]).collect(),
                normals: n.chunks(3).map(|c| [c[0], c[1], c[2]]).collect(),
                triangles: t.chunks(3).map(|c| [c[0], c[1], c[2]]).collect(),
            };
            focc_mesh_free(m);
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_hole_fillet() {
        let cube = Shape::cuboid(20.0, 20.0, 20.0).unwrap();
        let hole = Shape::cylinder(4.0, 28.0).unwrap();
        let cut = cube.cut(&hole).unwrap();
        let sec = cut.section_edges();
        assert_eq!(sec.len(), 2, "two rims");
        let f = cut.fillet(&sec, 1.5, 99).unwrap();
        assert!(f.is_valid());
        let v = f.volume();
        let raw = 8000.0 - std::f64::consts::PI * 16.0 * 20.0;
        assert!(v < raw && v > raw - 60.0, "volume {v}");
        // a fillet too big fails cleanly
        assert!(cut.fillet(&sec, 30.0, 99).is_err());
        let m = f.mesh(0.02, 0.25).unwrap();
        assert!(m.triangles.len() > 100);
        // lines: the cube's 12 edges, each rim's two fillet boundaries; the
        // fillets' and the hole's seams left out
        let raw = cut.edge_lines(0.02, 0.2).unwrap();
        assert_eq!(raw.len(), 12 + 2, "cube edges + 2 rims");
        let lines = f.edge_lines(0.02, 0.2).unwrap();
        assert_eq!(lines.len(), 12 + 4, "cube edges + 2 boundaries per rim");
        assert!(lines.iter().all(|l| l.len() >= 2));
    }

    #[test]
    fn inset() {
        // a 20 mm cube, tagged face by face; its top (normal +Y) inset by 3 mm
        let mut cube = Shape::cuboid(20.0, 20.0, 20.0).unwrap();
        let tags: Vec<i64> = (0..cube.face_count() as i64).collect();
        cube.set_tags(&tags);
        let top = (0..cube.face_count()).find(|f| cube.face_info(*f).unwrap().normal[1] > 0.9).unwrap();
        assert!(cube.face_distance(top, [0.0, 15.0, 0.0]).map(|d| (d - 5.0).abs() < 1e-6).unwrap_or(false));
        let mut inner = cube.inset_face(top, 3.0).unwrap();
        let info = inner.face_info(0).unwrap();
        assert!(info.normal[1] > 0.99, "facing up: {:?}", info.normal);
        inner.set_tags(&[100]);
        // a line only: the top cut in two, its inside tagged 100
        let lined = cube.imprint(&inner).unwrap();
        assert!(lined.is_valid());
        assert_eq!(lined.face_count(), 7);
        assert!((lined.volume() - 8000.0).abs() < 1e-6);
        let tags: Vec<i64> = (0..lined.face_count()).map(|f| lined.face_tag(f)).collect();
        assert!(tags.contains(&100) && tags.contains(&(top as i64)), "{tags:?}");
        // a pocket 4 mm deep, a raised panel 2 mm high
        let pocket = cube.cut(&inner.transformed([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.5], [0.0, 0.0, 1.0, 0.0]]).unwrap().extrude([0.0, -4.5, 0.0]).unwrap()).unwrap();
        assert!((pocket.volume() - (8000.0 - 14.0 * 14.0 * 4.0)).abs() < 1e-3, "{}", pocket.volume());
        let raised = cube.fuse(&inner.extrude([0.0, 2.0, 0.0]).unwrap()).unwrap();
        assert!((raised.volume() - (8000.0 + 14.0 * 14.0 * 2.0)).abs() < 1e-3, "{}", raised.volume());
        assert!(raised.is_valid() && pocket.is_valid());
        // too wide
        assert!(cube.inset_face(top, 11.0).is_err());
        // the whole face (inset 0) pushed 5 mm in
        let whole = cube.inset_face(top, 0.0).unwrap();
        let pushed = cube.cut(&whole.transformed([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.1], [0.0, 0.0, 1.0, 0.0]]).unwrap().extrude([0.0, -5.1, 0.0]).unwrap()).unwrap();
        assert!((pushed.volume() - 20.0 * 20.0 * 15.0).abs() < 1e-3 && pushed.is_valid() && pushed.face_count() == 6, "{} {}", pushed.volume(), pushed.face_count());
        // a face with a hole: the hole grows
        let plate = Shape::cuboid(40.0, 10.0, 30.0).unwrap().cut(&Shape::cylinder(5.0, 30.0).unwrap()).unwrap();
        let top = (0..plate.face_count()).find(|f| plate.face_info(*f).map(|i| i.kind == SurfKind::Plane && i.normal[1] > 0.9).unwrap_or(false)).unwrap();
        let inner = plate.inset_face(top, 2.0).unwrap();
        let panel = plate.fuse(&inner.extrude([0.0, 1.0, 0.0]).unwrap()).unwrap();
        let area = 36.0 * 26.0 - std::f64::consts::PI * 49.0;
        assert!((panel.volume() - plate.volume() - area).abs() < 0.05, "{}", panel.volume() - plate.volume());
        let sunk = plate.cut(&inner.transformed([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.5], [0.0, 0.0, 1.0, 0.0]]).unwrap().extrude([0.0, -2.5, 0.0]).unwrap()).unwrap();
        assert!((plate.volume() - sunk.volume() - 2.0 * area).abs() < 0.1, "{}", plate.volume() - sunk.volume());
        assert!(panel.is_valid() && sunk.is_valid());
        // the line only: the top in three (the frame, the ring round the
        // hole, the inside), only the inside tagged like the inset
        let mut plate = plate;
        let tags: Vec<i64> = (0..plate.face_count() as i64).collect();
        plate.set_tags(&tags);
        let mut inner = inner;
        inner.set_tags(&[100]);
        let lined = plate.imprint(&inner).unwrap();
        assert!(lined.is_valid());
        assert_eq!(lined.face_count(), plate.face_count() + 2);
        let tags: Vec<i64> = (0..lined.face_count()).map(|f| lined.face_tag(f)).collect();
        assert_eq!(tags.iter().filter(|t| **t == 100).count(), 1, "{tags:?}");
        assert_eq!(tags.iter().filter(|t| **t == top as i64).count(), 2, "{tags:?}");
        // the whole holed top pulled up 3 mm
        let whole = plate.inset_face(top, 0.0).unwrap();
        let pulled = plate.fuse(&whole.extrude([0.0, 3.0, 0.0]).unwrap()).unwrap();
        let area = 40.0 * 30.0 - std::f64::consts::PI * 25.0;
        assert!((pulled.volume() - plate.volume() - 3.0 * area).abs() < 0.05 && pulled.is_valid(), "{}", pulled.volume() - plate.volume());
    }

    #[test]
    fn profile_prism() {
        // an L, both windings: same positive volume, valid
        let l = [[0.0, 0.0], [20.0, 0.0], [20.0, 6.0], [6.0, 6.0], [6.0, 20.0], [0.0, 20.0]];
        let area = 20.0 * 6.0 + 6.0 * 14.0;
        for pts in [l.to_vec(), l.iter().rev().copied().collect()] {
            let p = Shape::prism(&pts, &[false; 6], 2.0).unwrap();
            assert!(p.is_valid());
            assert!((p.volume() - area * 4.0).abs() < 1e-6, "volume {}", p.volume());
            assert_eq!(p.edges(2).len(), 18);
        }
        // a square with one round corner: a little less matter
        let sq = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let r = Shape::prism(&sq, &[false, false, true, false], 1.0).unwrap();
        let cut = 25.0 / 6.0; // area between the corner and the curve: (5 x 5) / 6
        assert!((r.volume() - (100.0 - cut) * 2.0).abs() < 1e-6, "volume {}", r.volume());
        // all smooth: a rounded square, one face around
        let o = Shape::prism(&sq, &[true; 4], 1.0).unwrap();
        assert!(o.is_valid() && o.volume() > 0.0);
        // a bow tie crosses itself
        assert!(Shape::prism(&[[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]], &[false; 4], 1.0).is_err());
        // it fillets like the rest
        let p = Shape::prism(&l, &[false; 6], 2.0).unwrap();
        let all: Vec<i32> = p.edges(2).iter().map(|e| e.index).collect();
        assert!(p.fillet(&all, 0.5, 99).unwrap().is_valid());
    }

    #[test]
    fn vertices_moved() {
        // a 20 mm cube centred: its top face (y = 10) moved up, then aside
        let mut cube = Shape::cuboid(20.0, 20.0, 20.0).unwrap();
        let n = cube.face_count();
        cube.set_tags(&(0..n as i64).collect::<Vec<_>>());
        let pts = cube.vertex_points();
        let top: Vec<i32> = (1..=pts.len() as i32).filter(|i| (pts[(*i - 1) as usize][1] - 10.0).abs() < 1e-6).collect();
        assert_eq!(top.len(), 4);
        let up = cube.move_vertices(&top.iter().map(|i| (*i, { let p = pts[(*i - 1) as usize]; [p[0], p[1] + 5.0, p[2]] })).collect::<Vec<_>>()).unwrap();
        assert!((up.volume() - 20.0 * 20.0 * 25.0).abs() < 1e-6, "{}", up.volume());
        let mut tags: Vec<i64> = (0..up.face_count()).map(|f| up.face_tag(f)).collect();
        tags.sort();
        assert_eq!(tags, (0..n as i64).collect::<Vec<_>>(), "each face keeps its tag");
        let aside = cube.move_vertices(&top.iter().map(|i| (*i, { let p = pts[(*i - 1) as usize]; [p[0] + 6.0, p[1], p[2]] })).collect::<Vec<_>>()).unwrap();
        assert!((aside.volume() - 8000.0).abs() < 1e-6, "a shear keeps the volume: {}", aside.volume());
        // one edge of the top raised: two faces tilt, two stay flat and grow
        let edge: Vec<i32> = top.iter().copied().filter(|i| pts[(*i - 1) as usize][2] > 0.0).collect();
        let e = cube.move_vertices(&edge.iter().map(|i| (*i, { let p = pts[(*i - 1) as usize]; [p[0], p[1] + 4.0, p[2]] })).collect::<Vec<_>>()).unwrap();
        assert!((e.volume() - (8000.0 + 0.5 * 4.0 * 20.0 * 20.0)).abs() < 1e-6, "{}", e.volume());
        // one corner up: three faces bend into bilinear patches
        let c = cube.move_vertices(&[(top[0], { let p = pts[(top[0] - 1) as usize]; [p[0], p[1] + 4.0, p[2]] })]).unwrap();
        assert!(c.volume() > 8000.0 && c.volume() < 8000.0 + 4.0 * 400.0, "{}", c.volume());
        // a block with a hole: its side face moved out, the top grows round the hole
        let hole = Shape::cylinder(4.0, 40.0).unwrap();
        let block = Shape::cuboid(40.0, 20.0, 20.0).unwrap().cut(&hole).unwrap();
        let pts = block.vertex_points();
        let side: Vec<(i32, [f64; 3])> = (1..=pts.len() as i32).filter(|i| (pts[(*i - 1) as usize][0] - 20.0).abs() < 1e-6).map(|i| { let p = pts[(i - 1) as usize]; (i, [p[0] + 5.0, p[1], p[2]]) }).collect();
        assert_eq!(side.len(), 4);
        let v0 = block.volume();
        let b = block.move_vertices(&side).unwrap();
        assert!((b.volume() - (v0 + 5.0 * 20.0 * 20.0)).abs() < 1e-6, "{} vs {}", b.volume(), v0);
        // the top face (with the hole) tilts: its +X edge up 3, the hole's
        // wall runs up to it (the wedge less the hole's column, 1.5 high
        // on average)
        let top: Vec<(i32, [f64; 3])> = (1..=pts.len() as i32).filter(|i| { let p = pts[(*i - 1) as usize]; (p[1] - 10.0).abs() < 1e-6 && (p[0] - 20.0).abs() < 1e-6 }).map(|i| { let p = pts[(i - 1) as usize]; (i, [p[0], p[1] + 3.0, p[2]]) }).collect();
        let t = block.move_vertices(&top).unwrap();
        assert!(t.is_valid());
        let want = v0 + 0.5 * 3.0 * 40.0 * 20.0 - std::f64::consts::PI * 16.0 * 1.5;
        assert!((t.volume() - want).abs() < 1e-3, "{} vs {}", t.volume(), want);
    }

    #[test]
    fn tags_and_topo() {
        // a cube tagged 10..15, a hole tagged 20..22: the holed cube keeps
        // the cube's six tags and gets the hole's side; the fillet faces
        // get their own tag
        let mut cube = Shape::cuboid(20.0, 20.0, 20.0).unwrap();
        assert_eq!(cube.face_count(), 6);
        cube.set_tags(&(10..16).collect::<Vec<i64>>());
        let mut hole = Shape::cylinder(4.0, 28.0).unwrap();
        let n = hole.face_count();
        assert_eq!(n, 3);
        let kinds: Vec<SurfKind> = (0..n).map(|i| hole.face_info(i).unwrap().kind).collect();
        assert_eq!(kinds.iter().filter(|k| **k == SurfKind::Cylinder).count(), 1);
        hole.set_tags(&[20, 21, 22]);
        let moved = hole.transformed([[1.0, 0.0, 0.0, 2.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]).unwrap();
        assert_eq!((0..3).map(|i| moved.face_tag(i)).collect::<Vec<_>>(), vec![20, 21, 22]);
        let cut = cube.cut(&moved).unwrap();
        let t = cut.topo(0.05, 0.3).unwrap();
        assert_eq!(t.faces.len(), 7);
        let mut tags: Vec<i64> = t.faces.iter().map(|f| f.tag).collect();
        tags.sort();
        let side = (0..3).map(|i| moved.face_tag(i)).find(|&tg| {
            let k = (0..3).find(|&j| moved.face_tag(j) == tg).unwrap();
            moved.face_info(k).unwrap().kind == SurfKind::Cylinder
        });
        assert_eq!(tags, {
            let mut v: Vec<i64> = (10..16).collect();
            v.push(side.unwrap());
            v.sort();
            v
        });
        // every triangle has a face, every edge two faces (a seam has its
        // one face on both sides)
        assert_eq!(t.tri_face.len(), t.triangles.len());
        assert!(t.tri_face.iter().all(|f| (*f as usize) < t.faces.len()));
        assert!(!t.edges.is_empty());
        assert!(t.edges.iter().all(|e| e.faces[0] >= 0 && (e.seam || e.faces[1] >= 0) && e.pts.len() >= 2));
        // the hole's seam is flagged
        assert_eq!(t.edges.iter().filter(|e| e.seam).count(), 1);
        let f = cut.fillet(&cut.section_edges(), 1.0, 99).unwrap();
        let tf = f.topo(0.05, 0.3).unwrap();
        assert_eq!(tf.faces.iter().filter(|f| f.tag == 99).count(), 2, "one fillet face per rim");
        assert_eq!(tf.faces.iter().filter(|f| f.tag < 0).count(), 0);
        // edge adjacency without meshing agrees with the topology's
        let adj = cut.edge_faces();
        for e in &t.edges {
            assert_eq!(adj[e.index as usize - 1], e.faces, "edge {}", e.index);
        }
    }

    #[test]
    fn shell_box() {
        // a 40 × 20 × 30 box (Y up), tagged face by face, hollowed 2 mm
        let mut b = Shape::cuboid(40.0, 20.0, 30.0).unwrap();
        let tags: Vec<i64> = (0..b.face_count() as i64).map(|i| 10 + i).collect();
        b.set_tags(&tags);
        let top = (0..b.face_count()).find(|i| b.face_info(*i).unwrap().normal[1] > 0.9).unwrap();
        let base = 7i64 << 16;
        // the top open, inwards: 24000 − 36 × 18 × 26
        let s = b.shell(&[top], 2.0, false, base, false).unwrap();
        assert!(s.is_valid());
        assert!((s.volume() - 7152.0).abs() < 1.0, "open: {}", s.volume());
        let t: Vec<i64> = (0..s.face_count()).map(|i| s.face_tag(i)).collect();
        assert_eq!(t.iter().filter(|x| (10..16).contains(*x)).count(), 5, "the outer faces keep their tags: {t:?}");
        assert_eq!(t.iter().filter(|x| **x > base && **x < base + 65536).count(), 5, "the inner faces: {t:?}");
        assert!(t.iter().any(|x| *x == base), "the rim: {t:?}");
        // closed: a cavity inside, 24000 − 36 × 16 × 26
        let c = b.shell(&[], 2.0, false, base, false).unwrap();
        assert!((c.volume() - 9024.0).abs() < 1.0, "closed: {}", c.volume());
        // outwards: 44 × 22 × 34 − 24000
        let o = b.shell(&[top], 2.0, true, base, false).unwrap();
        assert!((o.volume() - 8912.0).abs() < 1.0, "outwards: {}", o.volume());
        // too thick: the box is 30 deep
        assert!(b.shell(&[top], 16.0, false, base, false).is_err());
        // with a hole through it and its rims rounded
        let hole = Shape::cylinder(4.0, 40.0).unwrap();
        let cut = b.cut(&hole).unwrap();
        let sec = cut.section_edges();
        let f = cut.fillet(&sec, 1.0, 99).unwrap();
        let top = (0..f.face_count()).find(|i| f.face_info(*i).map(|x| x.kind == SurfKind::Plane && x.normal[1] > 0.9).unwrap_or(false)).unwrap();
        let topc = (0..cut.face_count()).find(|i| cut.face_info(*i).map(|x| x.kind == SurfKind::Plane && x.normal[1] > 0.9).unwrap_or(false)).unwrap();
        let h = cut.shell(&[topc], 1.5, false, base, false).unwrap();
        assert!(h.is_valid() && h.volume() > 6000.0 && h.volume() < 6500.0, "hole: {}", h.volume());
        // the hole's rim rounded: the top meets the fillet tangent, its
        // wall cut through after the offset
        for (t, arc) in [(0.8, false), (0.8, true), (1.5, false), (1.5, true)] {
            match f.shell(&[top], t, false, base, arc) {
                Ok(h) => eprintln!("hole + fillet t {t} arc {arc}: valid {} volume {:.0} faces {}", h.is_valid(), h.volume(), h.face_count()),
                Err(e) => eprintln!("hole + fillet t {t} arc {arc}: {e}"),
            }
        }
    }
}
