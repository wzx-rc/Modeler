//! The link with Claude (plan point 22, step 2: "the app measures and
//! proposes, Claude chooses"). The MCP server `forme-nurbs-mcp` (started by
//! Claude Desktop) connects to the app on a local port (127.0.0.1:47830, or
//! FORME_NURBS_PORT) and sends one JSON request per line:
//! `{"method": "list_features", "params": {...}}`. The app answers one JSON
//! line: `{"ok": ...}` or `{"error": "..."}`. Requests are handled on the UI
//! thread, once per frame (`poll`); those that need the part built, the
//! reference loaded or the deviation measured wait for it (at most
//! `WAIT_SECS`); pictures are rendered by the host (`Capture`).
//!
//! Only this computer can connect (the port listens on 127.0.0.1).
//!
//! Shapes are given and returned as specs (`spec_of`, `apply_spec`): op,
//! kind, the base's centre (`origin`), local X (`x_axis`) and the way the
//! shape rises (`up`, local Y), then per kind its sizes (a cube's `size`
//! [x, y, z]; a cylinder's `radius`, `height`; a sketch's `outline` in mm
//! in its plane (x along `x_axis`, z along `x_axis × up`), `corners` (a
//! fillet radius > 0, a chamfer < 0, per point) and `height`). Units: mm.

use crate::camera::OrbitCamera;
use crate::state::AppState;
use glam::{DQuat, DVec3, Vec3};
use nurbs_kernel::doc::{Feature, Profile};
use nurbs_kernel::{Op, Prim};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

pub const DEFAULT_PORT: u16 = 47830;

/// How long a request waits for the part, the reference or the deviation
/// before it is answered as things are (s).
const WAIT_SECS: f64 = 120.0;

pub struct Request {
    pub method: String,
    pub params: Value,
    reply: Sender<String>,
    since: Instant,
}

impl Request {
    fn answer(self, v: Result<Value, String>) {
        let line = match v {
            Ok(v) => json!({ "ok": v }),
            Err(e) => json!({ "error": e }),
        };
        let _ = self.reply.send(line.to_string());
    }
}

/// A picture asked for: the host renders the 3D view alone (no interface)
/// with `camera` at `size` (pixels) and calls `finish_capture`.
pub struct Capture {
    req: Request,
    pub size: [u32; 2],
    pub camera: OrbitCamera,
    /// The reference shown, Ghost, and the deviation on, for this picture.
    pub ref_shown: bool,
    pub ref_ghost: bool,
    pub dev_on: bool,
}

/// The last picture's camera and size: `pick` reads spots of it.
#[derive(Clone, Copy)]
struct Shot {
    camera: OrbitCamera,
    size: [u32; 2],
}

#[derive(Default)]
pub struct Link {
    rx: Option<Receiver<Request>>,
    waiting: Vec<Request>,
    pub captures: Vec<Capture>,
    pub port: Option<u16>,
    /// Why the link could not start.
    pub error: Option<String>,
    /// Requests answered.
    pub served: usize,
    last_shot: Option<Shot>,
}

impl Link {
    pub fn listening(&self) -> bool {
        self.rx.is_some()
    }
    /// Requests waiting (for the part, a picture): keep the frames coming.
    pub fn busy(&self) -> bool {
        !self.waiting.is_empty() || !self.captures.is_empty()
    }
}

/// Listens on the local port; `wake` asks the host for a frame (the window
/// sleeps when nothing moves).
pub fn start(st: &mut AppState, wake: Arc<dyn Fn() + Send + Sync>) -> Result<u16, String> {
    let port = std::env::var("FORME_NURBS_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_PORT);
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            let e = format!("port {port}: {e}");
            st.link.error = Some(e.clone());
            return Err(e);
        }
    };
    let (tx, rx) = channel::<Request>();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let tx = tx.clone();
            let w = wake.clone();
            std::thread::spawn(move || serve(conn, tx, w));
        }
    });
    st.link.rx = Some(rx);
    st.link.port = Some(port);
    Ok(port)
}

fn serve(conn: TcpStream, tx: Sender<Request>, wake: Arc<dyn Fn() + Send + Sync>) {
    let Ok(read) = conn.try_clone() else { return };
    let mut out = conn;
    for line in BufReader::new(read).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(out, "{}", json!({ "error": format!("bad request: {e}") }));
                continue;
            }
        };
        let (rtx, rrx) = channel();
        let req = Request { method: v["method"].as_str().unwrap_or("").to_string(), params: v.get("params").cloned().unwrap_or(json!({})), reply: rtx, since: Instant::now() };
        if tx.send(req).is_err() {
            return;
        }
        wake();
        // the app wakes again while it waits (the part, a picture)
        let answer = loop {
            match rrx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(a) => break a,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => wake(),
                Err(_) => break json!({ "error": "the app closed" }).to_string(),
            }
        };
        if writeln!(out, "{answer}").is_err() {
            return;
        }
    }
}

/// What a request waits for.
fn waits_for(method: &str) -> (bool, bool, bool) {
    // (the part built, the reference loaded, the deviation measured)
    match method {
        "capture" | "pick" => (true, true, true),
        "deviation" => (true, true, true),
        "measure" | "primary_candidates" => (false, true, false),
        "edge_candidates" | "level_candidates" => (true, true, false),
        // its edges are found on the part as built from the document
        "add_fillets" => (true, false, false),
        "list_features" => (true, false, false),
        "status" => (false, false, false),
        _ => (false, false, false),
    }
}

/// Handles what came in, and what waited for the part.
pub fn poll(st: &mut AppState) {
    let mut reqs = std::mem::take(&mut st.link.waiting);
    if let Some(rx) = &st.link.rx {
        while let Ok(r) = rx.try_recv() {
            reqs.push(r);
        }
    }
    for r in reqs {
        let (part, reference, dev) = waits_for(&r.method);
        // the deviation asked: on (and measured) first
        let wants_dev = r.method == "deviation" || (r.method == "capture" && r.params.get("deviation").and_then(|v| v.as_bool()) == Some(true));
        if wants_dev && !st.dev_on && st.reference.is_some() {
            st.set_deviation(true);
        }
        let dev_off = r.params.get("deviation").and_then(|v| v.as_bool()) == Some(false);
        let wait = (part && st.building()) || (reference && st.ref_busy()) || (dev && !dev_off && st.dev_busy());
        if wait && r.since.elapsed().as_secs_f64() < WAIT_SECS {
            st.link.waiting.push(r);
            continue;
        }
        st.link.served += 1;
        handle(st, r);
    }
}

fn handle(st: &mut AppState, r: Request) {
    let p = r.params.clone();
    let out = match r.method.as_str() {
        "status" => Ok(status(st)),
        "open_reference" => open_reference(st, &p),
        "primary_candidates" => primary_candidates(st, &p),
        "list_features" => Ok(list_features(st, &p)),
        "add_features" => add_features(st, &p),
        "edit_feature" => edit_feature(st, &p),
        "delete_features" => delete_features(st, &p),
        "undo" => Ok(undo(st, &p)),
        "measure" => measure(st, &p),
        "edge_candidates" => edge_candidates(st, &p),
        "level_candidates" => level_candidates(st, &p),
        "add_fillets" => add_fillets(st, &p),
        "deviation" => deviation(st, &p),
        "show" => show(st, &p).map(|_| status(st)),
        "pick" => pick(st, &p),
        "capture" => match capture_of(st, &p) {
            Ok(c) => {
                st.link.captures.push(Capture { req: r, ..c });
                return;
            }
            Err(e) => Err(e),
        },
        m => Err(format!("unknown request {m}")),
    };
    r.answer(out);
}

// ----------------------------------------------------------------- values

fn v3(v: &Value) -> Option<DVec3> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some(DVec3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?))
}

fn r3(v: DVec3) -> Value {
    let r = |x: f64| (x * 1000.0).round() / 1000.0 + 0.0;
    json!([r(v.x), r(v.y), r(v.z)])
}

fn r(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0 + 0.0
}

fn op_name(op: Op) -> &'static str {
    match op {
        Op::Add => "add",
        Op::Carve => "carve",
    }
}

/// The world axis names "+X", "-Z"… or a direction.
fn axis_named(v: &Value) -> Option<DVec3> {
    if let Some(s) = v.as_str() {
        let s = s.trim().to_ascii_uppercase();
        let (sign, a) = if let Some(a) = s.strip_prefix('-') { (-1.0, a.to_string()) } else { (1.0, s.trim_start_matches('+').to_string()) };
        let d = match a.as_str() {
            "X" => DVec3::X,
            "Y" => DVec3::Y,
            "Z" => DVec3::Z,
            _ => return None,
        };
        return Some(d * sign);
    }
    v3(v)
}

/// Local X and Y (`x_axis`, `up`) made square: the rotation.
fn frame(x_axis: DVec3, up: DVec3) -> Result<DQuat, String> {
    let y = up.normalize_or_zero();
    if y == DVec3::ZERO {
        return Err("up is zero".into());
    }
    let mut x = x_axis - y * y.dot(x_axis);
    if x.length() < 1e-6 {
        // any square to up
        x = if y.x.abs() < 0.9 { DVec3::X - y * y.x } else { DVec3::Z - y * y.z };
    }
    Ok(crate::state::basis_rotation(x.normalize(), y))
}

/// A feature as a spec (what `add_features` takes back).
pub fn spec_of(f: &Feature) -> Value {
    let x_axis = f.rot * DVec3::X;
    let up = f.rot * DVec3::Y;
    let mut o = json!({
        "id": f.id,
        "name": f.name,
        "op": op_name(f.op),
        "kind": f.kind_label().to_ascii_lowercase(),
        "visible": f.visible,
        "material": f.material,
    });
    if f.suspended {
        o["suspended"] = json!(true);
    }
    if let Some(rd) = &f.round {
        o["kind"] = json!(if rd.chamfer { "chamfer" } else { "fillet" });
        o["size"] = json!(r(rd.size));
        o["edges"] = json!(rd.edges.iter().map(|e| r3(e.at)).collect::<Vec<_>>());
        if rd.tension > 0.0 {
            o["tension"] = json!(r(rd.tension));
        }
        return o;
    }
    if let Some(pu) = &f.push {
        o["kind"] = json!(pu.label().to_ascii_lowercase());
        o["inset"] = json!(r(pu.inset));
        o["depth"] = json!(r(pu.depth));
        o["face_at"] = r3(pu.face.at);
        return o;
    }
    if let Some(t) = &f.tweak {
        o["kind"] = json!(t.label().to_ascii_lowercase());
        o["at"] = r3(t.at());
        o["delta"] = r3(t.delta);
        return o;
    }
    if f.cut.is_some() {
        o["kind"] = json!("knife");
        o["at"] = r3(f.origin);
        return o;
    }
    o["origin"] = r3(f.origin);
    o["x_axis"] = r3(x_axis);
    o["up"] = r3(up);
    match f.prim {
        Prim::Box { x, y, z } => {
            o["kind"] = json!("cube");
            o["size"] = json!([r(x), r(y), r(z)]);
        }
        Prim::Cylinder { radius, height } => {
            o["kind"] = json!("cylinder");
            o["radius"] = json!(r(radius));
            o["height"] = json!(r(height));
        }
        Prim::Profile { x, y, z } => {
            o["kind"] = json!("sketch");
            o["height"] = json!(r(y));
            if let Some(pr) = &f.profile {
                o["outline"] = json!(pr.scaled(x, z).iter().map(|q| [r(q[0]), r(q[1])]).collect::<Vec<_>>());
                if pr.corner.iter().any(|c| *c != 0.0) {
                    o["corners"] = json!(pr.corner.iter().map(|c| r(*c)).collect::<Vec<_>>());
                }
                if pr.smooth.iter().any(|s| *s) {
                    o["smooth"] = json!(pr.smooth);
                }
            }
        }
        Prim::Path { x, z } => {
            o["kind"] = json!("path");
            if let Some(pr) = &f.profile {
                o["outline"] = json!(pr.scaled(x, z).iter().map(|q| [r(q[0]), r(q[1])]).collect::<Vec<_>>());
                o["open"] = json!(pr.open);
            }
        }
    }
    let (lo, hi) = f.bounds();
    o["bounds"] = json!([r3(lo), r3(hi)]);
    o
}

/// A shape from a spec, over `base` (an edit) or new. Only primitives
/// (cube, cylinder, sketch).
fn apply_spec(spec: &Value, base: Option<&Feature>) -> Result<(Op, Prim, DVec3, DQuat, Option<Profile>), String> {
    let op = match spec.get("op").and_then(|v| v.as_str()) {
        Some("add") | Some("Add") => Op::Add,
        Some("carve") | Some("Carve") | Some("cut") => Op::Carve,
        Some(o) => return Err(format!("op {o}: add or carve")),
        None => base.map(|f| f.op).unwrap_or(Op::Add),
    };
    let kind = spec.get("kind").and_then(|v| v.as_str()).map(|s| s.to_ascii_lowercase()).or_else(|| {
        base.map(|f| match f.prim {
            Prim::Box { .. } => "cube".to_string(),
            Prim::Cylinder { .. } => "cylinder".to_string(),
            Prim::Profile { .. } => "sketch".to_string(),
            Prim::Path { .. } => "path".to_string(),
        })
    });
    let kind = kind.ok_or("kind: cube, cylinder or sketch")?;
    let mut origin = spec.get("origin").and_then(v3).or(base.map(|f| f.origin)).ok_or("origin [x, y, z] (the base's centre) is missing")?;
    let up = spec.get("up").and_then(axis_named).or(base.map(|f| f.rot * DVec3::Y)).unwrap_or(DVec3::Y);
    let x_axis = spec.get("x_axis").and_then(axis_named).or(base.map(|f| f.rot * DVec3::X)).unwrap_or(DVec3::X);
    let rot = frame(x_axis, up)?;
    let num = |k: &str| spec.get(k).and_then(|v| v.as_f64());
    let pos = |k: &str, v: Option<f64>| -> Result<f64, String> {
        match v {
            Some(x) if x > 1e-4 && x.is_finite() => Ok(x),
            Some(x) => Err(format!("{k} {x}: must be > 0")),
            None => Err(format!("{k} is missing")),
        }
    };
    match kind.as_str() {
        "cube" | "box" | "block" => {
            let old = match base.map(|f| f.prim) {
                Some(Prim::Box { x, y, z }) => Some([x, y, z]),
                _ => None,
            };
            let s = match spec.get("size").and_then(|v| v.as_array()) {
                Some(a) if a.len() == 3 => [a[0].as_f64(), a[1].as_f64(), a[2].as_f64()],
                Some(_) => return Err("size: [x, y, z]".into()),
                None => old.map(|o| [Some(o[0]), Some(o[1]), Some(o[2])]).unwrap_or([None, None, None]),
            };
            Ok((op, Prim::Box { x: pos("size x", s[0])?, y: pos("size y", s[1])?, z: pos("size z", s[2])? }, origin, rot, None))
        }
        "cylinder" => {
            let (or, oh) = match base.map(|f| f.prim) {
                Some(Prim::Cylinder { radius, height }) => (Some(radius), Some(height)),
                _ => (None, None),
            };
            let radius = num("radius").or(num("diameter").map(|d| d * 0.5)).or(or);
            Ok((op, Prim::Cylinder { radius: pos("radius", radius)?, height: pos("height", num("height").or(oh))? }, origin, rot, None))
        }
        "sketch" | "profile" | "outline" => {
            let (old_pts, old_h, old_corner) = match (base.map(|f| f.prim), base.and_then(|f| f.profile.as_ref())) {
                (Some(Prim::Profile { x, y, z }), Some(pr)) => (Some(pr.scaled(x, z)), Some(y), pr.corner.clone()),
                _ => (None, None, vec![]),
            };
            let pts: Vec<[f64; 2]> = match spec.get("outline").and_then(|v| v.as_array()) {
                Some(a) => a.iter().map(|q| Some([q.get(0)?.as_f64()?, q.get(1)?.as_f64()?])).collect::<Option<Vec<_>>>().ok_or("outline: [[x, z], …] in mm")?,
                None => old_pts.ok_or("outline [[x, z], …] is missing")?,
            };
            if pts.len() < 3 {
                return Err("outline: at least 3 points".into());
            }
            let n = pts.len();
            let corner: Vec<f64> = match spec.get("corners").and_then(|v| v.as_array()) {
                Some(a) => {
                    let c: Vec<f64> = a.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect();
                    if c.len() != n {
                        return Err(format!("corners: one per outline point ({n}), got {}", c.len()));
                    }
                    c
                }
                None if spec.get("outline").is_none() => old_corner,
                None => vec![],
            };
            let height = pos("height", num("height").or(old_h))?;
            // the outline's box centred on the origin (its points in the
            // unit square), the origin moved there
            let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for q in &pts {
                for k in 0..2 {
                    lo[k] = lo[k].min(q[k]);
                    hi[k] = hi[k].max(q[k]);
                }
            }
            let (sx, sz) = (hi[0] - lo[0], hi[1] - lo[1]);
            if !(sx > 1e-3 && sz > 1e-3) {
                return Err("outline: it is flat (no area)".into());
            }
            let (cx, cz) = ((lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5);
            origin += rot * DVec3::new(cx, 0.0, cz);
            let points: Vec<[f64; 2]> = pts.iter().map(|q| [(q[0] - cx) / sx, (q[1] - cz) / sz]).collect();
            let corner = if corner.iter().any(|c| *c != 0.0) { corner } else { vec![] };
            let profile = Profile { points, smooth: vec![false; n], corner, tension: vec![], open: false };
            Ok((op, Prim::Profile { x: sx, y: height, z: sz }, origin, rot, Some(profile)))
        }
        k => Err(format!("kind {k}: cube, cylinder or sketch")),
    }
}

// --------------------------------------------------------------- requests

fn status(st: &AppState) -> Value {
    let m = st.model.as_ref();
    let rf = st.reference.as_ref().map(|rr| {
        json!({ "path": rr.path, "triangles": rr.triangles(), "bounds": [r3(rr.lo), r3(rr.hi)], "grain_mm": r(rr.edge), "shown": st.ref_shown, "ghost": st.ref_ghost })
    });
    let c = st.camera;
    json!({
        "app": crate::version_title(),
        "document": st.doc_name,
        "features": st.doc.features.len(),
        "reference": rf,
        "reference_loading": st.ref_loading.as_ref().map(|l| l.path.clone()),
        "reference_failed": st.ref_failed,
        "building": st.building(),
        "model": m.map(|m| json!({ "faces": m.faces.len(), "edges": m.edges.len(), "errors": m.errors, "fine": m.fine, "kernel_ms": m.ms })),
        "deviation_on": st.dev_on,
        "undo": st.undo_labels().into_iter().rev().take(8).collect::<Vec<_>>(),
        "camera": { "target": r3(c.target.as_dvec3()), "yaw_deg": r(c.yaw.to_degrees() as f64), "pitch_deg": r(c.pitch.to_degrees() as f64), "distance": r(c.dist as f64) },
    })
}

fn open_reference(st: &mut AppState, p: &Value) -> Result<Value, String> {
    let path = p.get("path").and_then(|v| v.as_str()).ok_or("path of the .obj is missing")?;
    if !std::path::Path::new(path).exists() {
        return Err(format!("{path}: no such file"));
    }
    st.import_reference(path);
    Ok(json!({ "loading": path, "note": "it loads in the background: measure, primary_candidates and capture wait for it" }))
}

/// Stage 1 (the "Primary shapes" button) as candidates; `apply`: made as
/// the button makes them.
fn primary_candidates(st: &mut AppState, p: &Value) -> Result<Value, String> {
    let rf = st.reference.clone().ok_or("no reference loaded (open_reference first)")?;
    if p.get("apply").and_then(|v| v.as_bool()) == Some(true) {
        let before = st.doc.features.len();
        st.read_primaries();
        let made: Vec<Value> = st.doc.features[before..].iter().map(spec_of).collect();
        return Ok(json!({ "made": made }));
    }
    let t0 = Instant::now();
    let pr = rectify_core::primary::read(&rf.mesh, rf.edge);
    let mut out = vec![];
    for (k, s) in pr.parts.iter().enumerate() {
        let Some(x) = crate::scan::primary_step(&pr, s) else { continue };
        let f = Feature {
            id: 0,
            name: format!("Block candidate {}", k + 1),
            op: Op::Add,
            prim: x.prim,
            origin: x.origin,
            rot: x.rot,
            visible: true,
            material: 0,
            suspended: false,
            round: None,
            profile: x.profile.clone(),
            push: None,
            tweak: None,
            cut: None,
            sheet: None,
        };
        let mut v = spec_of(&f);
        if let Some(o) = v.as_object_mut() {
            o.remove("id");
            o.remove("visible");
            o.remove("material");
        }
        v["read"] = json!({
            "area_mm2": r(s.area),
            "flat_share": r(s.flat),
            "back": r(s.back),
            "front": r(s.front),
            "corners_read": s.outline.len(),
        });
        out.push(v);
    }
    Ok(json!({
        "side_axis": r3(pr.w),
        "grid_step_mm": r(pr.px),
        "ms": r(t0.elapsed().as_secs_f64() * 1e3),
        "candidates": out,
        "note": "each candidate is a spec add_features takes as it is (edit its values first: round them, make the flanks symmetric…)",
    }))
}

fn list_features(st: &AppState, p: &Value) -> Value {
    let ids: Option<Vec<u64>> = p.get("ids").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_u64()).collect());
    let list: Vec<Value> = st.doc.features.iter().filter(|f| ids.as_ref().map(|i| i.contains(&(f.id as u64))).unwrap_or(true)).map(spec_of).collect();
    json!({ "features": list, "errors": st.model.as_ref().map(|m| m.errors.clone()).unwrap_or_default() })
}

fn add_features(st: &mut AppState, p: &Value) -> Result<Value, String> {
    let specs = p.get("features").and_then(|v| v.as_array()).ok_or("features: [spec, …]")?;
    let mut made = vec![];
    for (k, s) in specs.iter().enumerate() {
        made.push((apply_spec(s, None).map_err(|e| format!("feature {k}: {e}"))?, s));
    }
    let label = p.get("label").and_then(|v| v.as_str()).unwrap_or("Shapes by Claude").to_string();
    let mat = st.material;
    let mut ids = vec![];
    st.edit(&label, |d| {
        for ((op, prim, origin, rot, profile), s) in &made {
            let id = d.add(*op, *prim, *origin, *rot);
            let base = s.get("name").and_then(|v| v.as_str()).unwrap_or(match prim {
                Prim::Box { .. } => "Block",
                Prim::Cylinder { .. } => "Cylinder",
                _ => "Shape",
            });
            let name = d.fresh_name(base);
            if let Some(f) = d.feature_mut(id) {
                f.name = name;
                f.profile = profile.clone();
                f.material = s.get("material").and_then(|v| v.as_u64()).map(|m| m as u32).unwrap_or(mat);
            }
            ids.push(id);
        }
    });
    st.selection = vec![];
    Ok(json!({ "ids": ids, "note": "one undo step; the part is built in the background (capture and deviation wait for it)" }))
}

fn edit_feature(st: &mut AppState, p: &Value) -> Result<Value, String> {
    let id = p.get("id").and_then(|v| v.as_u64()).ok_or("id is missing")? as u32;
    let f = st.doc.feature(id).ok_or(format!("no feature {id}"))?.clone();
    if !f.is_solid() {
        // a step: its size only
        let size = p.get("size").and_then(|v| v.as_f64());
        let label = format!("Edit {}", f.name);
        st.edit(&label, |d| {
            if let Some(g) = d.feature_mut(id) {
                if let (Some(rd), Some(s)) = (g.round.as_mut(), size) {
                    rd.size = s.max(0.01);
                }
                if let Some(pu) = g.push.as_mut() {
                    if let Some(i) = p.get("inset").and_then(|v| v.as_f64()) {
                        pu.inset = i.max(0.0);
                    }
                    if let Some(dp) = p.get("depth").and_then(|v| v.as_f64()) {
                        pu.depth = dp;
                    }
                }
                if let Some(v) = p.get("visible").and_then(|v| v.as_bool()) {
                    g.visible = v;
                }
            }
        });
        return Ok(spec_of(st.doc.feature(id).ok_or("gone")?));
    }
    let (op, prim, origin, rot, profile) = apply_spec(p, Some(&f))?;
    let label = format!("Edit {}", f.name);
    st.edit(&label, |d| {
        if let Some(g) = d.feature_mut(id) {
            g.op = op;
            g.prim = prim;
            g.origin = origin;
            g.rot = rot;
            if profile.is_some() {
                g.profile = profile;
            }
            if let Some(n) = p.get("name").and_then(|v| v.as_str()) {
                g.name = n.to_string();
            }
            if let Some(v) = p.get("visible").and_then(|v| v.as_bool()) {
                g.visible = v;
            }
            if let Some(m) = p.get("material").and_then(|v| v.as_u64()) {
                g.material = m as u32;
            }
        }
    });
    Ok(spec_of(st.doc.feature(id).ok_or("gone")?))
}

fn delete_features(st: &mut AppState, p: &Value) -> Result<Value, String> {
    let ids: Vec<u32> = p.get("ids").and_then(|v| v.as_array()).ok_or("ids: [id, …]")?.iter().filter_map(|v| v.as_u64()).map(|v| v as u32).collect();
    let missing: Vec<u32> = ids.iter().copied().filter(|i| st.doc.feature(*i).is_none()).collect();
    if !missing.is_empty() {
        return Err(format!("no features {missing:?}"));
    }
    st.edit("Delete (Claude)", |d| d.remove(&ids));
    st.selection.retain(|i| !ids.contains(i));
    Ok(json!({ "deleted": ids, "left": st.doc.features.len() }))
}

fn undo(st: &mut AppState, p: &Value) -> Value {
    let n = p.get("steps").and_then(|v| v.as_u64()).unwrap_or(1).clamp(1, 50);
    let redo = p.get("redo").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut done = 0;
    for _ in 0..n {
        if redo && st.can_redo() {
            st.redo();
            done += 1;
        } else if !redo && st.can_undo() {
            st.undo();
            done += 1;
        }
    }
    json!({ "done": done, "features": st.doc.features.len(), "undo": st.undo_labels().into_iter().rev().take(8).collect::<Vec<_>>() })
}

/// A spec for a cylinder read (`RoundRead`).
fn round_spec(x: &crate::scan::RoundRead) -> Value {
    let rot = frame(DVec3::X, x.axis).unwrap_or(DQuat::IDENTITY);
    json!({
        "op": op_name(x.op), "kind": "cylinder", "origin": r3(x.origin), "x_axis": r3(rot * DVec3::X), "up": r3(x.axis),
        "radius": r(x.radius), "height": r(x.height), "read_depth": r(x.depth),
    })
}

/// "Measure a cylinder, a slot, a block or a plane here" (`a`, `b`: points
/// on or near the scan, as the tools' drags: rim to rim across a round part,
/// end to end along a slot, corner to corner across a block).
fn measure(st: &mut AppState, p: &Value) -> Result<Value, String> {
    let rf = st.reference.clone().ok_or("no reference loaded (open_reference first)")?;
    let kind = p.get("kind").and_then(|v| v.as_str()).ok_or("kind: round, slot, box or plane")?.to_string();
    let a = p.get("a").and_then(v3).ok_or("a: [x, y, z]")?;
    let near = |q: DVec3| rf.bvh.closest(&rf.mesh, q, 10.0).ok_or(format!("{} is more than 10 mm from the scan", r3(q)));
    let (ta, pa) = near(a)?;
    if kind == "plane" {
        let reach = p.get("reach").and_then(|v| v.as_f64()).unwrap_or(12.0);
        let (pl, flat) = rectify_core::planes::plane_under(&rf.mesh, &rf.topo, ta, pa, reach, rf.edge);
        return Ok(json!({
            "flat_face": flat,
            "point": r3(pl.c), "normal": r3(pl.n), "normal_snapped": r3(crate::scan::snap_axis(pl.n)),
            "rms_mm": r(pl.rms), "area_mm2": r(pl.area), "offset_along_normal": r(pl.n.dot(pl.c)),
            "note": if flat { "a flat face of the scan" } else { "no flat face there: the tangent plane" },
        }));
    }
    let b = p.get("b").and_then(v3).ok_or("b: [x, y, z]")?;
    let (_, pb) = near(b)?;
    let t0 = Instant::now();
    let found = match kind.as_str() {
        "round" | "cylinder" | "hole" | "boss" => rectify_core::slot::Slot::detect_round(&rf.mesh, &rf.topo, ta, pa, pb, rf.edge),
        "slot" | "pill" => rectify_core::slot::Slot::detect(&rf.mesh, &rf.topo, ta, pa, pb, rf.edge),
        "box" | "block" | "pocket" => rectify_core::slot::Slot::detect_box(&rf.mesh, &rf.topo, ta, pa, pb, rf.edge),
        k => return Err(format!("kind {k}: round, slot, box or plane")),
    };
    let s = found.map_err(|e| format!("nothing read there: {e}"))?;
    let ms = r(t0.elapsed().as_secs_f64() * 1e3);
    let mut out = json!({
        "a": r3(s.a), "b": r3(s.b), "axis": r3(s.axis), "radius": r(s.r), "half_width": r(s.w), "depth": r(s.depth), "inward": s.inward, "ms": ms,
    });
    if !s.tiers.is_empty() {
        out["tiers"] = json!(s.tiers.iter().map(|t| json!({ "centre": r3(t.centre), "radius": r(t.r), "from": r(t.from), "to": r(t.to) })).collect::<Vec<_>>());
    }
    let specs: Vec<Value> = if kind == "round" || kind == "cylinder" || kind == "hole" || kind == "boss" {
        crate::scan::round_steps(&s).iter().map(round_spec).collect()
    } else {
        crate::scan::outline_step(&s)
            .map(|x| {
                let pr = x.profile();
                let pts: Vec<[f64; 2]> = pr.scaled(x.x, x.z);
                vec![json!({
                    "op": op_name(x.op), "kind": "sketch", "origin": r3(x.origin), "x_axis": r3(x.rot * DVec3::X), "up": r3(x.rot * DVec3::Y),
                    "outline": pts.iter().map(|q| [r(q[0]), r(q[1])]).collect::<Vec<_>>(), "corners": [r(x.r), r(x.r), r(x.r), r(x.r)], "height": r(x.height), "read_depth": r(x.depth),
                })]
            })
            .unwrap_or_default()
    };
    out["specs"] = json!(specs);
    Ok(out)
}

/// Stage 2 (secondary): the part's sharp edges read on the scan, a fillet
/// radius each (or why none), longest first.
fn edge_candidates(st: &mut AppState, p: &Value) -> Result<Value, String> {
    if st.reference.is_none() {
        return Err("no reference loaded".into());
    }
    let m = st.model.clone().ok_or("no part built")?;
    let only: Option<Vec<u32>> = p.get("features").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_u64()).map(|v| v as u32).collect());
    let min_len = p.get("min_length").and_then(|v| v.as_f64()).unwrap_or(3.0);
    let t0 = Instant::now();
    let mut reads = st.read_edges(only.as_deref(), min_len);
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    reads.sort_by(|a, b| b.len.total_cmp(&a.len));
    let name = |f: Option<u32>| f.and_then(|i| st.doc.feature(i)).map(|g| g.name.clone());
    let mut found = vec![];
    let mut sharp = vec![];
    let mut none = 0;
    for e in &reads {
        let pts = &m.edges[e.edge].pts;
        let (a, b) = (Vec3::from(pts[0]).as_dvec3(), Vec3::from(pts[pts.len() - 1]).as_dvec3());
        let at = crate::state::polyline_middle(pts).unwrap_or((a + b) * 0.5);
        // where the scan's two faces lie from the part's (the slices'
        // median): a fillet sits off the scan by as much as its faces
        let off = |k: usize, f: &rectify_core::fillet::FilletRead| {
            let mut o: Vec<f64> = f.slices.iter().map(|s| s.off[k]).collect();
            o.sort_by(|x, y| x.total_cmp(y));
            o.get(o.len() / 2).copied().unwrap_or(0.0)
        };
        match &e.read {
            Ok(f) => found.push(json!({
                "at": r3(at), "from": r3(a), "to": r3(b), "length": r(e.len), "turn_deg": r(e.turn),
                "convex": f.convex, "radius": r(f.r), "lo": r(f.lo), "hi": r(f.hi), "slices": f.slices.len(), "tried": f.tried, "varies": f.varies(),
                "faces_off": [r(off(0, f)), r(off(1, f))],
                "features": [e.features[0], e.features[1]], "names": [name(e.features[0]), name(e.features[1])],
            })),
            Err(why) if why.contains("sharp") => sharp.push(json!({ "at": r3(at), "length": r(e.len), "features": [e.features[0], e.features[1]] })),
            Err(_) => none += 1,
        }
    }
    let limit = p.get("limit").and_then(|v| v.as_u64()).unwrap_or(60) as usize;
    let total = found.len();
    Ok(json!({
        "edges_read": reads.len(), "ms": r(ms),
        "fillets": found.into_iter().take(limit).collect::<Vec<_>>(), "fillets_total": total,
        "sharp_on_scan": sharp.into_iter().take(limit).collect::<Vec<_>>(),
        "not_read": none,
        "note": "radius: the median of the slices kept (lo..hi their spread; varies: more than a quarter of it); faces_off: where the scan's two faces lie from the part's two faces there, along their normals (mm, + the scan's face further out: the part is short; − the part's face sticks out): a fillet stays off the scan by about as much, a face off by more than ~0.5 mm is the block's to fix (a primary), not the fillet's; give fillets of one radius to edges that belong together (add_fillets), the biggest first; edges not read: no scan along them, a face far from the scan, a hose",
    }))
}

/// A level read on a face as a spec: a Cylinder for a round, else a
/// Sketch of its outline (its corners cut), pulled along the face's normal
/// from its base to its level; a plate begins 0.1 mm inside the face, a
/// pocket 0.1 mm outside it.
fn level_spec(l: &rectify_core::levels::Level, name: &str, round_to: f64) -> Value {
    use rectify_core::levels::Outline;
    let q = |x: f64| if round_to > 0.0 { (x / round_to).round() * round_to } else { x };
    let (base, level) = (q(l.base), q(l.level));
    const EPS: f64 = 0.1;
    let raised = level > base;
    // from the part's face when the base is past it (a plate on a level the
    // face does not have yet): never a shape floating off the part
    let fl = q(l.face_level);
    let (d0, h) = if raised {
        let from = base.min(fl) - EPS;
        (from, level - from)
    } else {
        let from = base.max(fl) + EPS;
        (level, from - level)
    };
    let op = if raised { "add" } else { "carve" };
    match &l.outline {
        Outline::Round { c, r: rad } => json!({
            "name": name, "op": op, "kind": "cylinder",
            "origin": r3(l.point(*c, d0)), "x_axis": r3(l.u), "up": r3(l.axis),
            "radius": r(q(*rad)), "height": r(h),
        }),
        o => {
            let (pts, cuts) = o.polygon().unwrap_or_default();
            let (lo, hi) = pts.iter().fold((glam::DVec2::splat(f64::INFINITY), glam::DVec2::splat(f64::NEG_INFINITY)), |(a, b), p| (a.min(*p), b.max(*p)));
            let c = (lo + hi) * 0.5;
            // the sketch's z runs along x_axis × up: ±v
            let flip = l.v.dot(l.u.cross(l.axis)).signum();
            let outline: Vec<Value> = pts.iter().map(|p| json!([r(p.x - c.x), r(flip * (p.y - c.y))])).collect();
            let corners: Vec<Value> = cuts.iter().map(|k| json!(r(*k))).collect();
            json!({
                "name": name, "op": op, "kind": "sketch",
                "origin": r3(l.point(c, d0)), "x_axis": r3(l.u), "up": r3(l.axis),
                "outline": outline, "corners": corners, "height": r(h),
            })
        }
    }
}

/// The detail: the scan's levels on the part's flat faces (recessed
/// panels, raised plates, pockets, bosses) as specs; `apply`: true for all
/// (but faces that are off as a whole), or the indices to add, in one undo
/// step.
fn level_candidates(st: &mut AppState, p: &Value) -> Result<Value, String> {
    if st.reference.is_none() {
        return Err("no reference loaded".into());
    }
    if st.model.is_none() {
        return Err("no part built".into());
    }
    let axes: Vec<DVec3> = match p.get("axes").and_then(|v| v.as_array()) {
        Some(a) => a.iter().map(|v| axis_named(v).ok_or(format!("axes: \"+X\", \"-Z\"…, got {v}"))).collect::<Result<_, _>>()?,
        None => vec![DVec3::X, -DVec3::X, DVec3::Y, -DVec3::Y, DVec3::Z, -DVec3::Z],
    };
    let only: Option<Vec<u32>> = p.get("features").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_u64()).map(|v| v as u32).collect());
    let mut opts = rectify_core::levels::Opts::default();
    if let Some(a) = p.get("min_area").and_then(|v| v.as_f64()) {
        opts.min_area = a.max(0.5);
    }
    if let Some(s) = p.get("min_step").and_then(|v| v.as_f64()) {
        opts.min_step = s.max(0.1);
    }
    let round_to = p.get("round_to").and_then(|v| v.as_f64()).unwrap_or(0.05).max(0.0);
    let t0 = Instant::now();
    let mut reads = st.read_levels(&axes, &opts, only.as_deref());
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    reads.sort_by(|a, b| (b.0.area * b.0.height()).total_cmp(&(a.0.area * a.0.height())));
    let name = |f: Option<u32>| f.and_then(|i| st.doc.feature(i)).map(|g| g.name.clone());
    let axis_name = |a: DVec3| {
        let s = if a.x + a.y + a.z > 0.0 { "+" } else { "-" };
        format!("{s}{}", if a.x.abs() > 0.5 { "X" } else if a.y.abs() > 0.5 { "Y" } else { "Z" })
    };
    let mut list = vec![];
    for (k, (l, f)) in reads.iter().enumerate() {
        let kind = if l.raised() { "Plate" } else { "Pocket" };
        let shape = l.outline.kind();
        let nm = match shape {
            "round" => if l.raised() { "Boss" } else { "Hole" },
            "slot" => if l.raised() { "Pill" } else { "Slot" },
            _ => kind,
        };
        list.push(json!({
            "index": k,
            "axis": axis_name(l.axis),
            "kind": if l.raised() { "raised" } else { "recessed" },
            "shape": shape,
            "on": { "feature": f, "name": name(*f), "face_level": r(l.face_level) },
            "base": r(l.base), "level": r(l.level), "height": r(l.height()),
            "depth": l.depth, "area_mm2": r(l.area), "cover": r(l.cover), "flat_share": r(l.flat), "on_level": r(l.near), "spread": if l.spread.is_finite() { json!(r(l.spread)) } else { Value::Null },
            "at": r3(l.at),
            "face_off": l.cover > 0.5,
            "spec": level_spec(l, nm, round_to),
        }));
    }
    let pick: Option<Vec<usize>> = match p.get("apply") {
        Some(Value::Bool(true)) => Some(list.iter().enumerate().filter(|(_, v)| v["face_off"] != json!(true)).map(|(k, _)| k).collect()),
        Some(Value::Array(a)) => Some(a.iter().filter_map(|v| v.as_u64()).map(|v| v as usize).filter(|&k| k < list.len()).collect()),
        _ => None,
    };
    if let Some(pick) = pick {
        if pick.is_empty() {
            return Ok(json!({ "made": [], "candidates": list.len(), "ms": r(ms) }));
        }
        // the steps on the face first, those in another step after it
        let mut pick = pick;
        pick.sort_by_key(|&k| (reads[k].0.depth, k));
        let specs: Vec<Value> = pick.iter().map(|&k| list[k]["spec"].clone()).collect();
        let label = p.get("label").and_then(|v| v.as_str()).unwrap_or("Levels from the scan");
        let out = add_features(st, &json!({ "features": specs, "label": label }))?;
        return Ok(json!({ "made": out["ids"], "from": pick, "candidates": list.len(), "ms": r(ms), "note": "one undo step; check with deviation" }));
    }
    let limit = p.get("limit").and_then(|v| v.as_u64()).unwrap_or(40) as usize;
    let total = list.len();
    Ok(json!({
        "ms": r(ms),
        "levels": list.into_iter().take(limit).collect::<Vec<_>>(),
        "total": total,
        "note": "each level is a step on a flat face of the part square to a world axis, read on the scan: its base (the face's level, or the step it lies in) and its level along the axis (mm), its shape (round, slot, rounded rectangle, rectangle, outline), and a spec add_features takes as it is (a plate begins 0.1 mm inside the face, a pocket 0.1 mm outside it; values rounded to round_to). Biggest first (area × height). face_off: it (with the steps of its level beside it) covers most of its face, the face itself is off: move the block's face (edit_feature) instead. Idealise before adding: one height for steps that belong together, the same size for a series, symmetric across the part's middle. Cables and hoses are not modelled.",
    }))
}

/// The model edge whose line passes nearest `p` (within `tol` mm).
fn edge_near(st: &AppState, p: DVec3, tol: f64) -> Option<usize> {
    let m = st.model.as_ref()?;
    let mut best = (tol, None);
    for (i, e) in m.edges.iter().enumerate() {
        if e.seam || e.faces[0] < 0 || e.faces[1] < 0 {
            continue;
        }
        for w in e.pts.windows(2) {
            let (a, b) = (Vec3::from(w[0]).as_dvec3(), Vec3::from(w[1]).as_dvec3());
            let ab = b - a;
            let t = if ab.length_squared() > 1e-12 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
            let d = p.distance(a + ab * t);
            if d < best.0 {
                best = (d, Some(i));
            }
        }
    }
    best.1
}

/// Fillet (or chamfer) steps on edges named by points on them (an edge
/// candidate's `at`), one step per group, all in one undo step.
fn add_fillets(st: &mut AppState, p: &Value) -> Result<Value, String> {
    let groups = p.get("groups").and_then(|v| v.as_array()).ok_or("groups: [{at: [[x,y,z], …], radius, chamfer?}, …]")?;
    let mut made = vec![];
    for (k, g) in groups.iter().enumerate() {
        let size = g.get("radius").or(g.get("size")).and_then(|v| v.as_f64()).ok_or(format!("group {k}: radius is missing"))?;
        if !(size > 0.0) {
            return Err(format!("group {k}: radius must be > 0"));
        }
        let chamfer = g.get("chamfer").and_then(|v| v.as_bool()).unwrap_or(false);
        let pts = g.get("at").and_then(|v| v.as_array()).ok_or(format!("group {k}: at: [[x,y,z], …]"))?;
        let mut edges = vec![];
        for q in pts {
            let q = v3(q).ok_or(format!("group {k}: a point is not [x,y,z]"))?;
            let i = edge_near(st, q, 1.5).ok_or(format!("group {k}: no edge of the part within 1.5 mm of {}", r3(q)))?;
            let e = st.edge_ref(i).ok_or(format!("group {k}: the edge at {} has no tagged faces", r3(q)))?;
            if !edges.contains(&e) {
                edges.push(e);
            }
        }
        made.push((size, chamfer, edges, g.get("name").and_then(|v| v.as_str()).map(|s| s.to_string())));
    }
    let label = p.get("label").and_then(|v| v.as_str()).unwrap_or("Fillets by Claude").to_string();
    let mut ids = vec![];
    st.edit(&label, |d| {
        for (size, chamfer, edges, name) in &made {
            let id = d.add_round(*size, *chamfer, edges.clone());
            if let (Some(n), Some(f)) = (name, d.feature_mut(id)) {
                f.name = n.clone();
            }
            ids.push(id);
        }
    });
    Ok(json!({ "ids": ids, "note": "one undo step; a fillet too big for its edges stops at the largest that works (list_features gives the size made)" }))
}

/// The deviation: on, its numbers overall and per feature, and the zones
/// still far from the scan (the next stage's work).
fn deviation(st: &mut AppState, p: &Value) -> Result<Value, String> {
    if st.reference.is_none() {
        return Err("no reference loaded".into());
    }
    if let Some(rg) = p.get("range").and_then(|v| v.as_f64()) {
        st.dev_range = rg.clamp(0.05, 5.0);
        st.dev_tol = (st.dev_range / 5.0).min(0.1);
    }
    if !st.dev_on {
        st.set_deviation(true);
    }
    if st.deviation_shown().is_none() {
        return Err("the deviation is still being measured: ask again in a few seconds".into());
    }
    let (Some(m), Some(d)) = (st.model.clone(), st.deviation.clone()) else {
        return Err("no deviation yet (the part is not built?)".into());
    };
    let tol = d.tol;
    let feat_of = |face: u32| m.faces.get(face as usize).and_then(|f| f.tag).map(|t| t.feature);
    // every statistic is weighted by area (the kernel cuts a fillet in many
    // small triangles: counted by point it would weigh more than a face)
    let w = crate::scan::dev_weights(&d.verts);
    let bx = p.get("box").and_then(|v| v.as_array()).and_then(|a| Some((v3(a.first()?)?, v3(a.get(1)?)?)));
    let mut per: std::collections::BTreeMap<u32, Vec<(f32, f64)>> = Default::default();
    let mut elem: std::collections::BTreeMap<u32, Vec<(f32, f64)>> = Default::default();
    let mut region: Vec<(f32, f64)> = vec![];
    let mut far: Vec<(DVec3, f32, u32)> = vec![];
    let mut on_face: std::collections::HashMap<u32, usize> = Default::default();
    let zone_min = p.get("zone_min").and_then(|v| v.as_f64()).unwrap_or(0.5) as f32;
    let mut element: std::collections::HashMap<u32, u32> = Default::default();
    for (v, w) in d.verts.iter().zip(&w) {
        let q = Vec3::from(v.pos).as_dvec3();
        if let Some((lo, hi)) = bx {
            if q.cmplt(lo.min(hi)).any() || q.cmpgt(lo.max(hi)).any() {
                continue;
            }
            region.push((v.dev, *w));
        }
        if let Some(f) = feat_of(v.face) {
            per.entry(f).or_default().push((v.dev, *w));
            let e = *element.entry(f).or_insert_with(|| st.element_of(f));
            elem.entry(e).or_default().push((v.dev, *w));
        }
        if v.dev == crate::render::NO_DEV {
            continue;
        }
        *on_face.entry(v.face).or_default() += 1;
        if v.dev.abs() > zone_min {
            far.push((q, v.dev, v.face));
        }
    }
    let stats = |ds: &mut Vec<(f32, f64)>| {
        let s = crate::scan::dev_stats(ds, tol);
        json!({ "area": r(s.area), "measured": r(s.reach), "within": r(s.within), "outside": r(s.outside), "inside": r(s.inside), "median": r(s.p50), "p95": r(s.p95), "mean": r(s.mean) })
    };
    let mut features = vec![];
    for (id, ds) in per.iter_mut() {
        let name = st.doc.feature(*id).map(|f| f.name.clone()).unwrap_or_default();
        let mut s = stats(ds);
        s["id"] = json!(id);
        s["name"] = json!(name);
        if element.get(id).is_some_and(|e| e != id) {
            s["element"] = json!(element[id]);
        }
        features.push(s);
    }
    // an element with operations: its faces and theirs together (a block
    // and its fillets: compare with the block before them)
    let mut elements = vec![];
    for (id, ds) in elem.iter_mut() {
        let ops: Vec<u32> = element.iter().filter(|(f, e)| *e == id && *f != id).map(|(f, _)| *f).collect();
        if ops.is_empty() {
            continue;
        }
        let mut s = stats(ds);
        s["id"] = json!(id);
        s["name"] = json!(st.doc.feature(*id).map(|f| f.name.clone()).unwrap_or_default());
        s["operations"] = json!(ops);
        elements.push(s);
    }
    // the zones: far points of one face of the part, one side, joined
    // over 3 mm cells (a face too far out, a band along an edge the scan
    // rounds off, a detail missing)
    let cell = p.get("zone_cell").and_then(|v| v.as_f64()).unwrap_or(3.0).max(0.5);
    let key = |q: DVec3| ((q.x / cell).floor() as i64, (q.y / cell).floor() as i64, (q.z / cell).floor() as i64);
    type Key = ((i64, i64, i64), bool, u32);
    let mut cells: std::collections::HashMap<Key, Vec<usize>> = Default::default();
    for (i, (q, dv, face)) in far.iter().enumerate() {
        cells.entry((key(*q), *dv > 0.0, *face)).or_default().push(i);
    }
    let mut seen: std::collections::HashSet<Key> = Default::default();
    let mut zones = vec![];
    let keys: Vec<_> = cells.keys().copied().collect();
    for k0 in keys {
        if !seen.insert(k0) {
            continue;
        }
        let mut stack = vec![k0];
        let mut members = vec![];
        while let Some(k) = stack.pop() {
            members.extend(cells[&k].iter().copied());
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let n = ((k.0 .0 + dx, k.0 .1 + dy, k.0 .2 + dz), k.1, k.2);
                        if cells.contains_key(&n) && seen.insert(n) {
                            stack.push(n);
                        }
                    }
                }
            }
        }
        if members.len() < 20 {
            continue;
        }
        let (mut lo, mut hi, mut c) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY), DVec3::ZERO);
        let mut ds: Vec<f32> = vec![];
        for &i in &members {
            let (q, dv, _) = far[i];
            lo = lo.min(q);
            hi = hi.max(q);
            c += q;
            ds.push(dv.abs());
        }
        ds.sort_by(|x, y| x.total_cmp(y));
        let face = &m.faces[k0.2 as usize];
        let fid = face.tag.map(|t| t.feature);
        zones.push((
            members.len(),
            json!({
                "points": members.len(),
                "side": if k0.1 { "outside (the part sticks out of the scan)" } else { "inside (the part is short of the scan)" },
                "centre": r3(c / members.len() as f64), "box": [r3(lo), r3(hi)],
                "median": r(ds[ds.len() / 2] as f64), "max": r(*ds.last().unwrap_or(&0.0) as f64),
                "feature": fid, "feature_name": fid.and_then(|i| st.doc.feature(i)).map(|g| g.name.clone()),
                "face": { "index": k0.2, "kind": format!("{:?}", face.kind), "normal": r3(DVec3::from(face.normal)), "share": r(members.len() as f64 / on_face.get(&k0.2).copied().unwrap_or(1).max(1) as f64) },
            }),
        ));
    }
    zones.sort_by(|a, b| b.0.cmp(&a.0));
    let limit = p.get("limit").and_then(|v| v.as_u64()).unwrap_or(15) as usize;
    let a = d.area;
    let mut out = json!({
        "tolerance": r(tol), "range": r(st.dev_range),
        "area": r(a.area), "measured": r(a.reach), "within": r(a.within), "outside": r(a.outside), "inside": r(a.inside), "median": r(a.p50), "p95": r(a.p95), "mean": r(a.mean),
        "features": features,
        "elements": elements,
        "zones": zones.into_iter().take(limit).map(|z| z.1).collect::<Vec<_>>(),
        "note": "by area (mm²); mean: over the whole area, a point past 5 mm counted at 5 (falls as the part comes closer); distances in mm, + outside the scan; past 5 mm a point is not measured (measured: the share of the area within 5 mm; a sharp corner far out of the scan, faces inside it, parts beside it are not)",
    });
    if bx.is_some() {
        out["region"] = stats(&mut region);
    }
    Ok(out)
}

/// The camera a request names (or the user's), framed on what it asks.
fn camera_of(st: &AppState, p: &Value) -> Result<OrbitCamera, String> {
    let mut c = st.camera;
    if let Some(v) = p.get("view").and_then(|v| v.as_str()) {
        let (yaw, pitch) = match v {
            "front" => (0.0, 0.0),
            "back" => (180.0, 0.0),
            "right" => (90.0, 0.0),
            "left" => (-90.0, 0.0),
            "top" => (0.0, 89.0),
            "bottom" => (0.0, -89.0),
            "3d" | "perspective" => (24.0, 20.0),
            o => return Err(format!("view {o}: front, back, left, right, top, bottom or 3d")),
        };
        c.yaw = (yaw as f32).to_radians();
        c.pitch = (pitch as f32).to_radians();
        c.ortho = p.get("ortho").and_then(|v| v.as_bool()).unwrap_or(v != "3d" && v != "perspective");
    }
    if let Some(y) = p.get("yaw").and_then(|v| v.as_f64()) {
        c.yaw = (y as f32).to_radians();
    }
    if let Some(y) = p.get("pitch").and_then(|v| v.as_f64()) {
        c.pitch = (y as f32).to_radians().clamp(-1.55, 1.55);
    }
    if let Some(o) = p.get("ortho").and_then(|v| v.as_bool()) {
        c.ortho = o;
    }
    // framed: on features, on a box, on everything
    let frame_on = |c: &mut OrbitCamera, lo: DVec3, hi: DVec3| {
        let mut k = *c;
        k.frame(lo.as_vec3(), hi.as_vec3());
        // islands do not cover a capture: closer than the app's framing
        c.target = k.target;
        c.dist = k.dist / 1.3 * 0.78;
    };
    if let Some(ids) = p.get("frame_ids").and_then(|v| v.as_array()) {
        let ids: Vec<u32> = ids.iter().filter_map(|v| v.as_u64()).map(|v| v as u32).collect();
        let b = st.doc.features.iter().filter(|f| ids.contains(&f.id)).map(|f| f.bounds()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)));
        let (lo, hi) = b.ok_or("frame_ids: no such features")?;
        frame_on(&mut c, lo, hi);
    } else if let Some(bx) = p.get("frame_box").and_then(|v| v.as_array()) {
        let lo = bx.first().and_then(v3).ok_or("frame_box: [[lo], [hi]]")?;
        let hi = bx.get(1).and_then(v3).ok_or("frame_box: [[lo], [hi]]")?;
        frame_on(&mut c, lo, hi);
    } else if p.get("view").is_some() || p.get("frame_all").and_then(|v| v.as_bool()) == Some(true) {
        let mut b = st.model.as_ref().and_then(|m| m.bounds()).map(|(lo, hi)| (Vec3::from(lo).as_dvec3(), Vec3::from(hi).as_dvec3()));
        if let Some(rf) = st.reference.as_ref() {
            b = Some(b.map(|(a, h)| (a.min(rf.lo), h.max(rf.hi))).unwrap_or((rf.lo, rf.hi)));
        }
        if let Some((lo, hi)) = b {
            frame_on(&mut c, lo, hi);
        }
    }
    if let Some(f) = p.get("focus").and_then(v3) {
        c.target = f.as_vec3();
    }
    if let Some(d) = p.get("distance").and_then(|v| v.as_f64()) {
        c.dist = (d as f32).clamp(5.0, 5000.0);
    }
    Ok(c)
}

fn display_of(st: &AppState, p: &Value) -> Result<(bool, bool, bool), String> {
    let (mut shown, mut ghost) = (st.ref_shown, st.ref_ghost);
    match p.get("reference").and_then(|v| v.as_str()) {
        Some("solid") => (shown, ghost) = (true, false),
        Some("ghost") => (shown, ghost) = (true, true),
        Some("hidden") => shown = false,
        Some(o) => return Err(format!("reference {o}: solid, ghost or hidden")),
        None => {}
    }
    let dev = p.get("deviation").and_then(|v| v.as_bool()).unwrap_or(st.dev_on);
    // the deviation asked: the part alone, unless the reference is named
    if p.get("deviation").and_then(|v| v.as_bool()) == Some(true) && p.get("reference").is_none() {
        shown = false;
    }
    Ok((shown, ghost, dev))
}

fn capture_of(st: &mut AppState, p: &Value) -> Result<Capture, String> {
    let camera = camera_of(st, p)?;
    let (ref_shown, ref_ghost, dev_on) = display_of(st, p)?;
    let w = p.get("width").and_then(|v| v.as_u64()).unwrap_or(1200).clamp(64, 2400) as u32;
    let h = p.get("height").and_then(|v| v.as_u64()).unwrap_or(900).clamp(64, 2400) as u32;
    // a request that comes back here (not via `poll`) is made again for the host
    let (tx, _) = channel();
    let dummy = Request { method: String::new(), params: Value::Null, reply: tx, since: Instant::now() };
    Ok(Capture { req: dummy, size: [w, h], camera, ref_shown, ref_ghost, dev_on })
}

/// The user's view and display changed (no picture).
fn show(st: &mut AppState, p: &Value) -> Result<(), String> {
    st.camera = camera_of(st, p)?;
    let (shown, ghost, dev) = display_of(st, p)?;
    st.ref_shown = shown;
    st.ref_ghost = ghost;
    if dev != st.dev_on {
        st.set_deviation(dev);
    }
    if let Some(ids) = p.get("select").and_then(|v| v.as_array()) {
        st.selection = ids.iter().filter_map(|v| v.as_u64()).map(|v| v as u32).filter(|i| st.doc.feature(*i).is_some()).collect();
    }
    Ok(())
}

/// What lies under a spot of the last picture (x, y: fractions from its
/// top left): the part's face and its feature, and the scan's point.
fn pick(st: &AppState, p: &Value) -> Result<Value, String> {
    let shot = st.link.last_shot.ok_or("no capture yet")?;
    let x = p.get("x").and_then(|v| v.as_f64()).ok_or("x (0 to 1)")?;
    let y = p.get("y").and_then(|v| v.as_f64()).ok_or("y (0 to 1)")?;
    let size = [shot.size[0] as f32, shot.size[1] as f32];
    let view = shot.camera.view(size, [size[0] * 0.5, size[1] * 0.5]);
    let (o, d) = view.ray(x as f32 * size[0], y as f32 * size[1]);
    let mut out = json!({});
    if let Some(m) = st.model.as_ref() {
        if let Some(h) = st.picker.pick(m, o, d) {
            let f = &m.faces[h.face];
            out["part"] = json!({
                "point": r3(h.point.as_dvec3()),
                "feature": f.tag.map(|t| t.feature),
                "feature_name": f.tag.and_then(|t| st.doc.feature(t.feature)).map(|g| g.name.clone()),
                "face_kind": format!("{:?}", f.kind),
                "face_normal": r3(DVec3::from(f.normal)),
            });
        }
    }
    if let Some(rf) = st.reference.as_ref() {
        if let Some((q, t)) = rf.hit(o.as_dvec3(), d.as_dvec3()) {
            let _ = t;
            out["scan"] = json!({ "point": r3(q) });
        }
    }
    Ok(out)
}

/// The host has rendered the picture (RGBA, top row first).
pub fn finish_capture(st: &mut AppState, cap: Capture, rgba: &[u8]) {
    let [w, h] = cap.size;
    st.link.last_shot = Some(Shot { camera: cap.camera, size: cap.size });
    let mut png = vec![];
    let enc = image::codecs::png::PngEncoder::new(&mut png);
    let res = image::ImageEncoder::write_image(enc, rgba, w, h, image::ExtendedColorType::Rgba8);
    let c = cap.camera;
    cap.req.answer(res.map_err(|e| e.to_string()).map(|_| {
        json!({
            "png_base64": base64(&png),
            "width": w, "height": h,
            "camera": { "target": r3(c.target.as_dvec3()), "yaw_deg": r(c.yaw.to_degrees() as f64), "pitch_deg": r(c.pitch.to_degrees() as f64), "distance": r(c.dist as f64), "ortho": c.ortho },
            "note": "pick reads a spot of this picture (x, y as fractions from its top left)",
        })
    }));
}

/// Renders each picture asked for with `render(st, w, h) -> rgba` (the
/// host's renderer, the view alone): the camera and display set for it,
/// then put back.
pub fn render_captures(st: &mut AppState, mut render: impl FnMut(&AppState, u32, u32) -> Vec<u8>) {
    for cap in std::mem::take(&mut st.link.captures) {
        let keep = (st.camera, st.screen, st.view_rect, st.ref_shown, st.ref_ghost, st.dev_on);
        let [w, h] = cap.size;
        st.camera = cap.camera;
        st.screen = [w as f32, h as f32];
        st.view_rect = [0.0, 0.0, w as f32, h as f32];
        st.ref_shown = cap.ref_shown;
        st.ref_ghost = cap.ref_ghost;
        if st.dev_on != cap.dev_on {
            st.dev_on = cap.dev_on;
            st.model_gen += 1;
        }
        let rgba = render(st, w, h);
        let dev_changed = st.dev_on != keep.5;
        (st.camera, st.screen, st.view_rect, st.ref_shown, st.ref_ghost, st.dev_on) = keep;
        if dev_changed {
            st.model_gen += 1;
        }
        finish_capture(st, cap, &rgba);
    }
}

/// Reads a texture back (RGBA8; BGRA swapped).
pub fn read_rgba(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture) -> Vec<u8> {
    let (w, h) = (tex.width(), tex.height());
    let padded = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("capture readback"), size: (padded * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    buf.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let data = buf.slice(..).get_mapped_range().expect("mapped");
    let bgra = matches!(tex.format(), wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let s = (row * padded) as usize;
        let line = &data[s..s + (w * 4) as usize];
        if bgra {
            for px in line.chunks_exact(4) {
                out.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        } else {
            for px in line.chunks_exact(4) {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
    }
    drop(data);
    buf.unmap();
    out
}

/// The texture a picture is rendered into (the renderer's format).
pub fn target(device: &wgpu::Device, format: wgpu::TextureFormat, w: u32, h: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("capture"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}
