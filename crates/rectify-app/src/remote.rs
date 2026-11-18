//! The bridge to Claude Desktop (the AI pass, stage 3). The MCP server
//! `forme-rectify-mcp` (started by Claude Desktop) connects to the app on a
//! local port (127.0.0.1:47820, or FORME_RECTIFY_PORT) and sends one JSON
//! request per line: `{"method": "regions", "params": {...}}`. The app
//! answers one JSON line: `{"ok": ...}` or `{"error": "..."}`. Requests that
//! need a computation (a stage) or a picture wait for it.
//!
//! Only this computer can connect (the port listens on 127.0.0.1).

use crate::camera::ViewPreset;
use crate::state::{AppState, Style};
use glam::DVec3;
use rectify_core::prim::Prim;
use rectify_core::script::{self, Cmd, Entry, Ref};
use rectify_core::{Kind, Stage, Step, NONE};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

pub const DEFAULT_PORT: u16 = 47820;

pub struct Request {
    pub method: String,
    pub params: Value,
    reply: Sender<String>,
    /// Waited for a job already (answered whatever the state now).
    waited: bool,
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

/// A picture asked for: the host renders the view at this size (pixels)
/// and calls `finish_capture`.
pub struct Capture {
    pub req: Request,
    pub size: [u32; 2],
    /// Regions in colour, the others in clay (Regions style).
    pub highlight: Vec<u32>,
}

#[derive(Default)]
pub struct Pending {
    rx: Option<Receiver<Request>>,
    after_job: Vec<Request>,
    /// Came in while a job was running: handled when it ends.
    deferred: Vec<Request>,
    pub captures: Vec<Capture>,
    pub port: Option<u16>,
    /// Why the link could not start.
    pub error: Option<String>,
    /// Requests answered (shown in the status line).
    pub served: usize,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Pending {
    pub fn listening(&self) -> bool {
        self.rx.is_some()
    }
}

/// Listens on the local port; `wake` asks the app for a frame (the window
/// sleeps when nothing moves).
pub fn start(st: &mut AppState, wake: Arc<dyn Fn() + Send + Sync>) -> Result<u16, String> {
    let port = std::env::var("FORME_RECTIFY_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_PORT);
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            let e = format!("port {port}: {e}");
            st.remote.error = Some(e.clone());
            return Err(e);
        }
    };
    let (tx, rx) = channel::<Request>();
    let w = wake.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let tx = tx.clone();
            let w = w.clone();
            std::thread::spawn(move || serve(conn, tx, w));
        }
    });
    st.remote.rx = Some(rx);
    st.remote.port = Some(port);
    st.remote.wake = Some(wake);
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
        let req = Request { method: v["method"].as_str().unwrap_or("").to_string(), params: v.get("params").cloned().unwrap_or(json!({})), reply: rtx, waited: false };
        if tx.send(req).is_err() {
            return;
        }
        wake();
        let answer = rrx.recv().unwrap_or_else(|_| json!({ "error": "the app closed" }).to_string());
        if writeln!(out, "{answer}").is_err() {
            return;
        }
    }
}

/// Handles what came in, and what waited for a job that has ended.
pub fn poll(st: &mut AppState) {
    let mut reqs = vec![];
    if !st.busy() {
        for mut r in std::mem::take(&mut st.remote.after_job) {
            r.waited = true;
            reqs.push(r);
        }
        reqs.append(&mut st.remote.deferred);
    }
    if let Some(rx) = &st.remote.rx {
        while let Ok(r) = rx.try_recv() {
            reqs.push(r);
        }
    }
    for r in reqs {
        // the mesh is with the job while it runs: wait for it
        if st.busy() && !r.waited && r.method != "status" {
            st.remote.deferred.push(r);
        } else {
            handle(st, r);
        }
    }
}

fn stage_of(v: &Value) -> Option<Stage> {
    match v {
        Value::Number(n) => n.as_u64().and_then(|n| Stage::ALL.get(n as usize).copied()),
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "0" | "scan" => Some(Stage::Scan),
            "1" | "algorithm" => Some(Stage::Algorithm),
            "2" | "cleaned" => Some(Stage::Cleaned),
            "3" | "ai" | "guided" => Some(Stage::Guided),
            _ => None,
        },
        _ => None,
    }
}

fn style_of(s: &str) -> Option<Style> {
    match s.to_ascii_lowercase().as_str() {
        "surface" => Some(Style::Surface),
        "regions" => Some(Style::Regions),
        "diff" => Some(Style::Diff),
        _ => None,
    }
}

fn v3(v: &Value) -> Option<DVec3> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some(DVec3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?))
}

fn r3(x: DVec3) -> Value {
    json!([(x.x * 1000.0).round() / 1000.0, (x.y * 1000.0).round() / 1000.0, (x.z * 1000.0).round() / 1000.0])
}

fn r(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn handle(st: &mut AppState, req: Request) {
    st.remote.served += 1;
    let method = req.method.clone();
    let p = req.params.clone();
    match method.as_str() {
        "status" => req.answer(Ok(status(st))),
        "open" => {
            if req.waited {
                let ok = st.pipe.is_some();
                return req.answer(if ok { Ok(status(st)) } else { Err("could not open the file".into()) });
            }
            let Some(path) = p["path"].as_str() else { return req.answer(Err("path missing".into())) };
            st.open(path.into());
            st.remote.after_job.push(req);
        }
        "run" => {
            let stage = stage_of(&p["stage"]).unwrap_or(Stage::Guided);
            if let Some(s) = p["settings"].as_str() {
                let before = st.params;
                let unknown = st.params.apply(s);
                if !unknown.is_empty() {
                    return req.answer(Err(format!("unknown settings: {}", unknown.join(", "))));
                }
                if st.params != before {
                    if let Some(pp) = &mut st.pipe {
                        pp.clear_from(first_changed(&before, &st.params));
                    }
                }
            }
            if !req.waited && st.pipe.as_ref().map(|pp| !pp.ready(stage)).unwrap_or(false) {
                st.stage = stage;
                st.gen += 1;
                st.run_stage(stage);
                st.remote.after_job.push(req);
                return;
            }
            if st.stage != stage {
                st.stage = stage;
                st.gen += 1;
            }
            req.answer(Ok(stage_report(st, stage)))
        }
        "show" => {
            if let Err(e) = apply_view(st, &p) {
                return req.answer(Err(e));
            }
            req.answer(Ok(status(st)))
        }
        "capture" => {
            if let Err(e) = apply_view(st, &p) {
                return req.answer(Err(e));
            }
            // the stage shown must be computed first
            if !req.waited && st.pipe.as_ref().map(|pp| !pp.ready(st.stage)).unwrap_or(false) && st.style != Style::Regions {
                let s = st.stage;
                st.run_stage(s);
                st.remote.after_job.push(req);
                return;
            }
            let w = p["width"].as_u64().unwrap_or(1280).clamp(256, 2400) as u32;
            let h = p["height"].as_u64().unwrap_or(800).clamp(256, 1600) as u32;
            let highlight: Vec<u32> = p["highlight"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).map(|x| x as u32).collect()).unwrap_or_default();
            st.remote.captures.push(Capture { req, size: [w, h], highlight });
            if let Some(w) = &st.remote.wake {
                w();
            }
        }
        "pick" => req.answer(pick(st, &p)),
        "regions" => req.answer(regions(st, &p)),
        "region" => req.answer(region(st, &p)),
        "edit" => {
            let Some(list) = p["edits"].as_array().cloned() else { return req.answer(Err("edits missing (a list)".into())) };
            let mut out = vec![];
            for e in list {
                out.push(match parse_edit(st, &e) {
                    Ok(entry) => match st.add_edit(entry) {
                        Ok(t) => json!({ "ok": t }),
                        Err(t) => json!({ "error": t }),
                    },
                    Err(t) => json!({ "error": t }),
                });
            }
            let n = st.pipe.as_ref().map(|pp| pp.script.edits.len()).unwrap_or(0);
            req.answer(Ok(json!({ "results": out, "edits": n, "note": "stage 3 is computed again on the next run / capture of stage 3" })))
        }
        "edits" => req.answer(Ok(edits(st))),
        "remove_edit" => {
            let Some(i) = p["index"].as_u64() else { return req.answer(Err("index missing".into())) };
            st.remove_edit(i as usize);
            req.answer(Ok(edits(st)))
        }
        "clear_edits" => {
            let by = p["by"].as_str().unwrap_or("ai").to_string();
            let n = st.pipe.as_ref().map(|pp| pp.script.edits.len()).unwrap_or(0);
            for i in (0..n).rev() {
                let mine = st.pipe.as_ref().map(|pp| by == "all" || pp.script.edits[i].by == by).unwrap_or(false);
                if mine {
                    st.remove_edit(i);
                }
            }
            req.answer(Ok(edits(st)))
        }
        "undo" => {
            st.undo();
            req.answer(Ok(edits(st)))
        }
        "diff" => req.answer(diff(st, &p)),
        "fragments" => req.answer(fragments(st, &p)),
        _ => req.answer(Err(format!("unknown method \"{method}\""))),
    }
}

/// The first step a settings change touches (everything after it runs again).
fn first_changed(a: &rectify_core::Params, b: &rectify_core::Params) -> Step {
    if a.symmetry != b.symmetry || a.sym_tol != b.sym_tol {
        Step::Symmetry
    } else if a.smooth_on != b.smooth_on || a.smooth != b.smooth {
        Step::Smooth
    } else if a.seg != b.seg {
        Step::Segment
    } else if a.reg_on != b.reg_on || a.reg != b.reg {
        Step::Regularize
    } else if a.snap != b.snap {
        Step::Snap
    } else if a.revolve != b.revolve {
        Step::Revolve
    } else {
        Step::Clean
    }
}

fn apply_view(st: &mut AppState, p: &Value) -> Result<(), String> {
    if !p["stage"].is_null() {
        let s = stage_of(&p["stage"]).ok_or("stage: 0 scan, 1 algorithm, 2 cleaned, 3 ai")?;
        if st.stage != s {
            st.stage = s;
            st.gen += 1;
        }
    }
    if let Some(s) = p["style"].as_str() {
        let s = style_of(s).ok_or("style: surface, regions, diff")?;
        if st.style != s {
            st.style = s;
            st.gen += 1;
        }
    }
    if let Some(z) = p["zebra"].as_bool() {
        st.settings.zebra = z;
    }
    if let Some(e) = p["edges"].as_bool() {
        st.settings.edges = e;
        st.gen += 1;
    }
    if let Some(v) = p["view"].as_str() {
        match v.to_ascii_lowercase().as_str() {
            "3d" | "perspective" => st.camera.preset(ViewPreset::Perspective),
            "front" => st.camera.preset(ViewPreset::Front),
            "right" => st.camera.preset(ViewPreset::Right),
            "top" => st.camera.preset(ViewPreset::Top),
            "back" | "left" | "bottom" => {
                let (yaw, pitch) = match v {
                    "back" => (std::f32::consts::PI, 0.0),
                    "left" => (-std::f32::consts::FRAC_PI_2, 0.0),
                    _ => (0.0, -1.52),
                };
                st.camera.yaw = yaw;
                st.camera.pitch = pitch;
                st.camera.ortho = true;
            }
            "all" | "frame" => {
                st.frame_all();
                // no interface in a capture: closer
                st.camera.dist *= 0.62;
            }
            other => return Err(format!("view \"{other}\": 3d, front, back, left, right, top, bottom, all")),
        }
    }
    if let Some(y) = p["yaw"].as_f64() {
        st.camera.yaw = (y as f32).to_radians();
        st.camera.ortho = false;
    }
    if let Some(x) = p["pitch"].as_f64() {
        st.camera.pitch = (x as f32).to_radians().clamp(-1.55, 1.55);
        st.camera.ortho = false;
    }
    if let Some(r) = p["region"].as_u64() {
        let (lo, hi) = region_box(st, r as u32).ok_or("no such region")?;
        st.camera.target = ((lo + hi) * 0.5).as_vec3();
        st.camera.dist = ((hi - lo).length() as f32 * 1.6).max(12.0);
    }
    if let Some(c) = v3(&p["focus"]) {
        st.camera.target = c.as_vec3();
    }
    if let Some(d) = p["distance"].as_f64() {
        st.camera.dist = (d as f32).clamp(3.0, 5000.0);
    }
    Ok(())
}

/// What lies under a spot of a capture (x, y: fractions from the top left;
/// the capture's size and the current view).
fn pick(st: &mut AppState, p: &Value) -> Result<Value, String> {
    let (x, y) = (p["x"].as_f64().ok_or("x missing")?, p["y"].as_f64().ok_or("y missing")?);
    let w = p["width"].as_u64().unwrap_or(1280) as f32;
    let h = p["height"].as_u64().unwrap_or(800) as f32;
    let (screen, rect) = (st.screen, st.view_rect);
    st.screen = [w, h];
    st.view_rect = [0.0, 0.0, w, h];
    let hit = st.pick_hit([x as f32 * w, y as f32 * h]);
    st.screen = screen;
    st.view_rect = rect;
    let (t, at) = hit.ok_or("nothing there (the background)")?;
    let mut v = json!({ "point": r3(at), "triangle": t });
    let pp = st.pipe.as_ref().ok_or("no mesh")?;
    let seg = pp.seg_edit.as_ref().or(pp.seg_raw.as_ref());
    if let Some(seg) = seg {
        let l = seg.label[t];
        if l == NONE {
            v["region"] = json!(null);
            v["note"] = json!("free faces (no region)");
        } else {
            let mut q = prim_json(&seg.regions[l as usize].prim);
            q["id"] = json!(l);
            q["area_mm2"] = json!(r(seg.regions[l as usize].area));
            v["region"] = q;
        }
    }
    Ok(v)
}

fn region_box(st: &AppState, r: u32) -> Option<(DVec3, DVec3)> {
    let pp = st.pipe.as_ref()?;
    let seg = st.shown_seg().or(pp.seg_raw.as_ref())?;
    let reg = seg.regions.get(r as usize)?;
    if reg.faces.is_empty() {
        return None;
    }
    let m = pp.smooth_out();
    let mut lo = DVec3::splat(f64::INFINITY);
    let mut hi = DVec3::splat(f64::NEG_INFINITY);
    for &f in &reg.faces {
        for &v in &m.tris[f as usize] {
            lo = lo.min(m.pos[v as usize]);
            hi = hi.max(m.pos[v as usize]);
        }
    }
    Some((lo, hi))
}

fn status(st: &AppState) -> Value {
    let Some(p) = &st.pipe else {
        if st.busy() {
            return json!({ "busy": true, "note": "a stage is being computed; other requests wait for it" });
        }
        return json!({ "mesh": null, "note": "no mesh open: use open with the .obj path", "busy": false });
    };
    let (lo, hi) = p.original.bounds();
    let stages: Vec<Value> = Stage::ALL.iter().map(|s| json!({ "stage": s.number(), "name": s.label(), "ready": p.ready(*s) })).collect();
    let steps: Vec<Value> = Step::ALL
        .iter()
        .map(|s| {
            json!({ "step": s.label(), "algorithm_secs": p.time_of(*s, 0).map(r), "ai_lane_secs": if s.lane_index().is_some() { p.time_of(*s, 1).map(r) } else { None } })
        })
        .collect();
    json!({
        "mesh": st.file.as_ref().map(|f| f.display().to_string()).unwrap_or(st.name.clone()),
        "vertices": p.original.pos.len(),
        "triangles": p.original.tris.len(),
        "size_mm": r3(hi - lo),
        "box_min": r3(lo),
        "box_max": r3(hi),
        "stage_shown": st.stage.number(),
        "style": st.style.label(),
        "stages": stages,
        "steps": steps,
        "edits": p.script.edits.len(),
        "edits_by_ai": p.script.count("ai"),
        "edits_by_hand": p.script.count("hand"),
        "busy": st.busy(),
        "camera": { "target": r3(st.camera.target.as_dvec3()), "distance": r(st.camera.dist as f64), "yaw_deg": r(st.camera.yaw.to_degrees() as f64), "pitch_deg": r(st.camera.pitch.to_degrees() as f64) },
    })
}

fn stage_report(st: &AppState, stage: Stage) -> Value {
    let mut v = status(st);
    let Some(p) = &st.pipe else { return v };
    if let Some(l) = stage.lane() {
        let lane = &p.lanes[l];
        if let Some(s) = p.stage_seg(stage) {
            v["regions"] = json!({ "planes": s.count(Kind::Plane), "cylinders": s.count(Kind::Cylinder), "spheres": s.count(Kind::Sphere), "covered_pct": r(s.covered(&p.smooth_out().areas()) * 100.0) });
        }
        if let Some(sn) = &lane.snap {
            v["snap"] = json!({ "edges": sn.edges, "tangent": sn.tangent, "rejected": sn.rejected, "radius_groups": sn.radius_groups, "mean_move_mm": r(sn.mean_move), "max_move_mm": r(sn.max_move) });
        }
        if let Some(rv) = &lane.revolve {
            v["round"] = json!({ "pieces_done": rv.done(), "candidates": rv.pieces.len(), "vertices_moved": rv.moved });
        }
        if let Some(t) = &lane.tidy {
            v["clean"] = json!({ "rough_before": t.rough_before, "rough_after": t.rough_after, "smoothed": t.smoothed, "reverted": t.reverted, "kept": t.kept });
        }
        if l == 1 {
            v["edit_results"] = json!(p.edit_log.iter().map(|x| match x { Ok(t) => json!({ "ok": t }), Err(e) => json!({ "error": e }) }).collect::<Vec<_>>());
        }
    }
    v
}

fn prim_json(q: &Prim) -> Value {
    match *q {
        Prim::Plane { n, d } => json!({ "kind": "plane", "normal": r3(n), "offset": r(d) }),
        Prim::Cylinder { p, a, r: rad } => json!({ "kind": "cylinder", "axis": r3(a), "axis_point": r3(p), "radius": r(rad) }),
        Prim::Sphere { c, r: rad } => json!({ "kind": "sphere", "centre": r3(c), "radius": r(rad) }),
    }
}

/// The regions of a stage (1: the algorithm's, 3: the edited ones),
/// largest first.
fn regions(st: &AppState, p: &Value) -> Result<Value, String> {
    let pp = st.pipe.as_ref().ok_or("no mesh open")?;
    let stage = stage_of(&p["stage"]).unwrap_or(Stage::Guided);
    let seg = match stage {
        Stage::Guided => pp.seg_edit.as_ref().or(pp.seg_raw.as_ref()),
        Stage::Scan => pp.seg_raw.as_ref(),
        s => pp.stage_seg(s),
    }
    .ok_or("no regions yet: run stage 1 first")?;
    let m = pp.smooth_out();
    let kind = p["kind"].as_str().map(|k| k.to_ascii_lowercase());
    let min_area = p["min_area"].as_f64().unwrap_or(0.0);
    let near = v3(&p["near"]);
    let radius = p["radius"].as_f64().unwrap_or(5.0);
    let limit = p["limit"].as_u64().unwrap_or(80) as usize;
    let mut list: Vec<(usize, &rectify_core::segment::Region)> = seg.regions.iter().enumerate().filter(|(_, x)| !x.faces.is_empty() && x.area >= min_area).collect();
    if let Some(k) = &kind {
        list.retain(|(_, x)| x.prim.kind().label().to_lowercase() == *k);
    }
    if let Some(c) = near {
        list.retain(|(_, x)| x.faces.iter().any(|&f| (m.tri_centre(f as usize) - c).length() < radius));
    }
    list.sort_by(|a, b| b.1.area.partial_cmp(&a.1.area).unwrap());
    let total = list.len();
    let out: Vec<Value> = list
        .iter()
        .take(limit)
        .map(|(i, x)| {
            let mut lo = DVec3::splat(f64::INFINITY);
            let mut hi = DVec3::splat(f64::NEG_INFINITY);
            for &f in &x.faces {
                let c = m.tri_centre(f as usize);
                lo = lo.min(c);
                hi = hi.max(c);
            }
            let mut v = prim_json(&x.prim);
            v["id"] = json!(i);
            v["area_mm2"] = json!(r(x.area));
            v["rms_mm"] = json!(r(x.rms));
            v["at"] = json!(script::point_of(m, seg, *i as u32).map(|a| r3(DVec3::from_array(a))));
            v["box_min"] = r3(lo);
            v["box_max"] = r3(hi);
            v
        })
        .collect();
    Ok(json!({ "stage": stage.number(), "total": total, "shown": out.len(), "regions": out }))
}

/// Clusters of small regions that touch (directly or across a free
/// triangle): where a detailed part (ribs, fins, grooves) was cut into many
/// short pieces. Largest clusters first.
fn fragments(st: &AppState, p: &Value) -> Result<Value, String> {
    let pp = st.pipe.as_ref().ok_or("no mesh open")?;
    let stage = stage_of(&p["stage"]).unwrap_or(Stage::Guided);
    let seg = match stage {
        Stage::Guided => pp.seg_edit.as_ref().or(pp.seg_raw.as_ref()),
        Stage::Scan => pp.seg_raw.as_ref(),
        s => pp.stage_seg(s),
    }
    .ok_or("no regions yet: run stage 1 first")?;
    let max_area = p["max_area"].as_f64().unwrap_or(20.0);
    let min_count = p["min_count"].as_u64().unwrap_or(6) as usize;
    let limit = p["limit"].as_u64().unwrap_or(12) as usize;
    let small: Vec<bool> = seg.regions.iter().map(|x| !x.faces.is_empty() && x.area < max_area).collect();
    let n = seg.regions.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let topo = &pp.topo;
    let label = &seg.label;
    let mut join = |a: u32, b: u32| {
        if a == NONE || b == NONE || a == b || !small[a as usize] || !small[b as usize] {
            return;
        }
        let (x, y) = (find(&mut parent, a as usize), find(&mut parent, b as usize));
        if x != y {
            parent[x] = y;
        }
    };
    for t in 0..label.len() {
        let lt = label[t];
        let ns: Vec<u32> = topo.adj[t].iter().filter(|&&o| o != NONE).map(|&o| label[o as usize]).collect();
        if lt != NONE {
            for &l in &ns {
                join(lt, l);
            }
        } else {
            // across a free triangle
            for i in 0..ns.len() {
                for j in i + 1..ns.len() {
                    join(ns[i], ns[j]);
                }
            }
        }
    }
    let mut groups: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for i in 0..n {
        if small[i] {
            let root = find(&mut parent, i);
            groups.entry(root).or_default().push(i);
        }
    }
    let m = pp.smooth_out();
    let mut list: Vec<Vec<usize>> = groups.into_values().filter(|g| g.len() >= min_count).collect();
    list.sort_by_key(|g| std::cmp::Reverse(g.len()));
    let total = list.len();
    let out: Vec<Value> = list
        .iter()
        .take(limit)
        .map(|g| {
            let mut lo = DVec3::splat(f64::INFINITY);
            let mut hi = DVec3::splat(f64::NEG_INFINITY);
            let (mut area, mut cyl, mut pla, mut sph, mut sum, mut w) = (0.0f64, 0, 0, 0, DVec3::ZERO, 0.0f64);
            for &i in g {
                let x = &seg.regions[i];
                area += x.area;
                match x.prim.kind() {
                    rectify_core::Kind::Cylinder => cyl += 1,
                    rectify_core::Kind::Sphere => sph += 1,
                    _ => pla += 1,
                }
                for &f in &x.faces {
                    let c = m.tri_centre(f as usize);
                    lo = lo.min(c);
                    hi = hi.max(c);
                    sum += c;
                    w += 1.0;
                }
            }
            json!({
                "regions": g,
                "count": g.len(),
                "planes": pla, "cylinders": cyl, "spheres": sph,
                "area_mm2": r(area),
                "centre": r3(sum / w.max(1.0)),
                "box_min": r3(lo),
                "box_max": r3(hi),
            })
        })
        .collect();
    Ok(json!({ "stage": stage.number(), "max_area_mm2": max_area, "clusters_total": total, "clusters": out }))
}

/// One region and its neighbours (the regions it touches, by shared border length).
fn region(st: &AppState, p: &Value) -> Result<Value, String> {
    let pp = st.pipe.as_ref().ok_or("no mesh open")?;
    let id = p["id"].as_u64().ok_or("id missing")? as u32;
    let stage = stage_of(&p["stage"]).unwrap_or(Stage::Guided);
    let seg = match stage {
        Stage::Guided => pp.seg_edit.as_ref().or(pp.seg_raw.as_ref()),
        Stage::Scan => pp.seg_raw.as_ref(),
        s => pp.stage_seg(s),
    }
    .ok_or("no regions yet")?;
    let reg = seg.regions.get(id as usize).filter(|x| !x.faces.is_empty()).ok_or("no such region")?;
    let m = pp.smooth_out();
    let mut border: std::collections::HashMap<u32, f64> = Default::default();
    let mut free_border = 0.0;
    for &f in &reg.faces {
        let tri = m.tris[f as usize];
        for k in 0..3 {
            let o = pp.topo.adj[f as usize][k];
            if o == NONE {
                continue;
            }
            let l = seg.label[o as usize];
            if l == id {
                continue;
            }
            let len = (m.pos[tri[k] as usize] - m.pos[tri[(k + 1) % 3] as usize]).length();
            if l == NONE {
                free_border += len;
            } else {
                *border.entry(l).or_default() += len;
            }
        }
    }
    let mut nb: Vec<(u32, f64)> = border.into_iter().collect();
    nb.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let nbs: Vec<Value> = nb
        .iter()
        .take(30)
        .map(|(l, len)| {
            let x = &seg.regions[*l as usize];
            let mut v = prim_json(&x.prim);
            v["id"] = json!(l);
            v["area_mm2"] = json!(r(x.area));
            v["border_mm"] = json!(r(*len));
            v
        })
        .collect();
    let mut v = prim_json(&reg.prim);
    v["id"] = json!(id);
    v["area_mm2"] = json!(r(reg.area));
    v["rms_mm"] = json!(r(reg.rms));
    v["faces"] = json!(reg.faces.len());
    v["at"] = json!(script::point_of(m, seg, id).map(|a| r3(DVec3::from_array(a))));
    v["border_with_free_faces_mm"] = json!(r(free_border));
    v["neighbours"] = json!(nbs);
    Ok(v)
}

fn edits(st: &AppState) -> Value {
    let Some(p) = &st.pipe else { return json!([]) };
    let list: Vec<Value> = p
        .script
        .edits
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mut v = serde_json::to_value(e).unwrap_or(json!({}));
            if let Some(o) = v.as_object_mut() {
                // hand strokes carry face lists: their size only
                if let Some(f) = o.get("faces").and_then(|f| f.as_array()).map(|f| f.len()) {
                    o.insert("faces".into(), json!(format!("{f} faces")));
                }
                o.insert("index".into(), json!(i));
                if let Some(res) = p.edit_log.get(i) {
                    o.insert("result".into(), match res { Ok(t) => json!(t), Err(e) => json!(format!("FAILED: {e}")) });
                }
            }
            v
        })
        .collect();
    json!({ "edits": list, "file": st.script_path().map(|p| p.display().to_string()) })
}

/// An edit from the AI: region ids become points (they outlive a new
/// segmentation).
fn parse_edit(st: &AppState, e: &Value) -> Result<Entry, String> {
    let mut e = e.clone();
    let note = e.get("note").and_then(|n| n.as_str()).unwrap_or("").to_string();
    if let Some(o) = e.as_object_mut() {
        o.remove("note");
        o.remove("by");
    }
    // "region": 12 / "regions": [12, 40] → references
    let to_ref = |v: &Value| -> Value {
        match v.as_u64() {
            Some(id) => serde_json::to_value(st.region_ref_in_edit(id as u32)).unwrap_or(json!({ "id": id })),
            None => v.clone(),
        }
    };
    if let Some(o) = e.as_object_mut() {
        if let Some(v) = o.get("region").cloned() {
            o.insert("region".into(), to_ref(&v));
        }
        if let Some(Value::Array(a)) = o.get("regions").cloned() {
            o.insert("regions".into(), Value::Array(a.iter().map(to_ref).collect()));
        }
    }
    let cmd: Cmd = serde_json::from_value(e).map_err(|x| format!("bad edit: {x}"))?;
    if matches!(cmd, Cmd::Paint { .. } | Cmd::Place { .. }) {
        return Err("paint / place are hand tools: use merge, retype, free, same_radius, coaxial, coplanar, revolve, no_revolve, keep".into());
    }
    Ok(Entry { by: "ai".into(), note, cmd })
}

impl AppState {
    /// A region of stage 3's (edited) regions, named by a point on it.
    pub fn region_ref_in_edit(&self, id: u32) -> Ref {
        let at = self.pipe.as_ref().and_then(|p| {
            let seg = p.seg_edit.as_ref().or(p.seg_raw.as_ref())?;
            script::point_of(p.smooth_out(), seg, id)
        });
        Ref { id: Some(id), at }
    }
}

/// Where a stage is rougher than the AI mesh: clusters of rough vertices,
/// largest first (where to look, what to keep).
fn diff(st: &AppState, p: &Value) -> Result<Value, String> {
    let pp = st.pipe.as_ref().ok_or("no mesh open")?;
    let stage = stage_of(&p["stage"]).unwrap_or(Stage::Guided);
    let d = pp.stage_diff(stage).ok_or("this stage is not computed (run it first)")?;
    let m = pp.stage_mesh(stage).ok_or("not computed")?;
    let rough: Vec<usize> = (0..d.worse.len()).filter(|&v| d.worse[v] > 0.0).collect();
    // clusters: connected rough vertices (through rough neighbours, or a
    // neighbour of one)
    let mut comp = vec![u32::MAX; m.pos.len()];
    let mut clusters: Vec<(usize, DVec3, f32, DVec3, DVec3)> = vec![];
    for &s in &rough {
        if comp[s] != u32::MAX {
            continue;
        }
        let id = clusters.len() as u32;
        let mut stack = vec![s];
        comp[s] = id;
        let (mut n, mut c, mut w) = (0usize, DVec3::ZERO, 0f32);
        let mut lo = DVec3::splat(f64::INFINITY);
        let mut hi = DVec3::splat(f64::NEG_INFINITY);
        while let Some(v) = stack.pop() {
            n += 1;
            c += m.pos[v];
            w = w.max(d.worse[v]);
            lo = lo.min(m.pos[v]);
            hi = hi.max(m.pos[v]);
            for &u in pp.topo.verts_of(v) {
                let u = u as usize;
                if comp[u] == u32::MAX && d.worse[u] > 0.0 {
                    comp[u] = id;
                    stack.push(u);
                }
            }
        }
        clusters.push((n, c / n as f64, w, lo, hi));
    }
    clusters.sort_by(|a, b| b.0.cmp(&a.0));
    let limit = p["limit"].as_u64().unwrap_or(25) as usize;
    let moved: Vec<f32> = d.moved.clone();
    let mean_moved = moved.iter().map(|x| *x as f64).sum::<f64>() / moved.len().max(1) as f64;
    Ok(json!({
        "stage": stage.number(),
        "rough_vertices": rough.len(),
        "mean_move_mm": r(mean_moved),
        "clusters": clusters.iter().take(limit).map(|(n, c, w, lo, hi)| json!({ "vertices": n, "centre": r3(*c), "worst_deg": r(*w as f64), "box_min": r3(*lo), "box_max": r3(*hi) })).collect::<Vec<_>>(),
        "note": "rough = sharper than the AI mesh by more than the Clean margin; a keep edit {op: keep, at, radius} leaves a zone as the AI mesh",
    }))
}

/// The view as a PNG (base64), from RGBA pixels the host rendered.
pub fn finish_capture(st: &mut AppState, c: Capture, rgba: Vec<u8>) {
    let [w, h] = c.size;
    let mut png = vec![];
    let res = {
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut png).write_image(&rgba, w, h, image::ExtendedColorType::Rgba8)
    };
    match res {
        Ok(()) => {
            let mut v = json!({ "png_base64": base64(&png), "width": w, "height": h });
            v["view"] = status(st)["camera"].clone();
            v["stage"] = json!(st.stage.number());
            v["style"] = json!(st.style.label());
            c.req.answer(Ok(v))
        }
        Err(e) => c.req.answer(Err(e.to_string())),
    }
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for ch in data.chunks(3) {
        let b = [ch[0], *ch.get(1).unwrap_or(&0), *ch.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if ch.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if ch.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Renders the pending captures with the host's drawer (no interface: the
/// view alone, at the asked size). `render(st, w, h) -> RGBA`.
pub fn render_captures(st: &mut AppState, mut render: impl FnMut(&mut AppState, u32, u32) -> Vec<u8>) {
    for c in std::mem::take(&mut st.remote.captures) {
        let [w, h] = c.size;
        let (screen, rect, bare) = (st.screen, st.view_rect, st.bare);
        st.screen = [w as f32, h as f32];
        st.view_rect = [0.0, 0.0, w as f32, h as f32];
        st.bare = true;
        let lit = !c.highlight.is_empty();
        if lit {
            st.highlight = c.highlight.clone();
            st.gen += 1;
        }
        let px = render(st, w, h);
        if lit {
            st.highlight.clear();
            st.gen += 1;
        }
        st.screen = screen;
        st.view_rect = rect;
        st.bare = bare;
        finish_capture(st, c, px);
    }
}


/// Reads a texture back as RGBA (BGRA surfaces are swapped).
pub fn read_rgba(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture) -> Vec<u8> {
    let (w, h) = (tex.width(), tex.height());
    let padded = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (padded * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
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
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let s = (row * padded) as usize;
        out.extend_from_slice(&data[s..s + (w * 4) as usize]);
    }
    drop(data);
    buf.unmap();
    let bgra = matches!(tex.format(), wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
    if bgra {
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
    }
    // opaque: the background is drawn, alpha may not be 255
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    out
}
